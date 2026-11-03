#!/bin/bash
# Boite a outils locale pour essayer le launcher sur macOS sans telecharger l'archive de la CI :
#   native/tools/dev_toolchain.sh [dossier]      (par defaut target/release/native-toolchain, a cote du binaire)
# Le dossier contient native -> le depot (traducteur, runtime, recettes) et python/ (Python + capstone). Le compilateur est
# celui des Command Line Tools. Le launcher le prend de lui-meme a cote du binaire, ou avec NDSGS_TOOLCHAIN_DIR=<dossier>.
set -e
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
OUT="${1:-$REPO/target/release/native-toolchain}"
PY="${PYTHON:-python3}"
rm -rf "$OUT"
mkdir -p "$OUT/python/bin"
ln -s "$REPO/native" "$OUT/native"
ln -s "$(command -v "$PY")" "$OUT/python/bin/python3"
"$PY" -m pip install --quiet --target "$OUT/python" capstone==5.0.7
"$OUT/python/bin/python3" -c "import sys; sys.path.insert(0, '$OUT/python'); import capstone; print('capstone', capstone.__version__)"
echo "boite a outils : $OUT"
