//! Private content-addressed provider image artifacts shared by live and history projections.

use std::fmt::Write as _;
use std::io::Write;
use std::path::{Path, PathBuf};

use base64::Engine;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::ports::agent_session::AgentSessionError;

#[derive(Debug, Clone)]
pub(super) struct ImageStore {
    directory: PathBuf,
}

impl Default for ImageStore {
    fn default() -> Self {
        Self::new(std::env::temp_dir().join(format!("ait-provider-images-{}", std::process::id())))
    }
}

impl ImageStore {
    pub(super) const fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    pub(super) fn render(&self, image: &Value) -> Result<String, AgentSessionError> {
        let source = image.as_str().or_else(|| {
            ["path", "savedPath", "saved_path", "url"]
                .into_iter()
                .find_map(|key| image[key].as_str())
        });
        if let Some(source) = source.filter(|source| {
            !source.starts_with("data:")
                && (source.starts_with("http://")
                    || source.starts_with("https://")
                    || windows_path(source)
                    || Path::new(source).is_absolute())
        }) {
            if source.chars().any(char::is_control) {
                return Err(AgentSessionError::Failed);
            }
            return Ok(format!("![Image]({})", encode_source(source)));
        }
        let data = image["data"]
            .as_str()
            .or_else(|| image["source"]["data"].as_str())
            .or(source)
            .ok_or(AgentSessionError::Failed)?;
        let mime = image["mimeType"]
            .as_str()
            .or_else(|| image["mime_type"].as_str())
            .or_else(|| image["source"]["media_type"].as_str())
            .unwrap_or("image/png");
        let (mime, data) = if let Some(url) = data.strip_prefix("data:") {
            url.split_once(";base64,")
                .ok_or(AgentSessionError::Failed)?
        } else {
            (mime, data)
        };
        let extension = match mime {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "image/gif" => "gif",
            "image/webp" => "webp",
            "image/bmp" => "bmp",
            "image/tiff" => "tiff",
            _ => return Err(AgentSessionError::Failed),
        };
        if data.is_empty() || data.len() > 2 * 1024 * 1024 {
            return Err(AgentSessionError::Failed);
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|_| AgentSessionError::Failed)?;
        self.prepare()?;
        let path = self
            .directory
            .join(format!("{:x}.{extension}", Sha256::digest(&bytes)));
        match path.symlink_metadata() {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                if metadata.len() != bytes.len() as u64
                    || std::fs::read(&path).map_err(|_| AgentSessionError::Failed)? != bytes
                {
                    return Err(AgentSessionError::Failed);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut temporary = tempfile::NamedTempFile::new_in(&self.directory)
                    .map_err(|_| AgentSessionError::Failed)?;
                temporary
                    .write_all(&bytes)
                    .map_err(|_| AgentSessionError::Failed)?;
                temporary
                    .as_file()
                    .sync_all()
                    .map_err(|_| AgentSessionError::Failed)?;
                match temporary.persist_noclobber(&path) {
                    Ok(_) => {}
                    Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                        let metadata = path
                            .symlink_metadata()
                            .map_err(|_| AgentSessionError::Failed)?;
                        if !metadata.is_file()
                            || metadata.file_type().is_symlink()
                            || metadata.len() != bytes.len() as u64
                            || std::fs::read(&path).map_err(|_| AgentSessionError::Failed)? != bytes
                        {
                            return Err(AgentSessionError::Failed);
                        }
                    }
                    Err(_) => return Err(AgentSessionError::Failed),
                }
            }
            Ok(_) | Err(_) => return Err(AgentSessionError::Failed),
        }
        Ok(format!(
            "![Image]({})",
            encode_source(path.to_str().ok_or(AgentSessionError::Failed)?)
        ))
    }

    fn prepare(&self) -> Result<(), AgentSessionError> {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&self.directory)
            .map_err(|_| AgentSessionError::Failed)?;
        let metadata = self
            .directory
            .symlink_metadata()
            .map_err(|_| AgentSessionError::Failed)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(AgentSessionError::Failed);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(AgentSessionError::Failed);
            }
        }
        Ok(())
    }

    pub(super) fn split(&self, content: &Value) -> Result<(Value, Vec<String>), AgentSessionError> {
        let mut output = content.clone();
        let mut images = Vec::new();
        let blocks = if output.is_array() {
            output.as_array_mut()
        } else {
            output.get_mut("content").and_then(Value::as_array_mut)
        };
        if let Some(blocks) = blocks {
            for block in blocks {
                if block["type"] == "image" {
                    images.push(self.render(block)?);
                    *block = json!({"type":"text","text":"[Image]"});
                }
            }
        }
        Ok((output, images))
    }
}

fn encode_source(source: &str) -> String {
    let normalized;
    let source = if windows_path(source) {
        normalized = source.replace('\\', "/");
        &normalized
    } else {
        source
    };
    let mut encoded = String::with_capacity(source.len());
    for byte in source.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~/:%?=&+#@".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn windows_path(source: &str) -> bool {
    source.starts_with("\\\\")
        || (source
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic)
            && source.as_bytes().get(1) == Some(&b':')
            && source
                .as_bytes()
                .get(2)
                .is_some_and(|byte| matches!(byte, b'/' | b'\\')))
}

#[cfg(test)]
mod tests;
