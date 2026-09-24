use super::*;

const PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x00,
];

#[test]
fn stores_reads_and_removes_validated_custom_icons() {
    let fixture = tempfile::tempdir().unwrap();
    let store = LocalProjectIconStore::new(fixture.path().join("icons"));
    store.write_custom("prj_a", PNG_1X1).unwrap();
    let icon = store.read_custom("prj_a").unwrap().unwrap();
    assert_eq!(icon.bytes, PNG_1X1);
    assert_eq!(icon.mime_type, "image/png");
    store.remove_custom("prj_a").unwrap();
    assert!(store.read_custom("prj_a").unwrap().is_none());
}

#[test]
fn rejects_unsupported_non_square_and_oversized_custom_icons() {
    let fixture = tempfile::tempdir().unwrap();
    let store = LocalProjectIconStore::new(fixture.path().join("icons"));
    assert_eq!(
        store.write_custom("prj_a", b"not an image"),
        Err(ProjectIconStoreError::Invalid)
    );
    let mut wide = PNG_1X1.to_vec();
    wide[19] = 2;
    assert_eq!(
        store.write_custom("prj_a", &wide),
        Err(ProjectIconStoreError::Invalid)
    );
    assert_eq!(
        store.write_custom("prj_a", &vec![0; MAX_CUSTOM_BYTES + 1]),
        Err(ProjectIconStoreError::Invalid)
    );
}

#[test]
fn automatic_discovery_prefers_priority_directories_and_accepts_svg() {
    let fixture = tempfile::tempdir().unwrap();
    let public = fixture.path().join("public");
    std::fs::create_dir(&public).unwrap();
    std::fs::write(fixture.path().join("favicon.png"), PNG_1X1).unwrap();
    std::fs::write(
        public.join("favicon.svg"),
        "<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>",
    )
    .unwrap();
    let store = LocalProjectIconStore::new(fixture.path().join("icons"));
    let icon = store
        .find_automatic(fixture.path().to_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(icon.mime_type, "image/svg+xml");
}
