//! CLI-owned input boundaries. File contents never appear in diagnostics.

use std::{
    fs,
    io::{self, Read},
    path::Path,
};

use serde::de::DeserializeOwned;

/// Source metadata supplied alongside the reader by the real CLI boundary.
#[derive(Clone, Copy)]
pub(crate) enum StdinSource {
    Terminal,
    Redirected,
}

pub(crate) fn id(value: &str) -> Result<String, String> {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        Err("expected a non-empty ID without control characters".into())
    } else {
        Ok(value.to_owned())
    }
}

pub(crate) fn url(value: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(value).map_err(|_| "expected an HTTP(S) URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("expected an HTTP(S) URL without credentials".into());
    }
    Ok(url)
}

pub(crate) fn host(value: &str) -> Result<String, String> {
    let address = value
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(value);
    if let Ok(address) = address.parse::<std::net::Ipv6Addr>() {
        return Ok(format!("[{address}]"));
    }
    let error = "expected a hostname or IP address without a scheme, port, credentials or path";
    if value.is_empty()
        || value.chars().any(|character| {
            character.is_whitespace() || character.is_control() || ":/\\?#@%[]".contains(character)
        })
    {
        return Err(error.into());
    }
    reqwest::Url::parse(&format!("http://{value}"))
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .ok_or_else(|| error.into())
}

pub(crate) fn read(path: &Path, stdin: &mut dyn Read) -> Result<String, io::Error> {
    let mut text = String::new();
    if path == Path::new("-") {
        stdin
            .read_to_string(&mut text)
            .map_err(|_| invalid("could not read UTF-8 stdin"))?;
    } else {
        text = fs::read_to_string(path).map_err(|_| invalid("could not read UTF-8 input file"))?;
    }
    Ok(text)
}

pub(crate) fn json<T: DeserializeOwned>(path: &Path, stdin: &mut dyn Read) -> Result<T, io::Error> {
    let text = read(path, stdin)?;
    serde_json::from_str(&text).map_err(|error| {
        invalid(&format!(
            "invalid entity JSON at line {}, column {}",
            error.line(),
            error.column()
        ))
    })
}

pub(crate) fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
