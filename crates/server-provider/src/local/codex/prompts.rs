//! Native custom-prompt files and bounded argument expansion; never shell evaluation.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::ports::agent_session::AgentSessionError;

pub(super) fn directory() -> Result<PathBuf, AgentSessionError> {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
        .filter(|home| home.is_absolute())
        .map(|home| home.join("prompts"))
        .ok_or(AgentSessionError::Unavailable)
}

pub(super) fn list(directory: &Path) -> Result<Vec<Value>, AgentSessionError> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(_) => return Err(AgentSessionError::Failed),
    };
    let mut commands = Vec::new();
    for (index, entry) in entries.enumerate() {
        if index >= 1024 {
            return Err(AgentSessionError::Failed);
        }
        let entry = entry.map_err(|_| AgentSessionError::Failed)?;
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name
            .to_str()
            .and_then(|name| name.strip_suffix(".md"))
            .filter(|name| valid_name(name))
        else {
            continue;
        };
        let (metadata, _) = read(&entry.path())?;
        commands.push(json!({"name":format!("prompts:{name}"),"description":metadata.get("description").map_or("Custom prompt",String::as_str),
            "argumentHint":metadata.get("argument-hint").or_else(|| metadata.get("argument_hint")).map_or("",String::as_str),"kind":"command"}));
    }
    commands.sort_by(|left, right| left["name"].as_str().cmp(&right["name"].as_str()));
    Ok(commands)
}

pub(super) fn invoke(
    directory: &Path,
    name: &str,
    args: &str,
) -> Result<String, AgentSessionError> {
    if !valid_name(name) {
        return Err(AgentSessionError::Rejected);
    }
    let (_, body) = read(&directory.join(format!("{name}.md")))?;
    expand(&body, args)
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !matches!(name, "." | "..")
        && !name.contains(['/', '\\'])
        && !name.chars().any(char::is_control)
}

fn read(path: &Path) -> Result<(BTreeMap<String, String>, String), AgentSessionError> {
    let metadata = path
        .symlink_metadata()
        .map_err(|_| AgentSessionError::Rejected)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 65536 {
        return Err(AgentSessionError::Rejected);
    }
    let mut content = String::new();
    std::fs::File::open(path)
        .map_err(|_| AgentSessionError::Rejected)?
        .take(65537)
        .read_to_string(&mut content)
        .map_err(|_| AgentSessionError::Rejected)?;
    if content.len() > 65536 {
        return Err(AgentSessionError::Rejected);
    }
    let lines: Vec<_> = content.split('\n').collect();
    let mut metadata = BTreeMap::new();
    if lines.first().is_none_or(|line| line.trim() != "---") {
        return Ok((metadata, content));
    }
    let Some(end) = lines
        .iter()
        .skip(1)
        .position(|line| line.trim() == "---")
        .map(|index| index + 1)
    else {
        return Ok((metadata, content));
    };
    for line in &lines[1..end] {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let value = value.trim().trim_matches(['\'', '"']);
            if !key.trim().is_empty() && !value.is_empty() {
                metadata.insert(key.trim().to_owned(), value.to_owned());
            }
        }
    }
    Ok((metadata, lines[end + 1..].join("\n")))
}

fn tokens(args: &str) -> Result<Vec<String>, AgentSessionError> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut characters = args.chars().peekable();
    while let Some(character) = characters.next() {
        if let Some(delimiter) = quote {
            if character == delimiter {
                quote = None;
                continue;
            }
            if character == '\\'
                && characters
                    .peek()
                    .is_some_and(|next| *next == delimiter || matches!(next, '\\' | 'n' | 't'))
            {
                current.push(match characters.next() {
                    Some('n') => '\n',
                    Some('t') => '\t',
                    Some(next) => next,
                    None => return Err(AgentSessionError::Rejected),
                });
            } else {
                current.push(character);
            }
        } else if matches!(character, '\'' | '"') {
            quote = Some(character);
        } else if character.is_whitespace() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
        } else {
            current.push(character);
        }
        if words.len() > 128 {
            return Err(AgentSessionError::Rejected);
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    Ok(words)
}

fn expand(template: &str, args: &str) -> Result<String, AgentSessionError> {
    if template.len() > 65536 || args.len() > 65536 {
        return Err(AgentSessionError::Rejected);
    }
    let words = tokens(args.trim())?;
    let mut named = BTreeMap::new();
    let mut positional = Vec::new();
    for word in &words {
        if let Some((key, value)) = word.split_once('=').filter(|(key, _)| !key.is_empty()) {
            named.insert(key, value);
        } else {
            positional.push(word.as_str());
        }
    }
    let mut named: Vec<_> = named.into_iter().collect();
    named.sort_by_key(|(key, _)| std::cmp::Reverse(key.len()));
    let mut output = String::new();
    let mut rest = template;
    while let Some(character) = rest.chars().next() {
        rest = &rest[character.len_utf8()..];
        if character != '$' {
            output.push(character);
        } else if let Some(next) = rest.strip_prefix('$') {
            output.push('$');
            rest = next;
        } else if let Some(next) = rest.strip_prefix("ARGUMENTS") {
            output.push_str(args.trim());
            rest = next;
        } else if let Some(index) = rest
            .chars()
            .next()
            .filter(|character| matches!(character, '1'..='9'))
        {
            output.push_str(
                positional
                    .get(index as usize - '1' as usize)
                    .copied()
                    .unwrap_or_default(),
            );
            rest = &rest[1..];
        } else if let Some((key, value)) = named.iter().find(|(key, _)| {
            rest.strip_prefix(key).is_some_and(|suffix| {
                suffix
                    .chars()
                    .next()
                    .is_none_or(|ch| !ch.is_ascii_alphanumeric() && ch != '_')
            })
        }) {
            output.push_str(value);
            rest = &rest[key.len()..];
        } else {
            output.push('$');
        }
        if output.len() > 65536 {
            return Err(AgentSessionError::Rejected);
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests;
