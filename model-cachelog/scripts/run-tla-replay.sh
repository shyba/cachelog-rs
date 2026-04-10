#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
crate_root="$(cd -- "${script_dir}/.." && pwd)"

RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-nightly}"
(cd "${crate_root}" && cargo +"${RUSTUP_TOOLCHAIN}" test --features tla-connect replay_inline_trace -- --nocapture)
