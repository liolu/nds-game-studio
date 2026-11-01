#!/bin/sh
# Essai du son de nitro-pc sur macOS : voir audio_smoke.c. Compile os_posix.c et audio_coreaudio.c avec les options du
# runtime (MACOS.md 3.6), lie libSystem + AudioToolbox, puis lance.
#   sh native/tests/audio_smoke.sh [--arch arm64|x86_64] [--mute]
set -e
HERE=$(cd "$(dirname "$0")" && pwd)
RT="$HERE/../nitro-pc"
ARCH=$(uname -m)
if [ "$1" = "--arch" ]; then ARCH=$2; shift 2; fi
OUT=$(mktemp -d)
trap 'rm -rf "$OUT"' EXIT
FLAGS="-arch $ARCH -std=gnu11 -O2 -Wall -Werror=pointer-to-int-cast -Werror=int-to-pointer-cast -I $RT"
for f in "$RT/os_posix.c" "$RT/audio_coreaudio.c" "$HERE/audio_smoke.c"; do
    ${CC:-clang} $FLAGS -DOS_NO_MAIN -c "$f" -o "$OUT/$(basename "$f" .c).o"
done
${CC:-clang} -arch "$ARCH" "$OUT"/*.o -framework AudioToolbox -o "$OUT/audio_smoke"
"$OUT/audio_smoke" "$@"
