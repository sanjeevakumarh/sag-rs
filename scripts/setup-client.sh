#!/usr/bin/env bash
# setup-client.sh — THIN wrapper. Builds the `sag` client and runs a health check
# against the controller. All real logic lives in the binary. Safe to re-run.
#
# Extra args are forwarded to `sag` (before the subcommand), e.g.:
#   ./scripts/setup-client.sh --peer http://ctrl-a:7000 --peer http://ctrl-b:7000
set -euo pipefail
cd "$(dirname "$0")/.."

BIN="target/release/sag"
[ -x "$BIN" ] || { echo "building sag…"; cargo build --release -p sag-cli; }

[ -f sag.toml ] || echo "tip: copy sag.toml.example → sag.toml and edit peers (config surface reference)"

echo "checking for a controller…"
"$BIN" "$@" doctor && echo && "$BIN" "$@" nodes || {
  echo "no controller reachable yet — start one with scripts/setup-node.sh on a GPU box"
  exit 0
}
