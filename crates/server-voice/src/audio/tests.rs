use super::*;

#[test]
fn pcm_mime_and_wav_roundtrip_preserve_samples_and_rate() {
    for rate in [8000, 16000, 22050, 24000, 44100, 48000] {
        let format = Format::Pcm(rate);
        assert_eq!(Format::parse(&format.mime()), Ok(format));
        let original = Audio {
            bytes: vec![0, 0, 255, 127, 0, 128],
            format,
        };
        let wav = original.clone().wav().unwrap();
        let decoded = Audio {
            bytes: wav,
            format: Format::Wav,
        }
        .pcm()
        .unwrap();
        assert_eq!(decoded.bytes, original.bytes);
        assert_eq!(decoded.format, format);
    }
    assert_eq!(Format::parse("pcm"), Ok(Format::Pcm(24000)));
    assert_eq!(Format::parse("audio/x-wav"), Ok(Format::Wav));
}

#[test]
fn malformed_audio_is_rejected_before_provider_io() {
    for mime in [
        "audio/webm",
        "audio/pcm;rate=0",
        "audio/pcm;bits=32",
        "audio/pcm;channels=2",
        "audio/pcm;rate=16000;rate=24000",
        "audio/pcm;extra=1",
        "audio/pcm;rate=x",
    ] {
        assert_eq!(Format::parse(mime), Err(Error::Invalid), "{mime}");
    }
    for bytes in [vec![], vec![1], vec![0; 43]] {
        assert!(
            Audio {
                bytes,
                format: Format::Wav
            }
            .pcm()
            .is_err()
        );
    }
    let wav = Audio {
        bytes: vec![0; 20],
        format: Format::Pcm(16000),
    }
    .wav()
    .unwrap();
    for offset in [0, 8, 20, 22, 28, 34, 40] {
        let mut bytes = wav.clone();
        bytes[offset] = 255;
        assert!(
            Audio {
                bytes,
                format: Format::Wav
            }
            .pcm()
            .is_err(),
            "{offset}"
        );
    }
    assert!(decode_chunk("?").is_err());
    assert!(decode_chunk("").is_err());
    assert!(decode_chunk(&"A".repeat(MAX_CHUNK_BYTES * 2)).is_err());
    assert_eq!(decode_chunk("AAE=").unwrap(), [0, 1]);
}
