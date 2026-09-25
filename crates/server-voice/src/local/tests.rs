use super::*;

#[test]
fn local_models_are_explicit_and_resampling_preserves_duration() {
    assert!(Whisper::new("whisper-cli".into(), "/missing/voice/model".into()).is_err());
    let bytes = wav_16khz(Audio {
        bytes: vec![0; 48000],
        format: Format::Pcm(24000),
    })
    .unwrap();
    let pcm = Audio {
        bytes,
        format: Format::Wav,
    }
    .pcm()
    .unwrap();
    assert_eq!(pcm.bytes.len(), 32000);
    assert_eq!(pcm.format, Format::Pcm(16000));
}

#[cfg(unix)]
mod process;
