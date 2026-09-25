use super::*;
use crate::ports::files::{FileInfo, FileKind};

#[derive(Debug)]
struct Reader {
    data: std::io::Cursor<Vec<u8>>,
    info: FileInfo,
    changed: bool,
}
impl Read for Reader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.data.read(buffer)
    }
}
impl FileReader for Reader {
    fn info(&self) -> &FileInfo {
        &self.info
    }
    fn verify(&self) -> Result<(), FileError> {
        if self.changed {
            Err(FileError("File changed during transfer".to_owned()))
        } else {
            Ok(())
        }
    }
}
fn reader(size: usize, changed: bool) -> Box<dyn FileReader> {
    Box::new(Reader {
        data: std::io::Cursor::new(vec![7; size]),
        changed,
        info: FileInfo {
            root: "/repo".to_owned(),
            absolute_path: "/repo/a".to_owned(),
            path: "a".to_owned(),
            file_name: "a".to_owned(),
            mime_type: "text/plain".to_owned(),
            kind: FileKind::Text,
            size: size as u64,
            modified_at: "now".to_owned(),
            revision: "1".to_owned(),
        },
    })
}
#[test]
fn chunks_are_bounded_and_end_verifies_the_original_revision() {
    let cursor = Cursor::new(reader(file_transfer::CHUNK_BYTES + 1, false));
    let (cursor, chunk) = cursor.read_chunk().unwrap();
    assert_eq!(chunk.unwrap().len(), file_transfer::CHUNK_BYTES);
    let (cursor, chunk) = cursor.read_chunk().unwrap();
    assert_eq!(chunk.unwrap(), [7]);
    assert!(cursor.read_chunk().unwrap().1.is_none());
    assert!(Cursor::new(reader(0, true)).read_chunk().is_err());
}
