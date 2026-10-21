@echo off
rem Compile le studio, le launcher, l'updater et l'installeur : target\release\*.exe
rem Pas de Visual Studio sur ce PC : toolchain Rust gnu + WinLibs (dlltool, as, ld) sur le PATH.
cd /d "%~dp0"
set PATH=%LOCALAPPDATA%\Microsoft\WinGet\Packages\BrechtSanders.WinLibs.POSIX.UCRT_Microsoft.Winget.Source_8wekyb3d8bbwe\mingw64\bin;%USERPROFILE%\.cargo\bin;%PATH%
rem L'installeur (windres) ne compile pas en local a cause des espaces du chemin : la CI le construit
cargo +stable-x86_64-pc-windows-gnu build --release --workspace --exclude nds-game-studio-installer %* || exit /b 1
