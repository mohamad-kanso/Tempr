#!/usr/bin/env bash
# Run a command inside a hard 4 GB memory cap.
#
# This repo's builds link GPUI and a debug binary, which together are enough to
# push a 30 GB desktop into swap and, with a couple of parallel rustc jobs,
# into the OOM killer. Every cargo invocation in this project goes through this
# script so the whole process tree (rustc, linker, test binaries) is capped by
# the kernel instead of by hope.
#
# Usage:
#   scripts/capped.sh cargo test --workspace
#   scripts/capped.sh cargo clippy --workspace --all-targets -- -D warnings
#
# With no systemd user manager the script still runs the command, but prints a
# warning: the cap then depends on `.cargo/config.toml`'s single build job.
set -euo pipefail

MEMORY_MAX="${TEMPR_MEMORY_MAX:-4G}"
SWAP_MAX="${TEMPR_MEMORY_SWAP_MAX:-0}"

if command -v systemd-run >/dev/null 2>&1 \
    && systemd-run --user --scope -p MemoryMax="${MEMORY_MAX}" -- true >/dev/null 2>&1; then
    exec systemd-run --user --scope \
        -p MemoryMax="${MEMORY_MAX}" \
        -p MemorySwapMax="${SWAP_MAX}" \
        -- "$@"
fi

echo "warning: no systemd user manager; running without a hard memory cap" >&2
exec "$@"
