//! Bounded audio decoding and PCM/WAV conversion without native bindings.

use base64::{Engine, engine::general_purpose::STANDARD};

use crate::Error;

/// Maximum buffered audio for a single utterance or dictation.
pub const MAX_AUDIO_BYTES: usize = 16 * 1024 * 1024;
/// Maximum decoded client audio chunk.
pub const MAX_CHUNK_BYTES: usize = 512 * 1024;
/// Maximum transcript or generated speech text in bytes.
pub const MAX_TEXT_BYTES: usize = 64 * 1024;

/// Supported input representations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Signed 16-bit little-endian, mono samples at the given rate.
    Pcm(u32),
    /// A complete mono PCM16 WAV file.
    Wav,
}

impl Format {
    /// Parse the supported MIME spelling and validate the PCM parameters.
    /// # Errors
    /// Rejects unsupported formats, rates, channel counts and malformed parameters.
    pub fn parse(value: &str) -> Result<Self, Error> {
        if matches!(value, "audio/wav" | "audio/x-wav" | "wav") {
            return Ok(Self::Wav);
        }
        let mut parts = value.split(';').map(str::trim);
        if !matches!(parts.next(), Some("audio/pcm" | "pcm")) {
            return Err(Error::Invalid);
        }
        let mut rate = None;
        let mut bits = None;
        let mut channels = None;
        for part in parts {
            let (key, value) = part.split_once('=').ok_or(Error::Invalid)?;
            let slot = match key {
                "rate" => &mut rate,
                "bits" => &mut bits,
                "channels" => &mut channels,
                _ => return Err(Error::Invalid),
            };
            if slot
                .replace(value.parse::<u32>().map_err(|_| Error::Invalid)?)
                .is_some()
            {
                return Err(Error::Invalid);
            }
        }
        let rate = rate.unwrap_or(24_000);
        if !matches!(rate, 8_000 | 16_000 | 22_050 | 24_000 | 44_100 | 48_000)
            || bits.is_some_and(|bits| bits != 16)
            || channels.is_some_and(|channels| channels != 1)
        {
            return Err(Error::Invalid);
        }
        Ok(Self::Pcm(rate))
    }

    /// Canonical MIME type, including rate and sample representation.
    #[must_use]
    pub fn mime(self) -> String {
        match self {
            Self::Pcm(rate) => format!("audio/pcm;rate={rate};bits=16;channels=1"),
            Self::Wav => "audio/wav".to_owned(),
        }
    }
}

/// Owned bounded audio passed to a speech adapter.
#[derive(Debug, Clone)]
pub struct Audio {
    /// Encoded audio or PCM bytes.
    pub bytes: Vec<u8>,
    /// Representation of the bytes.
    pub format: Format,
}

impl Audio {
    /// Decode complete audio into mono PCM16 samples.
    /// # Errors
    /// Rejects empty/oversized data, invalid alignment or unsupported WAV encodings.
    pub fn pcm(self) -> Result<Self, Error> {
        if self.bytes.is_empty() || self.bytes.len() > MAX_AUDIO_BYTES {
            return Err(Error::Invalid);
        }
        match self.format {
            Format::Pcm(_) if self.bytes.len().is_multiple_of(2) => Ok(self),
            Format::Pcm(_) => Err(Error::Invalid),
            Format::Wav => decode_wav(&self.bytes),
        }
    }

    /// Encode audio as a WAV file accepted by speech providers.
    /// # Errors
    /// Returns an error for invalid audio.
    pub fn wav(self) -> Result<Vec<u8>, Error> {
        let pcm = self.pcm()?;
        let Format::Pcm(rate) = pcm.format else {
            return Err(Error::Invalid);
        };
        let size = u32::try_from(pcm.bytes.len()).map_err(|_| Error::Invalid)?;
        let mut wav = Vec::with_capacity(44 + pcm.bytes.len());
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + size).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&rate.to_le_bytes());
        wav.extend_from_slice(&(rate * 2).to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&size.to_le_bytes());
        wav.extend_from_slice(&pcm.bytes);
        Ok(wav)
    }
}

pub(crate) fn decode_chunk(value: &str) -> Result<Vec<u8>, Error> {
    if value.len() > MAX_CHUNK_BYTES.div_ceil(3) * 4 {
        return Err(Error::Capacity);
    }
    let bytes = STANDARD.decode(value).map_err(|_| Error::Invalid)?;
    if bytes.is_empty() || bytes.len() > MAX_CHUNK_BYTES {
        return Err(Error::Invalid);
    }
    Ok(bytes)
}

fn decode_wav(bytes: &[u8]) -> Result<Audio, Error> {
    if bytes.len() < 44 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(Error::Invalid);
    }
    let read = |offset| {
        bytes
            .get(offset..offset + 4)
            .and_then(|value| value.try_into().ok())
            .map(u32::from_le_bytes)
            .ok_or(Error::Invalid)
    };
    let end = usize::try_from(read(4)?)
        .map_err(|_| Error::Invalid)?
        .checked_add(8)
        .ok_or(Error::Invalid)?;
    if end != bytes.len() {
        return Err(Error::Invalid);
    }
    let mut offset = 12;
    let mut format = None;
    let mut samples = None;
    while offset + 8 <= end {
        let size = usize::try_from(read(offset + 4)?).map_err(|_| Error::Invalid)?;
        let start = offset + 8;
        let next = start.checked_add(size).ok_or(Error::Invalid)?;
        let chunk = bytes.get(start..next).ok_or(Error::Invalid)?;
        match &bytes[offset..offset + 4] {
            b"fmt " => {
                if format.is_some()
                    || size < 16
                    || chunk[..4] != [1, 0, 1, 0]
                    || chunk[12..16] != [2, 0, 16, 0]
                {
                    return Err(Error::Invalid);
                }
                let rate = read(start + 4)?;
                if read(start + 8)? != rate.checked_mul(2).ok_or(Error::Invalid)? {
                    return Err(Error::Invalid);
                }
                format = Some(Format::parse(&format!("audio/pcm;rate={rate}"))?);
            }
            b"data" if samples.replace(chunk).is_some() => return Err(Error::Invalid),
            _ => {}
        }
        offset = next.checked_add(size % 2).ok_or(Error::Invalid)?;
    }
    let bytes = samples.ok_or(Error::Invalid)?;
    if offset != end || bytes.is_empty() || !bytes.len().is_multiple_of(2) {
        return Err(Error::Invalid);
    }
    Ok(Audio {
        bytes: bytes.to_vec(),
        format: format.ok_or(Error::Invalid)?,
    })
}

#[cfg(test)]
mod tests;
