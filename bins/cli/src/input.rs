//! CLI-owned input boundaries. File contents never appear in diagnostics.

use std::{
    fs,
    io::{self, Read},
    path::Path,
};

use serde::de::DeserializeOwned;

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
