#!/usr/bin/env bash
# Sets up the original-game measurement harness in gamefiles/harness/ (ignored
# by Git): a private copy of YOUR game with BepInEx and the KingdomTrace
# plugin. Your own installation and saves are never modified.
#
#   tools/orig-harness/setup.sh [--game "/path/to/Kingdom New Lands"]
#
# Needs: bash, curl, unzip, sha256sum, the .NET SDK (dotnet).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HERE="$ROOT/tools/orig-harness"
HARNESS="$ROOT/gamefiles/harness"
GAME_DIR="${KINGDOM_GAME:-$ROOT/gamefiles/Kingdom New Lands}"
BEPINEX_VERSION="5.4.23.2"
BEPINEX_ZIP="BepInEx_linux_x64_${BEPINEX_VERSION}.zip"
BEPINEX_URL="https://github.com/BepInEx/BepInEx/releases/download/v${BEPINEX_VERSION}/${BEPINEX_ZIP}"
BEPINEX_SHA256="ddc446143a0a277ed92eb7a1f5bf794ff4fdd0023bddc0491f85eafd1898225b"

while [ $# -gt 0 ]; do
    case "$1" in
        --game) GAME_DIR="$2"; shift 2 ;;
        -h|--help) sed -n '2,9p' "$0"; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

for tool in curl unzip sha256sum dotnet; do
    command -v "$tool" >/dev/null || { echo "missing: $tool" >&2; exit 1; }
done
[ -x "$GAME_DIR/Kingdom.x86_64" ] && [ -d "$GAME_DIR/Kingdom_Data" ] || {
    echo "no Linux Kingdom: New Lands at: $GAME_DIR (use --game or KINGDOM_GAME)" >&2
    exit 1
}

echo "== Harness copy of the game (symlinked data, copied executable)"
GAME="$HARNESS/game"
rm -rf "$GAME"
mkdir -p "$GAME"
cp "$GAME_DIR/Kingdom.x86_64" "$GAME/"
cp -as "$(cd "$GAME_DIR/Kingdom_Data" && pwd)" "$GAME/Kingdom_Data"

echo "== BepInEx $BEPINEX_VERSION"
mkdir -p "$HARNESS/cache"
ZIP="$HARNESS/cache/$BEPINEX_ZIP"
[ -f "$ZIP" ] || curl -fL --retry 3 -o "$ZIP" "$BEPINEX_URL"
echo "$BEPINEX_SHA256  $ZIP" | sha256sum -c --quiet || { echo "BepInEx checksum mismatch" >&2; exit 1; }
unzip -q -o "$ZIP" -d "$GAME"
chmod +x "$GAME/run_bepinex.sh"
mkdir -p "$GAME/BepInEx/plugins" "$GAME/BepInEx/config"
# Quiet logging; no console window.
cat > "$GAME/BepInEx/config/BepInEx.cfg" <<'CFG'
[Logging.Console]
Enabled = false
[Logging.Disk]
Enabled = true
CFG

echo "== KingdomTrace plugin"
dotnet build "$HERE/plugin/KingdomTrace.csproj" -c Release -nologo -v quiet \
    -p:GameManaged="$GAME/Kingdom_Data/Managed" \
    -p:BepInExCore="$GAME/BepInEx/core" \
    -o "$HARNESS/plugin-build"
cp "$HARNESS/plugin-build/KingdomTrace.dll" "$GAME/BepInEx/plugins/"

mkdir -p "$HARNESS/traces"
echo "Harness ready in $HARNESS"
echo "Next: cargo run --release -p regress -- orig run orig_walk"
