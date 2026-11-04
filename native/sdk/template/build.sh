#!/bin/bash
# Compile le mod pour une machine : la bibliotheque va ou nds-mod.json le dit ("plugin").
#   ./build.sh windows-x86            Windows : gcc -m32 (WinLibs) et ld, sans bibliotheque C, comme le runtime des jeux
#   ./build.sh macos-arm64            Mac : clang des Command Line Tools (xcode-select --install)
#   ./build.sh macos-x86_64
# Toutes les sources de src/ font une seule bibliotheque. Sur la CI : .github/workflows/release.yml.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
PLATFORM="${1:?windows-x86, macos-arm64 ou macos-x86_64}"
PY="$(command -v python3 || command -v python)"
OUT="$("$PY" -c "import json,sys; print(json.load(open(sys.argv[1]))['plugin'][sys.argv[2]])" "$HERE/nds-mod.json" "$PLATFORM")"
mkdir -p "$HERE/$(dirname "$OUT")"
FLAGS="-O2 -std=gnu11 -Wall -Wextra -Wno-unused-parameter -fno-strict-aliasing"
case "$PLATFORM" in
  windows-x86)
    # pas de bibliotheque C en 32 bits chez WinLibs : -fno-builtin, sans protection de pile ni sondes, point d'entree a nous
    OBJS=()
    for src in "$HERE"/src/*.c; do
      obj="$HERE/$(dirname "$OUT")/$(basename "$src" .c).o"
      gcc -m32 $FLAGS -fno-builtin -fno-stack-protector -mno-stack-arg-probe -c "$src" -o "$obj"
      OBJS+=("$obj")
    done
    ld -m i386pe --dll -e _DllMainCRTStartup@12 --enable-reloc-section --dynamicbase "${OBJS[@]}" -o "$HERE/$OUT"
    rm -f "${OBJS[@]}" ;;
  macos-arm64|macos-x86_64)
    ARCH="${PLATFORM#macos-}"
    clang -arch "$ARCH" $FLAGS -dynamiclib -fvisibility=hidden -mmacosx-version-min=11.0 "$HERE"/src/*.c -o "$HERE/$OUT" ;;
  *) echo "machine inconnue : $PLATFORM" >&2; exit 2 ;;
esac
echo "$OUT $(wc -c < "$HERE/$OUT") octets"
