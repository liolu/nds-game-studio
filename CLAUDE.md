# Nds Game Studio - Instructions Claude

## Build

- Pas de Visual Studio sur ce PC : `build.cmd` (toolchain `stable-x86_64-pc-windows-gnu` + WinLibs sur le PATH).
  Depuis Git Bash, le PATH WinLibs s'ecrit `/c/Users/thomr/AppData/Local/Microsoft/WinGet/Packages/BrechtSanders.WinLibs.POSIX.UCRT_Microsoft.Winget.Source_8wekyb3d8bbwe/mingw64/bin`.
- Toujours en release. Tuer `nds-game-studio.exe` avant de recompiler.
- Plateforme : Windows, PowerShell, clavier AZERTY. `gh` authentifie comme `liolu`.

## Structure

- `crates/engine` (nds-engine) : le moteur Bevy. `screens` (moteurs Main/Sub -> canvas 256x192 -> fenetres),
  `link` (fusionnes = 1 fenetre, separes = 2 fenetres, F9/F8, glisser Alt+clic), `input` (boutons DS + stylet, lus dans FixedPreUpdate), `spec` (game.toml),
  `studio` (registre des jeux, application de la spec), `launcher` (ecran d'accueil : grille/liste, page d'un jeu, favoris, depot d'une rom), `library` (roms du dossier rom, banniere, preferences launcher.toml), `titlebar` (barre de titre dessinee, bouton fusion),
  `devtools` (F5 fenetre Code, surveillance des sources -> cargo build -> relance avec LaunchOptions),
  `options` (F1 fenetre Options), `controllers` (manettes : controllers/<id>.toml, detection, dessin),
  `i18n` (langue : anglais par defaut, francais au choix ; tout texte montre s'ecrit `tr("en", "fr")` ou `t!("en {x}", "fr {x}")`, jamais une seule langue),
  `save` (GameSaves : sauvegardes des jeux dans data_dir()/saves), `settings` (options.toml), `saves_page`,
  `asset_browser` (F6 fenetre Assets : fichiers des jeux decompiles, apercu 2D/3D, chemin NitroFS, menus jeux et formats),
  `asset_scan` (liste, decodage, index des jeux : pourcentages + essai d'un fichier par format, `--example asset_audit`).
- `crates/nitro` (nds-nitro) : formats Nintendo DS standard (NSBMD/NSBTX via le code d'apicula 0BSD, NCGR/NCLR/NSCR/NCER, NARC, LZ77) ;
  essai sans fenetre : `cargo run -p nds-nitro --example probe -- sortie.png <fichier> [palette/tuiles...]`.
- `crates/video` (nds-video, LGPL 2.1+ : portage de FFmpeg et mobipeg, garde sa licence) : videos VX, MODS, Bink 1, image et son ; `engine/src/video.rs` (`VideoPlayer`) les joue dans une image Bevy ;
  essai sans fenetre : `cargo run -p nds-video --release --example vcheck -- <fichier> [dossier de sortie] [n images]`.
- `app` (nds-game-studio) : le binaire. Plus de jeux de demo : les jeux sont les roms de l'utilisateur, joues par decompilation + runtime (pas d'emulation). Un module natif s'enregistre avec `add_ds_game` et le `game_code` de sa rom.
- `tools/` : common / installer / updater / launcher (repris de SpaceSpore).
- Une entite de jeu doit avoir `Engine::Main.layers()` ou `Engine::Sub.layers()` (sinon invisible) et `in_game()`.
  Logique de jeu dans `FixedUpdate` avec `.run_if(playing("<id>"))`.

## Branches et versions

- `experimental` = branche de travail (par defaut). Chaque push publie la pre-release `experimental`.
- `stable` = versions publiees : fusionner `experimental` dans `stable` (PR). La CI cree `vX.Y.Z` + version.json.
- Numero de version : uniquement `[workspace.package] version` dans `Cargo.toml`. Pas de tag `v*` a la main.
- Le launcher fait partie de chaque version et doit toujours etre le plus recent.
