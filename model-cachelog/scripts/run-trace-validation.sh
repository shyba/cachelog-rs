#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
crate_root="$(cd -- "${script_dir}/.." && pwd)"

RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-nightly}"

if ! command -v apalache-mc >/dev/null 2>&1; then
  echo "apalache-mc not found in PATH; install Apalache or run ./scripts/run-tlc-tracespec.sh for TLC-only validation." >&2
  exit 1
fi

(cd "${crate_root}" && cargo +"${RUSTUP_TOOLCHAIN}" test --features tla-connect trace_validation_roundtrip -- --ignored --nocapture)
