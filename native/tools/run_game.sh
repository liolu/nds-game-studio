#!/bin/bash
# Lance un jeu natif quelques secondes et traduit les adresses x86 du journal en noms de fonctions.
# run_game.sh <code> <rom> [secondes] [arguments]
W=/c/Users/thomr/AppData/Local/Microsoft/WinGet/Packages/BrechtSanders.WinLibs.POSIX.UCRT_Microsoft.Winget.Source_8wekyb3d8bbwe/mingw64/bin
export PATH="$W:$PATH"
HERE="$(cd "$(dirname "$0")" && pwd)"
code=$1; rom=$2; secs=${3:-15}; shift 3
dir="$HERE/../build/$code"
base=${code%-debug}; base=${base%-c}
exe="$dir/$base.exe"
timeout "$secs" "$exe" "$rom" "$@" > "$dir/run.log" 2>&1
echo "exit=$?"
python "$HERE/sym.py" "$exe" "$dir/run.log" > "$dir/run.txt"
tail -${LOG_LINES:-40} "$dir/run.txt"
