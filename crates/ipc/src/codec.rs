//! Bounded, cancellation-safe framing. Readers are owned by one pump for their lifetime.
use ait_contracts::worker::{
    Envelope, Lease, MAX_FRAME_BYTES, PROTOCOL_MAJOR, PROTOCOL_MINOR, ProtocolError,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// One-direction framing and monotonic sequence validation.
pub struct Reader<R> {
    stream: R,
    maximum: u32,
    sequence: u64,
    negotiated_minor: Option<u16>,
}
impl<R: AsyncRead + Unpin> Reader<R> {
    /// Tighten the bound after protocol negotiation; it can never be enlarged.
    pub fn constrain(&mut self, maximum: u32) {
        self.maximum = self.maximum.min(maximum);
    }
    /// Require the selected minor on every subsequent frame.
    ///
    /// # Errors
    /// Rejects a minor newer than this codec can consume.
    pub fn negotiate(&mut self, protocol_minor: u16) -> Result<(), ProtocolError> {
        if protocol_minor > PROTOCOL_MINOR {
            return Err(ProtocolError::VersionMismatch);
        }
        self.negotiated_minor = Some(protocol_minor);
        Ok(())
    }
    /// Limits are clamped to the compiled ceiling before any allocation.
    pub fn new(stream: R, maximum: u32) -> Self {
        Self {
            stream,
            maximum: maximum.min(MAX_FRAME_BYTES),
            sequence: 0,
            negotiated_minor: None,
        }
    }

    /// Read exactly one frame. Keep this future alive until it finishes.
    ///
    /// # Errors
    /// Returns only stable errors, never JSON input or operating-system diagnostics.
    pub async fn read(&mut self) -> Result<Envelope, ProtocolError> {
        let length = self.stream.read_u32().await.map_err(io_error)?;
        if length == 0 {
            return Err(ProtocolError::InvalidFrame);
        }
        if length > self.maximum {
            return Err(ProtocolError::FrameTooLarge);
        }
        let mut bytes = vec![0; length as usize];
        self.stream.read_exact(&mut bytes).await.map_err(io_error)?;
        let frame: Envelope =
            serde_json::from_slice(&bytes).map_err(|_| ProtocolError::InvalidFrame)?;
        if frame.protocol_major != PROTOCOL_MAJOR
            || self
                .negotiated_minor
                .is_some_and(|minor| frame.protocol_minor != minor)
        {
            return Err(ProtocolError::VersionMismatch);
        }
        if frame.sequence
            != self
                .sequence
                .checked_add(1)
                .ok_or(ProtocolError::SequenceRollback)?
        {
            return Err(ProtocolError::SequenceRollback);
        }
        self.sequence = frame.sequence;
        Ok(frame)
    }
}

/// Serial writer. No other task may write to the same stdout/pipe.
pub struct Writer<W> {
    stream: W,
    maximum: u32,
    sequence: u64,
    protocol_minor: u16,
}
impl<W: AsyncWrite + Unpin> Writer<W> {
    /// Tighten the bound after protocol negotiation; it can never be enlarged.
    pub fn constrain(&mut self, maximum: u32) {
        self.maximum = self.maximum.min(maximum);
    }
    /// Stamp the selected minor on every subsequent frame.
    ///
    /// # Errors
    /// Rejects a minor newer than this codec can emit.
    pub fn negotiate(&mut self, protocol_minor: u16) -> Result<(), ProtocolError> {
        if protocol_minor > PROTOCOL_MINOR {
            return Err(ProtocolError::VersionMismatch);
        }
        self.protocol_minor = protocol_minor;
        Ok(())
    }
    /// Constructs a bounded writer.
    pub fn new(stream: W, maximum: u32) -> Self {
        Self {
            stream,
            maximum: maximum.min(MAX_FRAME_BYTES),
            sequence: 0,
            protocol_minor: PROTOCOL_MINOR,
        }
    }

    /// Writes and flushes one frame; callers impose a deadline and close on failure.
    ///
    /// # Errors
    /// Returns a stable protocol code.
    pub async fn write(
        &mut self,
        lease: Option<Lease>,
        payload: ait_contracts::worker::Payload,
    ) -> Result<(), ProtocolError> {
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or(ProtocolError::SequenceRollback)?;
        let bytes = serde_json::to_vec(&Envelope {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: self.protocol_minor,
            sequence,
            lease,
            payload,
        })
        .map_err(|_| ProtocolError::InvalidFrame)?;
        let length = u32::try_from(bytes.len()).map_err(|_| ProtocolError::FrameTooLarge)?;
        if length > self.maximum {
            return Err(ProtocolError::FrameTooLarge);
        }
        self.stream.write_u32(length).await.map_err(io_error)?;
        self.stream.write_all(&bytes).await.map_err(io_error)?;
        self.stream.flush().await.map_err(io_error)?;
        self.sequence = sequence;
        Ok(())
    }
}
#[allow(
    clippy::needless_pass_by_value,
    reason = "direct map_err callback consumes the underlying diagnostic"
)]
fn io_error(error: std::io::Error) -> ProtocolError {
    if error.kind() == std::io::ErrorKind::UnexpectedEof {
        ProtocolError::UnexpectedEof
    } else {
        ProtocolError::Io
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ait_contracts::worker::Payload;
    #[tokio::test]
    async fn big_endian_round_trip_and_eof() {
        let mut bytes = Vec::new();
        Writer::new(&mut bytes, MAX_FRAME_BYTES)
            .write(None, Payload::Heartbeat)
            .await
            .unwrap();
        assert_eq!(
            u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize,
            bytes.len() - 4
        );
        let mut reader = Reader::new(bytes.as_slice(), MAX_FRAME_BYTES);
        assert!(matches!(
            reader.read().await.unwrap().payload,
            Payload::Heartbeat
        ));
        assert_eq!(
            reader.read().await.unwrap_err(),
            ProtocolError::UnexpectedEof
        );
    }
    #[tokio::test]
    async fn rejects_length_before_allocating_and_never_echoes_pollution() {
        for (bytes, expected) in [
            (
                u32::MAX.to_be_bytes().to_vec(),
                ProtocolError::FrameTooLarge,
            ),
            (vec![0, 0, 0, 0], ProtocolError::InvalidFrame),
            (
                b"secret stdout pollution".to_vec(),
                ProtocolError::FrameTooLarge,
            ),
            (b"\0\0\0\x04oops".to_vec(), ProtocolError::InvalidFrame),
            (b"\0\0\0\x04x".to_vec(), ProtocolError::UnexpectedEof),
        ] {
            assert_eq!(
                Reader::new(bytes.as_slice(), MAX_FRAME_BYTES)
                    .read()
                    .await
                    .unwrap_err(),
                expected
            );
        }
    }
    #[tokio::test]
    async fn replayed_sequence_is_rejected() {
        let mut bytes = Vec::new();
        Writer::new(&mut bytes, MAX_FRAME_BYTES)
            .write(None, Payload::Heartbeat)
            .await
            .unwrap();
        bytes.extend(bytes.clone());
        let mut reader = Reader::new(bytes.as_slice(), MAX_FRAME_BYTES);
        reader.read().await.unwrap();
        assert_eq!(
            reader.read().await.unwrap_err(),
            ProtocolError::SequenceRollback
        );
    }

    #[tokio::test]
    async fn negotiated_frames_reject_major_and_minor_conflicts() {
        fn framed(protocol_major: u16, protocol_minor: u16) -> Vec<u8> {
            let body = serde_json::to_vec(&Envelope {
                protocol_major,
                protocol_minor,
                sequence: 1,
                lease: None,
                payload: Payload::Heartbeat,
            })
            .unwrap();
            let mut bytes = Vec::with_capacity(body.len() + 4);
            bytes.extend_from_slice(&u32::try_from(body.len()).unwrap().to_be_bytes());
            bytes.extend_from_slice(&body);
            bytes
        }

        let incompatible_major = framed(PROTOCOL_MAJOR + 1, PROTOCOL_MINOR);
        let mut reader = Reader::new(incompatible_major.as_slice(), MAX_FRAME_BYTES);
        assert_eq!(
            reader.read().await.unwrap_err(),
            ProtocolError::VersionMismatch
        );

        let future = framed(PROTOCOL_MAJOR, PROTOCOL_MINOR + 1);
        let mut reader = Reader::new(future.as_slice(), MAX_FRAME_BYTES);
        reader.negotiate(PROTOCOL_MINOR).unwrap();
        assert_eq!(
            reader.read().await.unwrap_err(),
            ProtocolError::VersionMismatch
        );
    }
}
