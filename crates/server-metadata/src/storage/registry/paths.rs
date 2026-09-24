// Lexical comparison from Paseo utils/path.ts; it must never resolve symlinks.

pub(super) fn equivalent(left: &str, right: &str) -> bool {
    let windows = is_windows(left) || is_windows(right);
    normalize(left, windows) == normalize(right, windows)
}

fn is_windows(value: &str) -> bool {
    let bytes = value.as_bytes();
    (bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\'))
        || value.starts_with("\\\\")
        || value.starts_with("//?/")
}

fn normalize(value: &str, windows: bool) -> String {
    let mut path = if windows {
        value.replace('\\', "/").to_lowercase()
    } else {
        value.to_owned()
    };
    if windows {
        if let Some(tail) = path.strip_prefix("//?/unc/") {
            path = format!("//{tail}");
        } else if let Some(tail) = path.strip_prefix("//?/") {
            path = tail.to_owned();
        }
    }
    let (prefix, rest, absolute) = split_root(&path, windows);
    let mut parts = Vec::new();
    for part in rest.split('/') {
        match part {
            "" | "." => {}
            ".." if parts.last().is_some_and(|p| *p != "..") => {
                parts.pop();
            }
            ".." if absolute => {}
            _ => parts.push(part),
        }
    }
    let suffix = parts.join("/");
    if prefix.is_empty() && suffix.is_empty() {
        return ".".to_owned();
    }
    format!("{prefix}{suffix}")
}

fn split_root(path: &str, windows: bool) -> (String, &str, bool) {
    if windows && path.starts_with("//") {
        let without = path.trim_start_matches('/');
        let mut parts = without.splitn(3, '/');
        if let (Some(server), Some(share)) = (parts.next(), parts.next()) {
            return (
                format!("//{server}/{share}/"),
                parts.next().unwrap_or(""),
                true,
            );
        }
    }
    if windows && path.as_bytes().get(1) == Some(&b':') {
        let rest = &path[2..];
        let absolute = rest.starts_with('/');
        let prefix = if absolute {
            format!("{}/", &path[..2])
        } else {
            path[..2].to_owned()
        };
        return (prefix, rest.trim_start_matches('/'), absolute);
    }
    if path.starts_with('/') {
        ("/".to_owned(), path.trim_start_matches('/'), true)
    } else {
        (String::new(), path, false)
    }
}
