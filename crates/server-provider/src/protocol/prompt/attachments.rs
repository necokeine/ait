use std::fmt::Write;

use serde_json::Value;

use crate::ports::agent_session::AgentSessionError;

pub(super) fn render(value: &Value) -> Result<String, AgentSessionError> {
    if !value.is_object()
        || serde_json::to_vec(value)
            .map_err(|_| AgentSessionError::Rejected)?
            .len()
            > 128 * 1024
    {
        return Err(AgentSessionError::Rejected);
    }
    match value["type"].as_str() {
        Some("text") if value["mimeType"] == "text/plain" => Ok(text(value, "text")?.to_owned()),
        Some("uploaded_file") => {
            text(value, "id")?;
            let path = text(value, "path")?;
            if !std::path::Path::new(path).is_absolute() {
                return Err(AgentSessionError::Rejected);
            }
            Ok(format!(
                "Uploaded file: {}\nPath: {path}\nMIME: {}\nSize: {} bytes",
                text(value, "fileName")?,
                text(value, "mimeType")?,
                value["size"].as_u64().ok_or(AgentSessionError::Rejected)?
            ))
        }
        Some("github_pr" | "forge_change_request" | "github_issue" | "forge_issue") => forge(value),
        Some("review") if value["mimeType"] == "application/paseo-review" => review(value),
        _ => Err(AgentSessionError::Rejected),
    }
}

fn text<'a>(value: &'a Value, field: &str) -> Result<&'a str, AgentSessionError> {
    value[field]
        .as_str()
        .filter(|text| !text.contains('\0'))
        .ok_or(AgentSessionError::Rejected)
}

fn forge(value: &Value) -> Result<String, AgentSessionError> {
    let number = value["number"]
        .as_u64()
        .filter(|number| *number > 0)
        .ok_or(AgentSessionError::Rejected)?;
    let kind = if matches!(
        value["type"].as_str(),
        Some("github_pr" | "forge_change_request")
    ) {
        "change request"
    } else {
        "issue"
    };
    let mut result = format!(
        "{} {kind} #{number}: {}\n{}",
        value["forge"].as_str().unwrap_or("github"),
        text(value, "title")?,
        text(value, "url")?
    );
    for (field, label) in [
        ("projectPath", "Project"),
        ("baseRefName", "Base"),
        ("headRefName", "Head"),
    ] {
        if let Some(text) = value[field].as_str() {
            let _ = write!(result, "\n{label}: {text}");
        }
    }
    if let Some(body) = value["body"].as_str() {
        let _ = write!(result, "\n\n{body}");
    }
    Ok(result)
}

fn review(value: &Value) -> Result<String, AgentSessionError> {
    let mode = text(value, "mode")?;
    if !matches!(mode, "base" | "uncommitted") {
        return Err(AgentSessionError::Rejected);
    }
    let comments = value["comments"]
        .as_array()
        .filter(|comments| comments.len() <= 256)
        .ok_or(AgentSessionError::Rejected)?;
    let mut result = format!("Review attachment ({mode})\nCWD: {}", text(value, "cwd")?);
    if let Some(base) = value["baseRef"].as_str() {
        let _ = write!(result, "\nBase: {base}");
    }
    for (index, comment) in comments.iter().enumerate() {
        let line = comment["lineNumber"]
            .as_u64()
            .filter(|line| *line > 0)
            .ok_or(AgentSessionError::Rejected)?;
        let side = text(comment, "side")?;
        if !matches!(side, "old" | "new") {
            return Err(AgentSessionError::Rejected);
        }
        let _ = write!(
            result,
            "\n\nComment {}: {}:{side}:{line}\n{}\n{}",
            index + 1,
            text(comment, "filePath")?,
            text(comment, "body")?,
            text(&comment["context"], "hunkHeader")?
        );
        let lines = comment["context"]["lines"]
            .as_array()
            .filter(|lines| lines.len() <= 1024)
            .ok_or(AgentSessionError::Rejected)?;
        for line in lines {
            let prefix = if *line == comment["context"]["targetLine"] {
                "> "
            } else {
                "  "
            };
            let marker = match line["type"].as_str() {
                Some("add") => '+',
                Some("remove") => '-',
                Some("context") => ' ',
                _ => return Err(AgentSessionError::Rejected),
            };
            let old = line["oldLineNumber"]
                .as_u64()
                .map_or_else(|| "-".to_owned(), |line| line.to_string());
            let new = line["newLineNumber"]
                .as_u64()
                .map_or_else(|| "-".to_owned(), |line| line.to_string());
            let _ = write!(
                result,
                "\n{prefix}{old:>2} {new:>2} {marker}{}",
                text(line, "content")?
            );
        }
    }
    Ok(result)
}
