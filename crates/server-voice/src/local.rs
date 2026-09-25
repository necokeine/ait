//! Local whisper.cpp/Piper process adapters with private temporary files and child reaping.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};
use tokio_util::sync::CancellationToken;

use crate::{
    Error,
    audio::{Audio, Format, MAX_AUDIO_BYTES, MAX_TEXT_BYTES},
    ports::{Operation, Synthesizer, Transcriber, Transcript},
};

/// whisper.cpp CLI and its already-installed model.
#[derive(Debug, Clone)]
pub struct Whisper {
    binary: PathBuf,
    model: PathBuf,
}

impl Whisper {
    /// Use an executable path/name and a local ggml model; no download is performed.
    /// # Errors
    /// Rejects missing model files or an empty executable name.
    pub fn new(binary: PathBuf, model: PathBuf) -> Result<Self, Error> {
        validate(&binary, &model)?;
        Ok(Self { binary, model })
    }
}

impl Transcriber for Whisper {
    fn transcribe(&self, audio: Audio, cancel: CancellationToken) -> Operation<'_, Transcript> {
        Box::pin(async move {
            let directory = tempfile::tempdir().map_err(|_| Error::Provider)?;
            let input = directory.path().join("input.wav");
            let output = directory.path().join("transcript");
            let wav = tokio::task::spawn_blocking(move || wav_16khz(audio))
                .await
                .map_err(|_| Error::Provider)??;
            tokio::fs::write(&input, wav)
                .await
                .map_err(|_| Error::Provider)?;
            let mut command = Command::new(&self.binary);
            command
                .arg("--model")
                .arg(&self.model)
                .arg("--file")
                .arg(&input)
                .args(["--language", "auto", "--output-txt", "--output-file"])
                .arg(&output)
                .args(["--no-prints", "--no-timestamps"]);
            run(command, None, cancel).await?;
            let bytes = read_bounded(&output.with_extension("txt"), MAX_TEXT_BYTES).await?;
            let text = String::from_utf8(bytes).map_err(|_| Error::Provider)?;
            Ok(Transcript {
                text: text.trim().to_owned(),
                language: None,
            })
        })
    }
}

/// Piper CLI and its already-installed ONNX voice (with adjacent JSON configuration).
#[derive(Debug, Clone)]
pub struct Piper {
    binary: PathBuf,
    model: PathBuf,
}

impl Piper {
    /// Use an executable path/name and a local voice model; no download is performed.
    /// # Errors
    /// Rejects missing model files or an empty executable name.
    pub fn new(binary: PathBuf, model: PathBuf) -> Result<Self, Error> {
        validate(&binary, &model)?;
        Ok(Self { binary, model })
    }
}

impl Synthesizer for Piper {
    fn synthesize<'a>(&'a self, text: &'a str, cancel: CancellationToken) -> Operation<'a, Audio> {
        Box::pin(async move {
            if text.trim().is_empty() || text.len() > MAX_TEXT_BYTES {
                return Err(Error::Invalid);
            }
            let directory = tempfile::tempdir().map_err(|_| Error::Provider)?;
            let output = directory.path().join("speech.wav");
            let mut command = Command::new(&self.binary);
            command
                .arg("--model")
                .arg(&self.model)
                .arg("--output_file")
                .arg(&output);
            let input = text.split_whitespace().collect::<Vec<_>>().join(" ");
            run(command, Some(&input), cancel).await?;
            let bytes = read_bounded(&output, MAX_AUDIO_BYTES).await?;
            tokio::task::spawn_blocking(move || {
                Audio {
                    bytes,
                    format: Format::Wav,
                }
                .pcm()
            })
            .await
            .map_err(|_| Error::Provider)?
        })
    }
}

fn validate(binary: &Path, model: &Path) -> Result<(), Error> {
    if binary.as_os_str().is_empty() || !model.is_file() {
        return Err(Error::Unavailable);
    }
    Ok(())
}

async fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, Error> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| Error::Provider)?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| Error::Provider)?;
    if bytes.len() > limit {
        return Err(Error::Capacity);
    }
    Ok(bytes)
}

async fn run(
    mut command: Command,
    input: Option<&str>,
    cancel: CancellationToken,
) -> Result<(), Error> {
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    command
        .kill_on_drop(true)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn().map_err(|_| Error::Unavailable)?;
    let result = tokio::select! { biased;
        () = cancel.cancelled() => Err(Error::Cancelled),
        () = tokio::time::sleep(Duration::from_secs(120)) => Err(Error::Timeout),
        result = async {
            if let Some(text) = input {
                let mut stdin = child.stdin.take().ok_or(Error::Provider)?;
                stdin.write_all(text.as_bytes()).await.map_err(|_| Error::Provider)?;
                stdin.write_all(b"\n").await.map_err(|_| Error::Provider)?;
                stdin.shutdown().await.map_err(|_| Error::Provider)?;
            }
            let status = child.wait().await.map_err(|_| Error::Provider)?;
            if status.success() { Ok(()) } else { Err(Error::Provider) }
        } => result,
    };
    if result.is_err() {
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
    result
}

fn wav_16khz(audio: Audio) -> Result<Vec<u8>, Error> {
    let audio = audio.pcm()?;
    let Format::Pcm(rate) = audio.format else {
        return Err(Error::Invalid);
    };
    if rate == 16_000 {
        return audio.wav();
    }
    // Linear interpolation is deterministic, bounded and sufficient for the CLI's input rate.
    let count = audio.bytes.len() / 2;
    let output_count = count * 16_000 / rate as usize;
    let mut bytes = Vec::with_capacity(output_count * 2);
    for index in 0..output_count {
        let scaled = index * rate as usize;
        let left = (scaled / 16_000).min(count - 1);
        let right = (left + 1).min(count - 1);
        let sample = |index: usize| {
            i64::from(i16::from_le_bytes([
                audio.bytes[index * 2],
                audio.bytes[index * 2 + 1],
            ]))
        };
        let fraction = i64::try_from(scaled % 16_000).map_err(|_| Error::Invalid)?;
        let value = (sample(left) * (16_000 - fraction) + sample(right) * fraction) / 16_000;
        bytes.extend_from_slice(
            &i16::try_from(value)
                .map_err(|_| Error::Invalid)?
                .to_le_bytes(),
        );
    }
    Audio {
        bytes,
        format: Format::Pcm(16_000),
    }
    .wav()
}

#[cfg(test)]
mod tests;
