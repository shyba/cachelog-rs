#!/usr/bin/env bash
set -euo pipefail

RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-nightly}" \
cargo +"${RUSTUP_TOOLCHAIN}" test -p cachelog-model --features tla-connect replay_inline_trace -- --nocapture
