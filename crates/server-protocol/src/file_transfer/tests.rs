use super::*;

#[test]
fn framing_matches_paseo_layout_and_round_trips() {
    let begin = FileFrame::Begin(FileBegin {
        mime: "text/plain".to_owned(),
        size: 3,
        encoding: "utf-8".to_owned(),
        modified_at: "now".to_owned(),
        revision: Some("v1".to_owned()),
        file_name: None,
    });
    for frame in [begin, FileFrame::Chunk(b"abc".to_vec()), FileFrame::End] {
        let bytes = encode("r1", &frame).unwrap();
        assert_eq!(decode(&bytes), Some(("r1".to_owned(), frame)));
    }
    assert_eq!(
        encode("r", &FileFrame::Chunk(vec![1, 2])).unwrap(),
        [0x11, 1, b'r', 1, 2]
    );
    assert_eq!(encode("r", &FileFrame::End).unwrap(), [0x12, 1, b'r']);
}

#[test]
fn rejects_malformed_lengths_metadata_opcodes_and_oversized_chunks() {
    for bytes in [
        vec![],
        vec![0x10],
        vec![0x11, 0],
        vec![0x12, 1, b'r', 0],
        vec![0x10, 1, b'r', 0, 2, b'{'],
        vec![0x13, 1, b'r'],
    ] {
        assert!(decode(&bytes).is_none());
    }
    assert!(encode("", &FileFrame::End).is_err());
    assert!(encode("r", &FileFrame::Chunk(vec![0; CHUNK_BYTES + 1])).is_err());
}
