#!/usr/bin/env bash
set -euo pipefail

cargo creusot -p cachelog-model --features creusot -- --crate-name cachelog_model
