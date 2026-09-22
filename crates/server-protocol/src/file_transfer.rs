//! Paseo-compatible binary file transfer framing.

use serde::{Deserialize, Serialize};

/// Transfer chunk byte limit, matching Paseo's streaming chunk size.
pub const CHUNK_BYTES: usize = 256 * 1024;

/// Metadata advertised before streaming a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileBegin {
    /// MIME type.
    pub mime: String,
    /// Byte length.
    pub size: u64,
    /// `utf-8` or `binary`.
    pub encoding: String,
    /// Display modification timestamp.
    pub modified_at: String,
    /// Opaque disk revision.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// Optional download name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
}

/// Binary payload following a one-byte opcode and UTF-8 request identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileFrame {
    /// Opcode 0x10, then a big-endian u16 metadata length and JSON.
    Begin(FileBegin),
    /// Opcode 0x11 and file bytes.
    Chunk(Vec<u8>),
    /// Opcode 0x12 without a body.
    End,
}

/// Decode a bounded transfer frame; reject malformed identifiers and lengths.
#[must_use]
pub fn decode(bytes: &[u8]) -> Option<(String, FileFrame)> {
    let length = usize::from(*bytes.get(1)?);
    if length == 0 {
        return None;
    }
    let request_id = std::str::from_utf8(bytes.get(2..2 + length)?)
        .ok()?
        .to_owned();
    if !crate::valid_id(&request_id) {
        return None;
    }
    let body = bytes.get(2 + length..)?;
    let frame = match bytes[0] {
        0x10 => {
            let size = usize::from(u16::from_be_bytes([*body.first()?, *body.get(1)?]));
            if size != body.len() - 2 {
                return None;
            }
            let metadata: FileBegin = serde_json::from_slice(&body[2..]).ok()?;
            if metadata.mime.is_empty() || !matches!(metadata.encoding.as_str(), "utf-8" | "binary")
            {
                return None;
            }
            FileFrame::Begin(metadata)
        }
        0x11 if body.len() <= CHUNK_BYTES => FileFrame::Chunk(body.to_vec()),
        0x12 if body.is_empty() => FileFrame::End,
        _ => return None,
    };
    Some((request_id, frame))
}

/// Encode a transfer frame with Paseo's byte layout.
///
/// # Errors
/// Rejects identifiers, metadata, or chunks exceeding their framing limits.
pub fn encode(request_id: &str, frame: &FileFrame) -> Result<Vec<u8>, crate::ErrorCode> {
    let length = u8::try_from(request_id.len()).map_err(|_| crate::ErrorCode::InvalidMessage)?;
    if !crate::valid_id(request_id) || length == 0 {
        return Err(crate::ErrorCode::InvalidMessage);
    }
    let opcode = match frame {
        FileFrame::Begin(_) => 0x10,
        FileFrame::Chunk(_) => 0x11,
        FileFrame::End => 0x12,
    };
    let mut bytes = vec![opcode, length];
    bytes.extend_from_slice(request_id.as_bytes());
    match frame {
        FileFrame::Begin(metadata) => {
            let json =
                serde_json::to_vec(metadata).map_err(|_| crate::ErrorCode::InvalidMessage)?;
            let length = u16::try_from(json.len()).map_err(|_| crate::ErrorCode::InvalidMessage)?;
            bytes.extend_from_slice(&length.to_be_bytes());
            bytes.extend_from_slice(&json);
        }
        FileFrame::Chunk(chunk) if chunk.len() <= CHUNK_BYTES => bytes.extend_from_slice(chunk),
        FileFrame::Chunk(_) => return Err(crate::ErrorCode::InvalidMessage),
        FileFrame::End => {}
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests;
