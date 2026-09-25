"""Check the frontend mapping against the authoritative Rust wire catalog."""

from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
rust = (ROOT / "crates/server-protocol/src/methods.rs").read_text()
client = (ROOT / "apps/app/src/runtime/rust-server/methods.ts").read_text()
expected = {
    source: (kind, canonical)
    for kind, _, source, canonical in re.findall(
        r'(request|event|response)!\(\s*(\w+),\s*"([^"]+)",\s*"([^"]+)"\s*\)', rust
    )
}
actual = {
    source: (kind, canonical)
    for source, canonical, kind in re.findall(
        r'"?([\w./-]+)"?:\s*\{\s*method:\s*"([^"]+)",\s*kind:\s*"([^"]+)"', client
    )
}
if len(expected) != 171 or actual != expected:
    raise SystemExit("Frontend method names/directions differ from Rust catalog")
print("Verified 171 frontend mappings to 168 canonical Rust methods")
