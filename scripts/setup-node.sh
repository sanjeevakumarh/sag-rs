#!/usr/bin/env bash
# setup-node.sh — THIN wrapper (see ARCHITECTURE.md "The two bootstrap scripts").
# All real logic lives in the tested `sag-node` binary; this script only builds it,
# runs the detection step, and hands off. Safe to re-run.
#
# Any extra args are forwarded to `sag-node run`, e.g.:
#   ./scripts/setup-node.sh --backend openai-compat --peer http://ctrl:7000
set -euo pipefail
cd "$(dirname "$0")/.."

BIN="target/release/sag-node"
[ -x "$BIN" ] || { echo "building sag-node…"; cargo build --release -p sag-node; }

"$BIN" bootstrap
echo
echo "starting node (Ctrl-C to stop); override via SAG_* env or flags"
exec "$BIN" run "$@"
