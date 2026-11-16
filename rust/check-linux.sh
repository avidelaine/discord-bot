#!/usr/bin/env bash
# Plan rule 3: the brain, Discord and model calls also compile for Linux (the future home lab).
# Run from Windows:  wsl.exe -d Ubuntu -- bash rust/check-linux.sh
# Add a crate to CRATES as soon as it has code. bobby-audio and bobby-pc stay Windows-only.
set -euo pipefail
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"  # wsl.exe runs a non-login shell
cd "$(dirname "$0")"
CRATES=(bobby-proto bobby-infer bobby-core bobby-discord)
export CARGO_TARGET_DIR="$HOME/.cache/bobby-linux-target"  # keep build files off /mnt/c (slow)
cargo check --locked $(printf -- '-p %s ' "${CRATES[@]}")
echo "linux check ok: ${CRATES[*]}"
