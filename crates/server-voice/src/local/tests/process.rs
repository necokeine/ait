use std::os::unix::fs::PermissionsExt;

use super::*;

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let binary = directory.path().join("speech-engine");
    std::fs::write(&binary, include_str!("process/engine.py")).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let model = directory.path().join("model");
    std::fs::write(&model, b"ready").unwrap();
    (directory, binary, model)
}

#[tokio::test]
async fn local_adapters_use_cli_contracts_and_remove_private_audio_files() {
    let (_directory, binary, model) = fixture();
    let stt = Whisper::new(binary.clone(), model.clone()).unwrap();
    let transcript = stt
        .transcribe(
            Audio {
                bytes: vec![0; 4800],
                format: Format::Pcm(24000),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(transcript.text, "local transcript");
    let path = std::fs::read_to_string(model.with_extension("seen")).unwrap();
    assert!(!Path::new(&path).exists());
    let tts = Piper::new(binary, model.clone()).unwrap();
    let audio = tts
        .synthesize("speak this", CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(audio.format, Format::Pcm(22050));
    assert_eq!(audio.bytes.len(), 400);
    let path = std::fs::read_to_string(model.with_extension("seen")).unwrap();
    assert!(!Path::new(&path).exists());
}

#[tokio::test]
async fn cancelled_local_child_is_reaped_and_provider_failure_is_safe() {
    let (_directory, binary, model) = fixture();
    std::fs::write(&model, b"block").unwrap();
    let stt = Whisper::new(binary, model.clone()).unwrap();
    let cancel = CancellationToken::new();
    let signal = cancel.clone();
    let work = tokio::spawn(async move {
        stt.transcribe(
            Audio {
                bytes: vec![0; 320],
                format: Format::Pcm(16000),
            },
            signal,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !model.with_extension("pid").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    assert_eq!(work.await.unwrap().unwrap_err(), Error::Cancelled);
    let pid = std::fs::read_to_string(model.with_extension("pid")).unwrap();
    assert!(
        !std::process::Command::new("kill")
            .args(["-0", &pid])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        Piper::new("/nonexistent/piper".into(), model)
            .unwrap()
            .synthesize("text", CancellationToken::new())
            .await
            .unwrap_err(),
        Error::Unavailable
    );
}
