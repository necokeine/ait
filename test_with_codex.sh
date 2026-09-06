#!/usr/bin/env bash
set -euo pipefail

# Run the real Codex workflow from this repository, regardless of the caller's cwd.
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"

cargo build -p ait-cli -p ait-daemon
exec cargo test -p ait-cli --test project_creation -- --ignored --nocapture
