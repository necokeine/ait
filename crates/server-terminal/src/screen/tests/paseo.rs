//! Restore and output-order contracts from Paseo terminal-restore/session-controller tests.

use super::*;

fn restore_into(source: &mut Screen, options: &Restore) -> Screen {
    let observed = source.observe(None, Some(options)).unwrap();
    assert_eq!(observed.frames.len(), 1);
    assert_eq!(observed.frames[0].0, Opcode::Restore);
    let mut restored = Screen::new(observed.size);
    restored.process(&observed.frames[0].1);
    restored
}

fn with_history(lines: usize) -> Screen {
    let mut result = screen();
    for index in 0..lines {
        result.process(format!("line{index:04}\r\n").as_bytes());
    }
    result
}

#[test]
fn live_attach_starts_after_old_output_but_preserves_input_modes() {
    let mut original = screen();
    original.process(b"old output\x1b[?2004h\x1b[?1000h\x1b[?1006h");
    let live = restore_into(&mut original, &restore(RestoreMode::Live));
    assert_eq!(live.capture(), ["", "", ""]);
    assert!(live.parser.screen().bracketed_paste());
    assert_eq!(
        live.mouse(1, 2, 0, MouseAction::Down).unwrap(),
        b"\x1b[<0;3;2M"
    );
}

#[test]
fn visible_restore_with_zero_history_keeps_only_visible_rows() {
    let mut original = with_history(20);
    let options = Restore {
        mode: RestoreMode::VisibleSnapshot,
        scrollback_lines: Some(0),
        size: None,
    };
    let restored = restore_into(&mut original, &options);
    assert_eq!(restored.capture(), ["line0018", "line0019", ""]);
}

#[test]
fn visible_restore_uses_two_hundred_history_rows_by_default() {
    let mut original = with_history(700);
    let options = Restore {
        mode: RestoreMode::VisibleSnapshot,
        scrollback_lines: None,
        size: None,
    };
    let restored = restore_into(&mut original, &options);
    assert_eq!(restored.capture().len(), 203);
    assert_eq!(restored.capture()[0], "line0498");
}

#[test]
fn visible_restore_clamps_requested_history_to_five_hundred_rows() {
    let mut original = with_history(700);
    let options = Restore {
        mode: RestoreMode::VisibleSnapshot,
        scrollback_lines: Some(usize::MAX),
        size: None,
    };
    let restored = restore_into(&mut original, &options);
    assert_eq!(restored.capture().len(), 503);
    assert_eq!(restored.capture()[0], "line0198");
}

#[test]
fn full_restore_ignores_visible_history_limit_and_keeps_all_retained_rows() {
    let mut original = with_history(700);
    let options = Restore {
        mode: RestoreMode::FullSnapshot,
        scrollback_lines: Some(0),
        size: None,
    };
    let restored = restore_into(&mut original, &options);
    assert_eq!(restored.capture(), original.capture());
    assert_eq!(restored.capture()[0], "line0000");
}

#[test]
fn independent_observers_receive_the_same_bytes_from_their_own_cursors() {
    let mut original = screen();
    let first = original.observe(None, None).unwrap();
    original.process(b"first");
    let second = original.observe(None, None).unwrap();
    original.process(b" second");
    let first_delta = original.observe(Some(first.revision), None).unwrap();
    let second_delta = original.observe(Some(second.revision), None).unwrap();
    assert_eq!(
        first_delta.frames,
        [
            (Opcode::Output, b"first".to_vec()),
            (Opcode::Output, b" second".to_vec())
        ]
    );
    assert_eq!(second_delta.frames, [(Opcode::Output, b" second".to_vec())]);
    assert_eq!(first_delta.revision, second_delta.revision);
}

#[test]
fn restore_revision_fences_later_output_without_duplication() {
    let mut original = screen();
    original.process(b"before");
    let initial = original
        .observe(None, Some(&restore(RestoreMode::FullSnapshot)))
        .unwrap();
    original.process(b" after");
    let delta = original.observe(Some(initial.revision), None).unwrap();
    let mut restored = Screen::new(initial.size);
    restored.process(&initial.frames[0].1);
    for (_, bytes) in &delta.frames {
        restored.process(bytes);
    }
    assert_eq!(restored.capture(), original.capture());
    assert_eq!(delta.frames, [(Opcode::Output, b" after".to_vec())]);
    assert!(
        original
            .observe(Some(delta.revision), None)
            .unwrap()
            .frames
            .is_empty()
    );
}

#[test]
fn split_utf8_output_is_replayed_in_byte_order_without_lossy_decoding() {
    let mut original = screen();
    let cursor = original.observe(None, None).unwrap().revision;
    let text = "🦀中文".as_bytes();
    for bytes in text.chunks(2) {
        original.process(bytes);
    }
    let output = original.observe(Some(cursor), None).unwrap();
    let replay: Vec<_> = output
        .frames
        .into_iter()
        .flat_map(|(_, bytes)| bytes)
        .collect();
    assert_eq!(replay, text);
    assert_eq!(original.capture()[0], "🦀中文");
}

#[test]
fn slow_live_observer_receives_visible_restore_after_output_overflow() {
    let mut original = screen();
    let live = Restore {
        mode: RestoreMode::Live,
        scrollback_lines: Some(0),
        size: None,
    };
    let cursor = original.observe(None, Some(&live)).unwrap().revision;
    for _ in 0..17 {
        original.process(&vec![b'x'; 16 * 1024]);
    }
    original.process(b"\r\nfinal");
    let observed = original.observe(Some(cursor), Some(&live)).unwrap();
    assert_eq!(observed.frames[0].0, Opcode::Restore);
    let mut restored = Screen::new(observed.size);
    restored.process(&observed.frames[0].1);
    assert!(restored.capture().join("\n").contains("final"));
    assert_eq!(restored.capture().len(), 3);
    assert!(
        original
            .observe(Some(observed.revision), Some(&live))
            .unwrap()
            .frames
            .is_empty()
    );
}

#[test]
fn cursor_at_retained_output_boundary_receives_deltas_without_snapshot() {
    let mut original = screen();
    original.process(&vec![b'x'; OUTPUT_BYTES]);
    let cursor = original.observe(None, None).unwrap().revision;
    original.process(b"tail");
    let delta = original.observe(Some(cursor), None).unwrap();
    assert_eq!(delta.frames, [(Opcode::Output, b"tail".to_vec())]);
}

#[test]
fn resize_invalidates_old_cursors_and_includes_new_output_in_one_snapshot() {
    let mut original = screen();
    original.process(b"before");
    let cursor = original.observe(None, None).unwrap().revision;
    original.resize(Size { rows: 4, cols: 20 });
    original.process(b" after");
    let observed = original.observe(Some(cursor), None).unwrap();
    assert_eq!(observed.size, Size { rows: 4, cols: 20 });
    assert_eq!(observed.frames.len(), 1);
    assert_eq!(observed.frames[0].0, Opcode::Snapshot);
    let snapshot: Value = serde_json::from_slice(&observed.frames[0].1).unwrap();
    assert_eq!(snapshot["rows"], 4);
    assert_eq!(snapshot["cursor"]["col"], 12);
    assert!(
        original
            .observe(Some(observed.revision), None)
            .unwrap()
            .frames
            .is_empty()
    );
}

#[test]
fn final_output_is_returned_together_with_the_drained_exit_flag() {
    let mut original = screen();
    let cursor = original.observe(None, None).unwrap().revision;
    original.process(b"last bytes");
    original.drained = true;
    let observed = original.observe(Some(cursor), None).unwrap();
    assert!(observed.exited);
    assert_eq!(observed.frames, [(Opcode::Output, b"last bytes".to_vec())]);
    assert!(
        original
            .observe(Some(observed.revision), None)
            .unwrap()
            .frames
            .is_empty()
    );
}

#[test]
fn ansi_restore_preserves_cursor_modes_and_attributes_for_subsequent_output() {
    let mut original = screen();
    original.process(b"\x1b[2;3H\x1b[?25l\x1b[?2004h\x1b[2;3;4;7;38;2;1;2;3;48;5;199m");
    let mut restored = restore_into(&mut original, &restore(RestoreMode::FullSnapshot));
    original.process("界!".as_bytes());
    restored.process("界!".as_bytes());
    assert_eq!(restored.snapshot(0), original.snapshot(0));
    assert!(restored.parser.screen().bracketed_paste());
    let state = restored.snapshot(0);
    assert_eq!(state["cursor"]["hidden"], true);
    assert_eq!(state["grid"][1][2]["dim"], true);
    assert_eq!(state["grid"][1][2]["inverse"], true);
}

#[test]
fn snapshot_history_reads_do_not_change_following_capture_or_cursor() {
    let mut original = with_history(20);
    let before = original.capture();
    let cursor = original.parser.screen().cursor_position();
    assert_eq!(
        original.snapshot(5)["scrollback"].as_array().unwrap().len(),
        5
    );
    assert_eq!(original.capture(), before);
    assert_eq!(original.parser.screen().cursor_position(), cursor);
    original.process(b"tail");
    assert_eq!(original.capture().last().unwrap(), "tail");
}

#[test]
fn button_motion_suppresses_hover_without_a_pressed_button() {
    let mut original = screen();
    original.process(b"\x1b[?1002h\x1b[?1006h");
    assert!(
        original
            .mouse(1, 2, 3, MouseAction::Move)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        original.mouse(1, 2, 1, MouseAction::Move).unwrap(),
        b"\x1b[<33;3;2M"
    );
    assert_eq!(
        original.mouse(1, 2, 1, MouseAction::Up).unwrap(),
        b"\x1b[<1;3;2m"
    );
}

#[test]
fn sgr_wheel_input_and_mode_reset_follow_the_terminal_application() {
    let mut original = screen();
    original.process(b"\x1b[?1000h\x1b[?1006h");
    assert_eq!(
        original.mouse(0, 0, 64, MouseAction::Down).unwrap(),
        b"\x1b[<64;1;1M"
    );
    assert_eq!(
        original.mouse(2, 11, 65, MouseAction::Down).unwrap(),
        b"\x1b[<65;12;3M"
    );
    original.process(b"\x1b[?1000l");
    assert!(
        original
            .mouse(0, 0, 0, MouseAction::Down)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn osc_titles_are_bounded_by_characters_without_splitting_unicode() {
    let mut original = screen();
    original.process(format!("\x1b]2;{}\x07", "界".repeat(250)).as_bytes());
    assert_eq!(original.title(), Some("界".repeat(200)));
    assert_eq!(original.snapshot(0)["title"], "界".repeat(200));
    original.process(b"\x1b]2;new\x1b\\");
    assert_eq!(original.title().as_deref(), Some("new"));
}
