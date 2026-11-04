#!/bin/bash
# Compile le mod pour une machine : la bibliotheque va ou nds-mod.json le dit ("plugin").
#   ./build.sh windows-x86_64         Windows : gcc de WinLibs, sans bibliotheque C (retirez -nostdlib pour l'avoir : UCRT)
#   ./build.sh macos-arm64            Mac : clang des Command Line Tools (xcode-select --install)
#   ./build.sh macos-x86_64
# Toutes les sources de src/ font une seule bibliotheque. Sur la CI : .github/workflows/release.yml.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
PLATFORM="${1:?windows-x86_64, macos-arm64 ou macos-x86_64}"
PY="$(command -v python3 || command -v python)"
OUT="$("$PY" -c "import json,sys; print(json.load(open(sys.argv[1]))['plugin'][sys.argv[2]])" "$HERE/nds-mod.json" "$PLATFORM")"
mkdir -p "$HERE/$(dirname "$OUT")"
FLAGS="-O2 -std=gnu11 -Wall -Wextra -Wno-unused-parameter -fno-strict-aliasing"
case "$PLATFORM" in
  windows-x86_64)
    # le point d'entree de la DLL vient de nds_util.h ; sans -nostdlib, definissez NDS_NO_ENTRY et la bibliotheque C prend le sien
    gcc -m64 $FLAGS -fno-builtin -fno-stack-protector -mno-stack-arg-probe -shared -nostdlib -Wl,-e,DllMainCRTStartup "$HERE"/src/*.c -o "$HERE/$OUT" ;;
  macos-arm64|macos-x86_64)
    ARCH="${PLATFORM#macos-}"
    clang -arch "$ARCH" $FLAGS -dynamiclib -fvisibility=hidden -mmacosx-version-min=11.0 "$HERE"/src/*.c -o "$HERE/$OUT" ;;
  *) echo "machine inconnue : $PLATFORM" >&2; exit 2 ;;
esac
echo "$OUT $(wc -c < "$HERE/$OUT") octets"
