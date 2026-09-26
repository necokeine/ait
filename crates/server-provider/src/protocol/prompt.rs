//! Bounded rich prompts shared by provider ports and request admission.

mod attachments;

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::ports::agent_session::AgentSessionError;

/// An inline raster image, using the existing Paseo wire format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromptImage {
    /// Strict standard base64 bytes; data URLs belong in provider output conversion.
    pub data: String,
    /// Supported raster media type.
    pub mime_type: String,
}

impl PromptImage {
    /// Decode a bounded raster payload or return a safe rejection before native admission.
    /// # Errors
    /// Rejects unsupported MIME, malformed base64, empty or oversized data.
    pub fn decode(&self) -> Result<Vec<u8>, AgentSessionError> {
        if !matches!(
            self.mime_type.as_str(),
            "image/png" | "image/jpeg" | "image/gif" | "image/webp"
        ) || self.data.is_empty()
            || self.data.len() > 768 * 1024
        {
            return Err(AgentSessionError::Rejected);
        }
        base64::engine::general_purpose::STANDARD
            .decode(&self.data)
            .map_err(|_| AgentSessionError::Rejected)
    }
}

/// One complete logical user input. Voice and schedule callers can keep using plain text.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentPrompt {
    /// User-authored text, at most 64 KiB; may be empty when attachments are present.
    #[serde(default)]
    pub text: String,
    /// Optional inline images, within the server's existing one MiB request budget.
    #[serde(default)]
    pub images: Vec<PromptImage>,
    /// Existing Paseo text, review, forge and uploaded-file attachment objects.
    #[serde(default)]
    pub attachments: Vec<Value>,
    /// Client-supplied stable input identity, preserved independently of native turn IDs.
    pub client_message_id: Option<String>,
    /// Optional native structured-output constraint for this input.
    pub output_schema: Option<Value>,
}

impl AgentPrompt {
    /// Construct an unadorned text prompt without choosing a message identity.
    #[must_use]
    pub fn text(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            ..Self::default()
        }
    }

    /// Validate the entire input before reserving or submitting any part of it.
    /// # Errors
    /// Rejects empty, oversized or malformed input, including unsupported attachment shapes.
    pub fn validate(&self) -> Result<(), AgentSessionError> {
        if self.text.len() > 65536
            || self.text.contains('\0')
            || self.images.len() > 8
            || self.attachments.len() > 32
            || (self.text.trim().is_empty()
                && self.images.is_empty()
                && self.attachments.is_empty())
            || self.client_message_id.as_ref().is_some_and(|id| {
                id.is_empty() || id.len() > 128 || id.chars().any(char::is_control)
            })
            || self
                .output_schema
                .as_ref()
                .is_some_and(|schema| !schema.is_object())
            || serde_json::to_vec(self)
                .map_err(|_| AgentSessionError::Rejected)?
                .len()
                > 896 * 1024
        {
            return Err(AgentSessionError::Rejected);
        }
        for image in &self.images {
            image.decode()?;
        }
        let mut text_bytes = self.text.len();
        for attachment in &self.attachments {
            text_bytes = text_bytes.saturating_add(attachments::render(attachment)?.len());
        }
        if text_bytes > 192 * 1024 {
            return Err(AgentSessionError::Rejected);
        }
        Ok(())
    }

    /// Whether a legacy text-only provider can receive this input without losing intent.
    #[must_use]
    pub fn is_plain_text(&self) -> bool {
        self.images.is_empty()
            && self.attachments.is_empty()
            && self.output_schema.is_none()
            && self.client_message_id.is_none()
    }

    /// Produce provider-independent ordered text/image blocks after validation.
    /// # Errors
    /// Returns malformed or oversized prompt errors.
    pub fn blocks(&self) -> Result<Vec<Value>, AgentSessionError> {
        self.validate()?;
        let mut blocks = Vec::new();
        for attachment in self
            .attachments
            .iter()
            .filter(|attachment| attachment["contextKind"] == "chat_history")
        {
            blocks.push(json!({"type":"text","text":attachments::render(attachment)?}));
        }
        if !self.text.trim().is_empty() {
            blocks.push(json!({"type":"text","text":self.text}));
        }
        for image in &self.images {
            blocks.push(json!({"type":"image","data":image.data,"mimeType":image.mime_type}));
        }
        for attachment in self
            .attachments
            .iter()
            .filter(|attachment| attachment["contextKind"] != "chat_history")
        {
            blocks.push(json!({"type":"text","text":attachments::render(attachment)?}));
        }
        Ok(blocks)
    }

    /// Map ordered blocks to native Codex app-server input, including inline image URLs.
    /// # Errors
    /// Returns invalid rich input before submission.
    pub fn codex_input(&self) -> Result<Vec<Value>, AgentSessionError> {
        self.blocks()?.into_iter().map(|block| {
            Ok(if block["type"] == "image" {
                json!({"type":"image","url":format!("data:{};base64,{}", block["mimeType"].as_str().ok_or(AgentSessionError::Rejected)?, block["data"].as_str().ok_or(AgentSessionError::Rejected)?)})
            } else { json!({"type":"text","text":block["text"],"text_elements":[]}) })
        }).collect()
    }

    /// Map ordered blocks to Claude Code's Anthropic content representation.
    /// # Errors
    /// Returns invalid rich input before submission.
    pub fn claude_content(&self) -> Result<Value, AgentSessionError> {
        if self.images.is_empty() && self.attachments.is_empty() {
            self.validate()?;
            return Ok(json!(self.text));
        }
        Ok(Value::Array(self.blocks()?.into_iter().map(|block| {
            if block["type"] == "image" {
                json!({"type":"image","source":{"type":"base64","media_type":block["mimeType"],"data":block["data"]}})
            } else { block }
        }).collect()))
    }
}

#[cfg(test)]
mod tests;
