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
