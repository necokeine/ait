//! Bounded synchronous file reads shared by WebSocket and HTTP delivery.
use std::io::Read;

use crate::ports::files::{FileError, FileReader};
use crate::protocol::file_transfer;
/// Reader pinned to the file's advertised size and revision.
#[derive(Debug)]
pub struct Cursor {
    reader: Box<dyn FileReader>,
    remaining: u64,
}
impl Cursor {
    /// Start streaming the already-opened file at its advertised size.
    #[must_use]
    pub fn new(reader: Box<dyn FileReader>) -> Self {
        let remaining = reader.info().size;
        Self { reader, remaining }
    }
    /// Read one bounded chunk, verifying the original revision at end of stream.
    ///
    /// # Errors
    /// Returns read errors or a changed-file error when final verification fails.
    pub fn read_chunk(mut self) -> Result<(Self, Option<Vec<u8>>), FileError> {
        if self.remaining == 0 {
            self.reader.verify()?;
            return Ok((self, None));
        }
        let mut bytes = vec![
            0;
            usize::try_from(self.remaining)
                .unwrap_or(usize::MAX)
                .min(file_transfer::CHUNK_BYTES)
        ];
        self.reader.read_exact(&mut bytes)?;
        self.remaining -= bytes.len() as u64;
        Ok((self, Some(bytes)))
    }
}

#[cfg(test)]
mod tests;
