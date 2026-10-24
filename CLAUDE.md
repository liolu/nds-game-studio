# Nds Game Studio - Instructions Claude

## Build

- Pas de Visual Studio sur ce PC : `build.cmd` (toolchain `stable-x86_64-pc-windows-gnu` + WinLibs sur le PATH).
  Depuis Git Bash, le PATH WinLibs s'ecrit `/c/Users/thomr/AppData/Local/Microsoft/WinGet/Packages/BrechtSanders.WinLibs.POSIX.UCRT_Microsoft.Winget.Source_8wekyb3d8bbwe/mingw64/bin`.
- Toujours en release. Tuer `nds-game-studio.exe` avant de recompiler.
- Plateforme : Windows, PowerShell, clavier AZERTY. `gh` authentifie comme `liolu`.

## Structure

- `crates/engine` (nds-engine) : le moteur Bevy. `screens` (moteurs Main/Sub -> canvas 256x192 -> fenetres),
  `link` (fusionnes = 1 fenetre, separes = 2 fenetres, F9/F8, glisser Alt+clic), `input` (boutons DS + stylet, lus dans FixedPreUpdate), `spec` (game.toml),
  `studio` (registre des jeux, menu, application de la spec), `titlebar` (barre de titre dessinee, bouton fusion),
  `devtools` (F5 fenetre Code, surveillance des sources -> cargo build -> relance avec LaunchOptions),
  `options` (F1 fenetre Options), `controllers` (manettes : controllers/<id>.toml, detection, dessin),
  `save` (GameSaves : sauvegardes des jeux dans data_dir()/saves), `settings` (options.toml), `saves_page`.
- `app` (nds-game-studio) : le binaire + jeux de demo dans `app/src/games/<id>/` (game.toml + mod.rs).
- `tools/` : common / installer / updater / launcher (repris de SpaceSpore).
- Une entite de jeu doit avoir `Engine::Main.layers()` ou `Engine::Sub.layers()` (sinon invisible) et `in_game()`.
  Logique de jeu dans `FixedUpdate` avec `.run_if(playing("<id>"))`.

## Branches et versions

- `experimental` = branche de travail (par defaut). Chaque push publie la pre-release `experimental`.
- `stable` = versions publiees : fusionner `experimental` dans `stable` (PR). La CI cree `vX.Y.Z` + version.json.
- Numero de version : uniquement `[workspace.package] version` dans `Cargo.toml`. Pas de tag `v*` a la main.
- Le launcher fait partie de chaque version et doit toujours etre le plus recent.
