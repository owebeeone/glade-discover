#!/bin/sh
set -eu

architecture_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
exec cargo run --quiet --locked \
  --manifest-path "$architecture_root/tools/architecture-check/Cargo.toml" \
  -- "$architecture_root"
