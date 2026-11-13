#!/usr/bin/env bash
# Runs the original game once under the harness with a config written by
# `regress orig export` (or `regress orig run`, which calls this).
#
#   tools/orig-harness/run.sh CONFIG [TIMEOUT_SECONDS]
#
# The game runs with a throwaway HOME, so it starts fresh every time and
# never touches your real saves or settings.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GAME="$ROOT/gamefiles/harness/game"
CONFIG="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
TIMEOUT="${2:-1800}"
[ -x "$GAME/run_bepinex.sh" ] || { echo "run tools/orig-harness/setup.sh first" >&2; exit 1; }

HOME_DIR="$(mktemp -d "$ROOT/gamefiles/harness/home.XXXXXX")"
trap 'rm -rf "$HOME_DIR"' EXIT
# Steam's client socket lives under the real home; the game may want it.
[ -e "$HOME/.steam" ] && ln -s "$HOME/.steam" "$HOME_DIR/.steam"

cd "$GAME"
env HOME="$HOME_DIR" XDG_CONFIG_HOME="$HOME_DIR/.config" XDG_DATA_HOME="$HOME_DIR/.local/share" \
    KTRACE_CONFIG="$CONFIG" \
    timeout "$TIMEOUT" ./run_bepinex.sh ./Kingdom.x86_64 \
    -screen-fullscreen 0 -screen-width 640 -screen-height 360 -logFile "$HOME_DIR/player.log" \
    || status=$?
status="${status:-0}"
cp -f "$HOME_DIR/player.log" "$(dirname "$CONFIG")/player.log" 2>/dev/null || true
cp -f "$GAME/BepInEx/LogOutput.log" "$(dirname "$CONFIG")/bepinex.log" 2>/dev/null || true
exit "$status"
