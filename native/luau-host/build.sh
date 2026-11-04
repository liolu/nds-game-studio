#!/bin/bash
# Compile l'hote Luau des mods en script : nds_luau.dll (Windows 64 bits) ou nds_luau.dylib (Mac).
#   native/luau-host/build.sh <sources de Luau> <machine> <sortie>
#   machine : windows-x86_64, macos-arm64, macos-x86_64. Luau : https://github.com/luau-lang/luau, etiquette LUAU_TAG.
# Seuls la VM, le compilateur et l'arbre syntaxique de Luau entrent (pas d'analyse de types ni de generation de code).
set -euo pipefail
LUAU="$(cd "${1:?sources de Luau}" && pwd)"; PLATFORM="${2:?machine}"; OUT="${3:?sortie}"
HERE="$(cd "$(dirname "$0")" && pwd)"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
INC="-I$LUAU/VM/include -I$LUAU/Compiler/include -I$LUAU/Ast/include -I$LUAU/Common/include -I$LUAU/VM/src"
case "$PLATFORM" in
  windows-x86_64) CXX="g++ -m64"; LINK="-shared -s -static -static-libstdc++ -static-libgcc" ;;
  macos-arm64|macos-x86_64) CXX="clang++ -arch ${PLATFORM#macos-} -mmacosx-version-min=11.0"; LINK="-dynamiclib -fvisibility=hidden" ;;
  *) echo "machine inconnue : $PLATFORM" >&2; exit 2 ;;
esac
FLAGS="-O2 -std=c++17 -DNDEBUG -fno-strict-aliasing"
OBJS=()
for src in "$LUAU"/VM/src/*.cpp "$LUAU"/Compiler/src/*.cpp "$LUAU"/Ast/src/*.cpp "$LUAU"/Common/src/*.cpp; do
  obj="$WORK/$(basename "$(dirname "$(dirname "$src")")")_$(basename "$src" .cpp).o"
  $CXX $FLAGS $INC -c "$src" -o "$obj" &
  OBJS+=("$obj")
  # pas plus de 8 compilations a la fois
  while [ "$(jobs -r | wc -l)" -ge 8 ]; do sleep 0.2; done
done
wait
$CXX $FLAGS $INC -c "$HERE/nds_luau.cpp" -o "$WORK/nds_luau.o"
mkdir -p "$(dirname "$OUT")"
$CXX $LINK "$WORK/nds_luau.o" "${OBJS[@]}" -o "$OUT"
echo "$OUT $(wc -c < "$OUT") octets"
