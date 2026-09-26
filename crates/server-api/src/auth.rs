use axum::http::{HeaderMap, StatusCode, Uri};
use secrecy::{ExposeSecret, SecretString};
use subtle::ConstantTimeEq;

use crate::{ApiError, ConfigError};

/// Validate an environment-sourced token without including it in errors.
///
/// # Errors
/// Rejects tokens outside 32–256 bytes or containing whitespace/non-ASCII bytes.
pub fn validate_token(token: &str) -> Result<(), ConfigError> {
    if !(32..=256).contains(&token.len()) || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(ConfigError::InvalidToken);
    }
    Ok(())
}

pub(super) fn single_header<'a>(
    headers: &'a HeaderMap,
    name: &str,
) -> Result<Option<&'a str>, ApiError> {
    let mut values = headers.get_all(name).iter();
    let value = values
        .next()
        .map(|v| v.to_str().map_err(|_| ApiError(StatusCode::BAD_REQUEST)))
        .transpose()?;
    if values.next().is_some() {
        return Err(ApiError(StatusCode::BAD_REQUEST));
    }
    Ok(value)
}

pub(super) fn validate_source(
    headers: &HeaderMap,
    authorities: &[String],
    browser: &crate::browser_auth::BrowserAuth,
) -> Result<(), ApiError> {
    let host = single_header(headers, "host")?.ok_or(ApiError(StatusCode::BAD_REQUEST))?;
    if !authorities
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(host))
    {
        return Err(ApiError(StatusCode::FORBIDDEN));
    }
    if let Some(origin) = single_header(headers, "origin")? {
        if browser.permits(origin) {
            return Ok(());
        }
        let origin: Uri = origin
            .parse()
            .map_err(|_| ApiError(StatusCode::FORBIDDEN))?;
        if origin.scheme_str() != Some("http")
            || !origin.authority().is_some_and(|a| {
                authorities
                    .iter()
                    .any(|v| v.eq_ignore_ascii_case(a.as_str()))
            })
            || origin
                .path_and_query()
                .is_some_and(|v| !v.as_str().is_empty() && v.as_str() != "/")
        {
            return Err(ApiError(StatusCode::FORBIDDEN));
        }
    }
    Ok(())
}

pub(super) fn authenticate(headers: &HeaderMap, expected: &SecretString) -> Result<(), ApiError> {
    let authorization = single_header(headers, "authorization")?
        .and_then(|value| value.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map(|(_, value)| value)
        .ok_or(ApiError(StatusCode::UNAUTHORIZED))?;
    if bool::from(
        authorization
            .as_bytes()
            .ct_eq(expected.expose_secret().as_bytes()),
    ) {
        Ok(())
    } else {
        Err(ApiError(StatusCode::UNAUTHORIZED))
    }
}

#[cfg(test)]
mod tests;
