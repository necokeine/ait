#!/usr/bin/env python3
"""Offline executable contract fixture; never used by production adapters."""
import os
from pathlib import Path
import sys
import time
import wave

args = sys.argv[1:]
model = Path(args[args.index("--model") + 1])
if model.read_text() == "block":
    model.with_suffix(".pid").write_text(str(os.getpid()))
    time.sleep(60)
if "--file" in args:
    source = Path(args[args.index("--file") + 1])
    with wave.open(str(source)) as audio:
        assert audio.getframerate() == 16000
        assert audio.getsampwidth() == 2
    model.with_suffix(".seen").write_text(str(source.parent))
    Path(args[args.index("--output-file") + 1] + ".txt").write_text("local transcript\n")
else:
    assert sys.stdin.read().strip() == "speak this"
    output = Path(args[args.index("--output_file") + 1])
    model.with_suffix(".seen").write_text(str(output.parent))
    with wave.open(str(output), "wb") as audio:
        audio.setnchannels(1)
        audio.setsampwidth(2)
        audio.setframerate(22050)
        audio.writeframes(b"\0\0" * 200)
