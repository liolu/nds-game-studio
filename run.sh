#!/usr/bin/env bash
# Nds Game Studio - lanceur Linux / macOS (equivalent de run.cmd)
#  - Dans un dossier d'installation : lance le launcher (mises a jour) ou le studio.
#  - Dans le depot source : compile si besoin puis lance le studio.
cd "$(dirname "$0")" || exit 1
if [ -x "./nds-game-studio-launcher" ]; then
    exec ./nds-game-studio-launcher
fi
if [ -x "./nds-game-studio" ]; then
    exec ./nds-game-studio "$@"
fi
if [ ! -x "target/release/nds-game-studio" ]; then
    cargo build --release -p nds-game-studio || exit 1
fi
exec ./target/release/nds-game-studio "$@"
