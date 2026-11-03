# Assemble la boite a outils des jeux natifs sur une machine de CI Windows.
param([Parameter(Mandatory)][string]$Out)
$ErrorActionPreference = "Stop"
$work = Join-Path $env:RUNNER_TEMP "native-toolchain"
New-Item -ItemType Directory -Force -Path $work | Out-Null
$winlibs = "https://github.com/brechtsanders/winlibs_mingw/releases/download/16.2.0posix-14.0.0-ucrt-r1/winlibs-x86_64-posix-seh-gcc-16.2.0-mingw-w64ucrt-14.0.0-r1.zip"
Invoke-WebRequest $winlibs -OutFile "$work/winlibs.zip"
Expand-Archive "$work/winlibs.zip" -DestinationPath "$work/winlibs"
Invoke-WebRequest "https://www.python.org/ftp/python/3.13.1/python-3.13.1-embed-amd64.zip" -OutFile "$work/python.zip"
Expand-Archive "$work/python.zip" -DestinationPath "$work/python"
python -m pip install --target "$work/pkg" capstone==5.0.7
python "$PSScriptRoot/make_toolchain.py" --gcc "$work/winlibs/mingw64" --python "$work/python" --capstone "$work/pkg/capstone" --out $Out
