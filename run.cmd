@echo off
rem Lance le studio (compile d'abord si besoin). run.cmd bubbles = lance directement un jeu.
cd /d "%~dp0"
if not exist target\release\nds-game-studio.exe call build.cmd || exit /b 1
start "" target\release\nds-game-studio.exe %*
