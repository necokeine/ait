#!/usr/bin/env bash
set -euo pipefail

# Resolve a caller-supplied dotenv path before changing to the repository root.
if [[ $# -gt 1 ]]; then
  printf 'Usage: %s [path/to/.env]\n' "$0" >&2
  exit 2
fi
if [[ $# -eq 1 ]]; then
  export AIT_DEEPSEEK_ENV_FILE="$(cd -- "$(dirname -- "$1")" && pwd)/$(basename -- "$1")"
fi
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
export AIT_DEEPSEEK_ENV_FILE="${AIT_DEEPSEEK_ENV_FILE:-$PWD/.env}"
if [[ ! -f "$AIT_DEEPSEEK_ENV_FILE" ]]; then
  printf 'Missing .env file; pass its path as the first argument (see WF-11).\n' >&2
  exit 1
fi

# Rust reads only DEEPSEEK_API_KEY; never source a credentials file as shell code.
cargo build -p ait-cli -p ait-daemon
exec cargo test -p ait-cli --test deepseek_workflow wf11_real_deepseek_python_hello_world -- --ignored --exact --nocapture
