use super::*;
use std::io::Read;

#[derive(Debug)]
struct MemoryFiles;

#[derive(Debug)]
struct Reader {
    info: FileInfo,
    bytes: std::io::Cursor<Vec<u8>>,
}

impl Read for Reader {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.bytes.read(bytes)
    }
}
impl FileReader for Reader {
    fn info(&self) -> &FileInfo {
        &self.info
    }
    fn verify(&self) -> Result<(), FileError> {
        Ok(())
    }
}

impl FileSystem for MemoryFiles {
    fn list(&self, _: &str, _: &str) -> Result<(String, Vec<FileEntry>), FileError> {
        Err(FileError("unused".to_owned()))
    }
    fn open(&self, _: &str, path: &str) -> Result<Box<dyn FileReader>, FileError> {
        if path == "missing" {
            return Err(FileError("missing".to_owned()));
        }
        Ok(Box::new(Reader {
            info: FileInfo {
                root: "/repo".to_owned(),
                absolute_path: format!("/repo/{}", path.strip_prefix("/repo/").unwrap_or(path)),
                path: path.to_owned(),
                file_name: "a".to_owned(),
                mime_type: "text/plain".to_owned(),
                kind: FileKind::Text,
                size: 3,
                modified_at: "now".to_owned(),
                revision: "v1".to_owned(),
            },
            bytes: std::io::Cursor::new(b"abc".to_vec()),
        }))
    }
    fn version(&self, _: &str, _: &str) -> FileVersion {
        FileVersion::Missing
    }
    fn write(&self, _: &FileWrite) -> Result<FileWritten, FileError> {
        Err(FileError("unused".to_owned()))
    }
    fn create(&self, _: &str, _: &str, _: &str, _: EntryKind) -> Result<String, FileError> {
        Err(FileError("unused".to_owned()))
    }
    fn rename(&self, _: &str, _: &str, _: &str) -> Result<String, FileError> {
        Err(FileError("unused".to_owned()))
    }
    fn duplicate(&self, _: &str, _: &str) -> Result<String, FileError> {
        Err(FileError("unused".to_owned()))
    }
    fn delete(&self, _: &str, _: &str) -> Result<(), FileError> {
        Err(FileError("unused".to_owned()))
    }
    fn search(&self, _: &FileSearch) -> Result<Vec<(String, EntryKind)>, FileError> {
        Err(FileError("unused".to_owned()))
    }
    fn upload(&self, _: UploadedFile) -> Result<Box<dyn FileUpload>, FileError> {
        Err(FileError("unused".to_owned()))
    }
}

#[test]
fn tokens_are_single_use_and_expiration_is_checked_before_opening() {
    let mut files = Files::new(Box::new(MemoryFiles));
    files
        .issue_download("token".to_owned(), "/repo", "a")
        .unwrap();
    assert_eq!(files.consume_download("token").unwrap().0.info().size, 3);
    assert!(files.consume_download("token").is_err());
    files.downloads.insert(
        "expired".to_owned(),
        DownloadGrant {
            expires: Instant::now().checked_sub(Duration::from_secs(1)).unwrap(),
            root: "/repo".to_owned(),
            path: "a".to_owned(),
            file_name: "a".to_owned(),
        },
    );
    assert!(files.consume_download("expired").is_err());
    assert!(files.downloads.is_empty());
}

#[test]
fn issuing_rejects_missing_paths_reused_ids_and_capacity_overflow() {
    let mut files = Files::new(Box::new(MemoryFiles));
    assert!(
        files
            .issue_download("bad".to_owned(), "root", "missing")
            .is_err()
    );
    assert!(files.downloads.is_empty());
    for index in 0..256 {
        files
            .issue_download(index.to_string(), "root", "a")
            .unwrap();
    }
    assert!(files.issue_download("256".to_owned(), "root", "a").is_err());
    assert!(files.issue_download("0".to_owned(), "root", "a").is_err());
}
