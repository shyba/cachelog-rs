#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"

# Proof obligations currently live in cachelog-core.
# Newer cargo-creusot passes cargo flags after `--`.
(cd "${repo_root}/cachelog-core" && cargo creusot -p cachelog-core -- --features creusot)
