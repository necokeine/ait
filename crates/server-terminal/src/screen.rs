use std::collections::VecDeque;

use serde_json::{Value, json};
use vt100::{Color, MouseProtocolEncoding, MouseProtocolMode};

use crate::Error;
use crate::ports::Observation;
use crate::protocol::{MouseAction, Opcode, Restore, RestoreMode, Size};

const OUTPUT_BYTES: usize = 256 * 1024;

#[derive(Default)]
struct Callbacks {
    title: Option<String>,
}

impl vt100::Callbacks for Callbacks {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = Some(
            String::from_utf8_lossy(title)
                .chars()
                .filter(|c| !c.is_control())
                .take(200)
                .collect(),
        );
    }
}

pub(super) struct Screen {
    parser: vt100::Parser<Callbacks>,
    output: VecDeque<(u64, Vec<u8>)>,
    bytes: usize,
    revision: u64,
    floor: u64,
    pub(super) drained: bool,
}

impl Screen {
    pub(super) fn new(size: Size) -> Self {
        let history = (12_000 / usize::from(size.cols))
            .saturating_sub(usize::from(size.rows))
            .min(1000);
        Self {
            parser: vt100::Parser::new_with_callbacks(
                size.rows,
                size.cols,
                history,
                Callbacks::default(),
            ),
            output: VecDeque::new(),
            bytes: 0,
            revision: 0,
            floor: 0,
            drained: false,
        }
    }

    pub(super) fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
        self.revision += 1;
        self.output.push_back((self.revision, bytes.to_vec()));
        self.bytes += bytes.len();
        while self.bytes > OUTPUT_BYTES {
            if let Some((revision, bytes)) = self.output.pop_front() {
                self.bytes -= bytes.len();
                self.floor = revision;
            }
        }
    }

    pub(super) fn resize(&mut self, size: Size) {
        self.parser.screen_mut().set_size(size.rows, size.cols);
        self.revision += 1;
        self.floor = self.revision;
        self.output.clear();
        self.bytes = 0;
    }

    pub(super) fn title(&self) -> Option<String> {
        self.parser.callbacks().title.clone()
    }

    pub(super) fn observe(
        &mut self,
        revision: Option<u64>,
        restore: Option<&Restore>,
    ) -> Result<Observation, Error> {
        let (rows, cols) = self.parser.screen().size();
        let frames = match revision {
            Some(revision) if revision >= self.floor => self
                .output
                .iter()
                .filter(|(seq, _)| *seq > revision)
                .map(|(_, bytes)| (Opcode::Output, bytes.clone()))
                .collect(),
            _ => {
                let live =
                    revision.is_none() && restore.is_some_and(|r| r.mode == RestoreMode::Live);
                let frame = if live {
                    (Opcode::Restore, self.parser.screen().input_mode_formatted())
                } else {
                    let history = restore.map_or(usize::MAX, |r| match r.mode {
                        RestoreMode::Live | RestoreMode::VisibleSnapshot => {
                            r.scrollback_lines.unwrap_or(200).min(500)
                        }
                        RestoreMode::FullSnapshot => usize::MAX,
                    });
                    if restore.is_none() {
                        (
                            Opcode::Snapshot,
                            serde_json::to_vec(&self.snapshot(history)).map_err(|_| Error::Io)?,
                        )
                    } else {
                        (Opcode::Restore, self.ansi(history))
                    }
                };
                vec![frame]
            }
        };
        Ok(Observation {
            revision: self.revision,
            size: Size { rows, cols },
            frames,
            exited: self.drained,
        })
    }

    pub(super) fn capture(&self) -> Vec<String> {
        let mut screen = self.parser.screen().clone();
        let (rows, cols) = screen.size();
        screen.set_scrollback(history_limit(&screen));
        let history = screen.scrollback();
        let mut lines = Vec::with_capacity(history + usize::from(rows));
        for offset in (1..=history).rev() {
            screen.set_scrollback(offset);
            lines.push(
                screen
                    .rows(0, cols)
                    .next()
                    .unwrap_or_default()
                    .trim_end()
                    .to_owned(),
            );
        }
        screen.set_scrollback(0);
        lines.extend(screen.rows(0, cols).map(|line| line.trim_end().to_owned()));
        lines
    }

    fn snapshot(&mut self, limit: usize) -> Value {
        let screen = self.parser.screen_mut();
        let (rows, cols) = screen.size();
        let (row, col) = screen.cursor_position();
        let hidden = screen.hide_cursor();
        screen.set_scrollback(limit.min(history_limit(screen)));
        let history = screen.scrollback();
        let mut scrollback = Vec::with_capacity(history);
        let mut scrollback_wrapped = Vec::with_capacity(history);
        for offset in (1..=history).rev() {
            screen.set_scrollback(offset);
            scrollback.push(row_cells(screen, 0, cols));
            scrollback_wrapped.push(screen.row_wrapped(0));
        }
        screen.set_scrollback(0);
        let grid: Vec<_> = (0..rows).map(|row| row_cells(screen, row, cols)).collect();
        let wrapped: Vec<_> = (0..rows).map(|row| screen.row_wrapped(row)).collect();
        json!({"rows":rows,"cols":cols,"grid":grid,"scrollback":scrollback,
            "gridWrapped":wrapped,"scrollbackWrapped":scrollback_wrapped,
            "cursor":{"row":row,"col":col,"hidden":hidden},"title":self.title().unwrap_or_default()})
    }

    fn ansi(&mut self, limit: usize) -> Vec<u8> {
        if self.parser.screen().alternate_screen() {
            let (rows, cols) = self.parser.screen().size();
            let mut main = Self::new(Size { rows, cols });
            *main.parser.screen_mut() = self.parser.screen().clone();
            main.parser.process(b"\x1b[?1049l");
            let mut output = main.ansi(limit);
            output.extend_from_slice(b"\x1b[?1049h\x1b[0m");
            output.extend(self.parser.screen().state_formatted());
            return output;
        }
        let screen = self.parser.screen_mut();
        let (_, cols) = screen.size();
        // Reset presentation and scrollback before replaying the retained rows.
        let mut output = b"\x1b[?1049l\x1b[0m\x1b[2J\x1b[3J\x1b[H\x1b[?7l".to_vec();
        screen.set_scrollback(limit.min(history_limit(screen)));
        let history = screen.scrollback();
        for offset in (1..=history).rev() {
            screen.set_scrollback(offset);
            if let Some(row) = screen.rows_formatted(0, cols).next() {
                output.extend_from_slice(b"\x1b[0m");
                output.extend(row);
            }
            output.extend_from_slice(b"\r\n");
        }
        screen.set_scrollback(0);
        let rows: Vec<_> = screen.rows_formatted(0, cols).collect();
        for (index, row) in rows.iter().enumerate() {
            output.extend_from_slice(b"\x1b[0m");
            output.extend(row);
            if index + 1 < rows.len() {
                output.extend_from_slice(b"\r\n");
            }
        }
        output.extend_from_slice(b"\x1b[?7h");
        output.extend(screen.cursor_state_formatted());
        output.extend(screen.input_mode_formatted());
        output.extend(screen.attributes_formatted());
        output
    }

    pub(super) fn mouse(
        &self,
        row: u16,
        col: u16,
        button: u8,
        action: MouseAction,
    ) -> Result<Vec<u8>, Error> {
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();
        if row >= rows || col >= cols || !matches!(button, 0..=3 | 64..=65) {
            return Err(Error::Invalid);
        }
        let mode = screen.mouse_protocol_mode();
        let accepted = match (mode, action) {
            (MouseProtocolMode::None, _)
            | (MouseProtocolMode::Press, MouseAction::Up | MouseAction::Move)
            | (MouseProtocolMode::PressRelease, MouseAction::Move) => false,
            (MouseProtocolMode::ButtonMotion, MouseAction::Move) => button != 3,
            _ => true,
        };
        if !accepted {
            return Ok(Vec::new());
        }
        let code = if action == MouseAction::Move {
            button | 32
        } else {
            button
        };
        let suffix = if action == MouseAction::Up { 'm' } else { 'M' };
        match screen.mouse_protocol_encoding() {
            MouseProtocolEncoding::Sgr => {
                Ok(format!("\x1b[<{code};{};{}{suffix}", col + 1, row + 1).into_bytes())
            }
            encoding => {
                let button = if action == MouseAction::Up { 3 } else { code };
                let values = [
                    u32::from(button) + 32,
                    u32::from(col) + 33,
                    u32::from(row) + 33,
                ];
                let mut output = b"\x1b[M".to_vec();
                for value in values {
                    if encoding == MouseProtocolEncoding::Utf8 {
                        let character = char::from_u32(value).ok_or(Error::Invalid)?;
                        output.extend_from_slice(character.encode_utf8(&mut [0; 4]).as_bytes());
                    } else {
                        output.push(u8::try_from(value).map_err(|_| Error::Invalid)?);
                    }
                }
                Ok(output)
            }
        }
    }
}

fn history_limit(screen: &vt100::Screen) -> usize {
    let (rows, cols) = screen.size();
    (12_000 / usize::from(cols))
        .saturating_sub(usize::from(rows))
        .min(1000)
}

fn row_cells(screen: &vt100::Screen, row: u16, cols: u16) -> Vec<Value> {
    (0..cols)
        .map(|col| {
            let Some(cell) = screen.cell(row, col) else {
                return json!({"char":" "});
            };
            let content = if cell.is_wide_continuation() {
                ""
            } else if cell.contents().is_empty() {
                " "
            } else {
                cell.contents()
            };
            let mut value = json!({"char":content});
            for (name, active) in [
                ("bold", cell.bold()),
                ("dim", cell.dim()),
                ("italic", cell.italic()),
                ("underline", cell.underline()),
                ("inverse", cell.inverse()),
            ] {
                if active {
                    value[name] = Value::Bool(true);
                }
            }
            add_color(&mut value, "fg", "fgMode", cell.fgcolor());
            add_color(&mut value, "bg", "bgMode", cell.bgcolor());
            value
        })
        .collect()
}

fn add_color(value: &mut Value, key: &str, mode: &str, color: Color) {
    let (color, encoding) = match color {
        Color::Default => return,
        Color::Idx(index) => (u32::from(index), if index < 16 { 1 } else { 2 }),
        Color::Rgb(red, green, blue) => (
            (u32::from(red) << 16) | (u32::from(green) << 8) | u32::from(blue),
            3,
        ),
    };
    value[key] = json!(color);
    value[mode] = json!(encoding);
}

#[cfg(test)]
mod tests;
