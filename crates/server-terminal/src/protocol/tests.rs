use super::*;
use serde_json::json;

#[test]
fn terminal_messages_validate_enums_dimensions_and_utf8_frames() {
    let create: CreateRequest = serde_json::from_value(json!({"cwd":"/workspace"})).unwrap();
    assert_eq!(create.size, Size { rows: 24, cols: 80 });
    assert!(create.args.is_empty());
    for size in [
        Size { rows: 0, cols: 80 },
        Size {
            rows: 100,
            cols: 200,
        },
        Size {
            rows: 24,
            cols: 201,
        },
    ] {
        assert_eq!(size.validate(), Err(crate::Error::Invalid));
    }
    assert!(
        serde_json::from_value::<CreateRequest>(json!({"cwd":"/", "size":{"rows":1.5,"cols":80}}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<SubscribeRequest>(
            json!({"terminalId":"t","restore":{"mode":"wrong"}})
        )
        .is_err()
    );
    let input = client_frame(&frame(Opcode::Input, 7, "输入\r".as_bytes())).unwrap();
    assert_eq!(input.0, 7);
    assert!(matches!(input.1, Input::Input { data } if data == "输入\r"));
    let resize = client_frame(&frame(
        Opcode::Resize,
        8,
        br#"{"rows":30,"cols":90,"intent":"update"}"#,
    ))
    .unwrap();
    assert!(matches!(
        resize.1,
        Input::Resize(Resize {
            intent: ResizeIntent::Update,
            ..
        })
    ));
    for bytes in [
        vec![],
        vec![2],
        vec![1, 0],
        vec![4, 0],
        vec![5, 0],
        vec![9, 0],
        vec![2, 0, 255],
        vec![3, 0, 1],
    ] {
        assert!(client_frame(&bytes).is_err());
    }
}

#[test]
fn capture_default_and_input_forms_round_trip_source_shapes() {
    let capture: CaptureRequest =
        serde_json::from_value(json!({"terminalId":"t","start":-10,"end":-1})).unwrap();
    assert!(capture.strip_ansi);
    assert_eq!(capture.start, Some(-10));
    for value in [
        json!({"type":"input","data":"hello"}),
        json!({"type":"resize","rows":24,"cols":80}),
        json!({"type":"mouse","row":1,"col":2,"button":0,"action":"down"}),
    ] {
        assert!(serde_json::from_value::<Input>(value).is_ok());
    }
    assert_eq!(CAPABILITIES.len(), 10);
    assert!(CAPABILITIES.contains(&"terminal.input"));
}
