use std::collections::VecDeque;
use std::io::{self, ErrorKind};

use super::*;

struct Reader(VecDeque<io::Result<Vec<u8>>>);

impl Read for Reader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let bytes = self.0.pop_front().unwrap_or_else(|| Ok(Vec::new()))?;
        output[..bytes.len()].copy_from_slice(&bytes);
        Ok(bytes.len())
    }
}

#[derive(Default)]
struct Written {
    bytes: Vec<u8>,
    flushes: usize,
}

struct Writer {
    state: Arc<Mutex<Written>>,
    write_limit: usize,
    flush_error: bool,
}

impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let length = self.write_limit.min(bytes.len());
        self.state
            .lock()
            .unwrap()
            .bytes
            .extend_from_slice(&bytes[..length]);
        Ok(length)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.state.lock().unwrap().flushes += 1;
        if self.flush_error {
            Err(ErrorKind::BrokenPipe.into())
        } else {
            Ok(())
        }
    }
}

#[test]
fn interrupted_pty_reads_preserve_split_utf8_and_drain_after_eof() {
    let bytes = "中🦀".as_bytes();
    let reader = Reader(VecDeque::from([
        Ok(bytes[..2].to_vec()),
        Err(ErrorKind::Interrupted.into()),
        Ok(bytes[2..].to_vec()),
        Ok(Vec::new()),
    ]));
    let screen = Mutex::new(Screen::new(Size::default()));
    read_output(Box::new(reader), &screen);
    let mut screen = screen.lock().unwrap();
    assert!(screen.drained);
    assert_eq!(screen.capture()[0], "中🦀");
    let observed = screen.observe(Some(0), None).unwrap();
    let replay: Vec<_> = observed
        .frames
        .into_iter()
        .flat_map(|(_, bytes)| bytes)
        .collect();
    assert_eq!(replay, bytes);
}

#[test]
fn terminal_read_error_preserves_preceding_output_and_marks_the_reader_drained() {
    let reader = Reader(VecDeque::from([
        Ok(b"before failure".to_vec()),
        Err(ErrorKind::BrokenPipe.into()),
        Ok(b"must not read".to_vec()),
    ]));
    let screen = Mutex::new(Screen::new(Size::default()));
    read_output(Box::new(reader), &screen);
    let screen = screen.lock().unwrap();
    assert!(screen.drained);
    assert_eq!(screen.capture()[0], "before failure");
}

fn write_chunks(limit: usize, flush_error: bool) -> Arc<Mutex<Written>> {
    let state = Arc::new(Mutex::new(Written::default()));
    let writer = Writer {
        state: state.clone(),
        write_limit: limit,
        flush_error,
    };
    let (sender, receiver) = mpsc::channel();
    sender.send("中🦀".as_bytes().to_vec()).unwrap();
    sender.send(b"next".to_vec()).unwrap();
    drop(sender);
    write_input(Box::new(writer), &receiver);
    state
}

#[test]
fn short_pty_writes_are_completed_in_order_before_flushing_each_input_chunk() {
    let state = write_chunks(2, false);
    let state = state.lock().unwrap();
    assert_eq!(state.bytes, "中🦀next".as_bytes());
    assert_eq!(state.flushes, 2);
}

#[test]
fn writer_stops_after_flush_failure_instead_of_sending_later_input() {
    let state = write_chunks(2, true);
    let state = state.lock().unwrap();
    assert_eq!(state.bytes, "中🦀".as_bytes());
    assert_eq!(state.flushes, 1);
}

#[test]
fn zero_byte_write_stops_without_spinning_or_flushing_unwritten_input() {
    let state = write_chunks(0, false);
    let state = state.lock().unwrap();
    assert!(state.bytes.is_empty());
    assert_eq!(state.flushes, 0);
}
