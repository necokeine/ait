//! Bounded native Workflow result projection. Only announced workflows invoke this reader.

use serde_json::Value;

mod replay;
pub(super) use replay::histories;
use std::io::Read;
use std::path::Path;

pub(super) fn read(path: &str) -> Option<String> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return None;
    }
    let metadata = path.symlink_metadata().ok()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 1024 * 1024 {
        return None;
    }
    let mut data = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut data)
        .ok()?;
    if data.len() > 1024 * 1024 {
        return None;
    }
    let result: Value = serde_json::from_slice(&data).ok()?;
    format(&result["result"])
}

pub(super) fn format(value: &Value) -> Option<String> {
    let mut value = value;
    for _ in 0..8 {
        let Some(object) = value.as_object().filter(|object| object.len() == 1) else {
            break;
        };
        value = object.values().next()?;
    }
    let text = match value {
        Value::Null => return None,
        Value::String(text) => text.trim().to_owned(),
        Value::Number(_) | Value::Bool(_) => value.to_string(),
        _ => format!(
            "```json\n{}\n```",
            serde_json::to_string_pretty(value).ok()?
        ),
    };
    if text.is_empty() {
        return None;
    }
    if text.len() <= 100_000 {
        return Some(text);
    }
    let mut end = 100_000;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Some(format!("{}\n[Workflow output truncated]", &text[..end]))
}

#[cfg(test)]
mod tests;
