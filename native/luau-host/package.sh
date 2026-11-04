#!/bin/bash
# Archive de l'hote Luau pour la release native-toolchain : nds-luau-host-<version>-<machine>.zip (le launcher la telecharge a
# l'installation du premier mod en script). Les sources de Luau sont prises a l'etiquette LUAU_TAG.
#   native/luau-host/package.sh <machine> <dossier de sortie>      machine : windows-x86_64, macos-arm64, macos-x86_64
set -euo pipefail
LUAU_TAG=0.700
PLATFORM="${1:?machine}"; OUT="$(mkdir -p "${2:?sortie}" && cd "$2" && pwd)"
HERE="$(cd "$(dirname "$0")" && pwd)"
PY="$(command -v python3 || command -v python)"
VERSION="$("$PY" -c "import json,sys; print(json.load(open(sys.argv[1])).get('luau_host', 1))" "$HERE/../games/index.json")"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
git clone -q --depth 1 --branch "$LUAU_TAG" https://github.com/luau-lang/luau.git "$WORK/luau"
case "$PLATFORM" in windows-*) NAME=nds_luau.dll ;; *) NAME=nds_luau.dylib ;; esac
"$HERE/build.sh" "$WORK/luau" "$PLATFORM" "$WORK/host/$NAME"
cp "$WORK/luau/LICENSE.txt" "$WORK/host/LICENSE-luau.txt"
printf 'Hote Luau des mods en script de Nds Game Studio (native/luau-host, licence MIT).\nLuau %s : https://github.com/luau-lang/luau, licence MIT (LICENSE-luau.txt).\n' "$LUAU_TAG" > "$WORK/host/NOTICE.txt"
"$PY" - "$WORK/host" "$OUT/nds-luau-host-$VERSION-$PLATFORM.zip" <<'PYEOF'
import sys, zipfile
from pathlib import Path
src, out = Path(sys.argv[1]), sys.argv[2]
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
    for f in sorted(src.iterdir()):
        z.write(f, f.name)
print(out)
PYEOF
