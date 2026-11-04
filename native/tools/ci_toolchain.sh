#!/bin/bash
# Assemble la boite a outils des jeux natifs sur une machine de CI macOS (pendant de ci_toolchain.ps1) :
#   native/tools/ci_toolchain.sh macos-arm64|macos-x86_64 <archive.zip>
# Python et Capstone sont telecharges aux versions fixees ci-dessous (sha256 verifie) ; pas de compilateur dans l'archive.
# Marche aussi sur un Mac de developpement : le travail est fait dans un dossier temporaire.
set -euo pipefail
PLATFORM="${1:?macos-arm64 ou macos-x86_64}"
OUT="$(cd "$(dirname "${2:?archive de sortie}")" && pwd)/$(basename "$2")"
HERE="$(cd "$(dirname "$0")" && pwd)"

PBS_TAG=20261026
PBS_PYTHON=3.13.18
CAPSTONE=5.0.7
case "$PLATFORM" in
  macos-arm64)
    PBS_TRIPLE=aarch64-apple-darwin
    PBS_SHA256=e5af2fde6a937a0e0c795a0c595c4efb70578d57463a2657c819992b9b9acbac
    WHEEL_PLATFORM=macosx_11_0_arm64 ;;
  macos-x86_64)
    PBS_TRIPLE=x86_64-apple-darwin
    PBS_SHA256=708be9e7df297f68bdae589ea9e62a621db5abe7708f79e851c41410347452d1
    WHEEL_PLATFORM=macosx_10_9_x86_64 ;;
  *) echo "plateforme inconnue : $PLATFORM" >&2; exit 2 ;;
esac

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
ARCHIVE="cpython-$PBS_PYTHON+$PBS_TAG-$PBS_TRIPLE-install_only.tar.gz"
curl -fsSL --retry 3 -o "$WORK/python.tar.gz" "https://github.com/astral-sh/python-build-standalone/releases/download/$PBS_TAG/${ARCHIVE//+/%2B}"
echo "$PBS_SHA256  $WORK/python.tar.gz" | shasum -a 256 -c -
# la roue de la plateforme visee, pas celle du Python de la machine
python3 -m pip download "capstone==$CAPSTONE" --only-binary=:all: --platform "$WHEEL_PLATFORM" --no-deps -d "$WORK/wheel"
python3 "$HERE/make_toolchain.py" --platform "$PLATFORM" --python "$WORK/python.tar.gz" --capstone "$WORK/wheel/capstone-$CAPSTONE-py3-none-$WHEEL_PLATFORM.whl" --out "$OUT"
# la boite commune (3e argument) : Python et Capstone seulement, pour les jeux qui ont leur propre runtime
if [ -n "${3:-}" ]; then
  COMMON="$(cd "$(dirname "$3")" && pwd)/$(basename "$3")"
  python3 "$HERE/make_toolchain.py" --platform "$PLATFORM" --python "$WORK/python.tar.gz" --capstone "$WORK/wheel/capstone-$CAPSTONE-py3-none-$WHEEL_PLATFORM.whl" --out "$COMMON" --common
fi

# Controle : l'archive decompressee lance son Python et charge Capstone, avec le meme environnement que le launcher
CHECK="$WORK/check"
mkdir "$CHECK"
unzip -q "$OUT" -d "$CHECK"
PYTHONPATH="$CHECK/python" PYTHONDONTWRITEBYTECODE=1 "$CHECK/python/bin/python3" -c "import capstone, ctypes, zlib, json, hashlib; print('capstone', capstone.__version__)"
