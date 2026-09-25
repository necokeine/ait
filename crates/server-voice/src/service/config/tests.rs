use std::collections::BTreeMap;

use super::*;

#[test]
fn backends_are_opt_in_and_stt_tts_can_be_mixed() {
    let default = load(|_| None, None).unwrap();
    assert_eq!(default.availability(), (false, false));
    let models = tempfile::tempdir().unwrap();
    let model = models.path().join("voice.onnx");
    std::fs::write(&model, b"model").unwrap();
    let values = BTreeMap::from([
        ("AIT_SPEECH_STT_PROVIDER", "openai".to_owned()),
        ("AIT_SPEECH_TTS_PROVIDER", "local".to_owned()),
        (
            "AIT_SPEECH_PIPER_MODEL",
            model.to_string_lossy().into_owned(),
        ),
    ]);
    let speech = load(|key| values.get(key).cloned(), None).unwrap();
    assert!(speech.stt.is_some());
    assert!(speech.tts.is_some());
    assert_eq!(speech.availability(), (true, false));
    assert!(
        load(
            |key| (key == "AIT_SPEECH_PROVIDER").then(|| "unknown".to_owned()),
            None
        )
        .is_err()
    );
    assert!(
        load(
            |key| (key == "AIT_SPEECH_PROVIDER").then(|| "local".to_owned()),
            None
        )
        .is_err()
    );
}
