# Nds Game Studio - Instructions Claude

## Build

- Pas de Visual Studio sur ce PC : `build.cmd` (toolchain `stable-x86_64-pc-windows-gnu` + WinLibs sur le PATH).
  Depuis Git Bash, le PATH WinLibs s'ecrit `/c/Users/thomr/AppData/Local/Microsoft/WinGet/Packages/BrechtSanders.WinLibs.POSIX.UCRT_Microsoft.Winget.Source_8wekyb3d8bbwe/mingw64/bin`.
- Toujours en release. Tuer `nds-game-studio.exe` avant de recompiler.
- Temps mesures (moteur, 1 ligne changee) : 40 s, 15-23 s avec `CARGO_PROFILE_RELEASE_INCREMENTAL=true` (dans `build.cmd`) ; le lien de l'app seule prend 12 s, un autre lieur ne gagnerait presque rien. **Ne pas partager un `CARGO_TARGET_DIR` entre deux worktrees** : Cargo garde les memes empreintes et prend pour a jour les sources plus anciennes que le dernier build de l'autre branche, ce qui compile le mauvais code.
- Plateforme : Windows, PowerShell, clavier AZERTY. `gh` authentifie comme `liolu`.

## Structure

- `crates/engine` (nds-engine) : le moteur Bevy. `screens` (moteurs Main/Sub -> canvas 256x192 -> fenetres),
  `link` (fusionnes = 1 fenetre, separes = 2 fenetres, F9/F8, glisser Alt+clic), `input` (boutons DS + stylet, lus dans FixedPreUpdate), `spec` (game.toml),
  `studio` (registre des jeux, application de la spec), `launcher` (ecran d'accueil : grille/liste, page d'un jeu, favoris, depot d'une rom), `library` (roms du dossier rom, banniere, preferences launcher.toml), `titlebar` (barre de titre dessinee, bouton fusion), `logfile` (journal log/launcher.log a cote de l'exe, `NDSGS_LOG_DIR`),
  `devtools` (F5 fenetre Code, surveillance des sources -> cargo build -> relance avec LaunchOptions),
  `options` (F1 fenetre Options), `controllers` (manettes : controllers/<id>.toml, detection, dessin),
  `activity` (journal des evenements, `activity.jsonl`, montre par le launcher : ce que fait l'app, decompilation, historique ; tout evenement passe par `activity::record`),
  `i18n` (langue : anglais par defaut, francais au choix ; tout texte montre s'ecrit `tr("en", "fr")` ou `t!("en {x}", "fr {x}")`, jamais une seule langue),
  `save` (GameSaves : sauvegardes des jeux dans data_dir()/saves), `settings` (options.toml), `saves_page`,
  `asset_browser` (F6 fenetre Assets : fichiers des jeux decompiles, apercu 2D/3D, chemin NitroFS, menus jeux et formats),
  `asset_scan` (liste, decodage, index des jeux : pourcentages + essai d'un fichier par format, `--example asset_audit`).
- `crates/nitro` (nds-nitro) : formats Nintendo DS standard (NSBMD/NSBTX via le code d'apicula 0BSD, NCGR/NCLR/NSCR/NCER, NARC, LZ77) ;
  essai sans fenetre : `cargo run -p nds-nitro --example probe -- sortie.png <fichier> [palette/tuiles...]`.
- `crates/video` (nds-video, LGPL 2.1+ : portage de FFmpeg et mobipeg, garde sa licence) : videos VX, MODS, Bink 1, image et son ; `engine/src/video.rs` (`VideoPlayer`) les joue dans une image Bevy ;
  essai sans fenetre : `cargo run -p nds-video --release --example vcheck -- <fichier> [dossier de sortie] [n images]`.
  `vx::encode_like`, `mobiclip::encode_like` et `bink::encode_like` reecrivent une video (image et son) pour l'outil de mod ; aucun jeu a videos ne demarre en natif : la controler avec `crates/video/tools/vx_arm_check.py`, `mods_arm_check.py` ou `bink_arm_check.py`, qui passent le fichier dans le code ARM du jeu sous Unicorn. Les videos rangees dans une archive (`farc`, `.bundle` + `.toc`, `ANAG`) passent par `nds_nitro::packs`, qui ne connait que ces videos : le reste de l'archive ne change pas. Le decodeur Rust seul ne suffit pas : le code du jeu lit l'ancienne image hors des blocs decodes et calcule sur 32 bits.
- `app` (nds-game-studio) : le binaire. Plus de jeux de demo : les jeux sont les roms de l'utilisateur, joues par decompilation + runtime (pas d'emulation). Un module natif s'enregistre avec `add_ds_game` et le `game_code` de sa rom.
- `native` : runtime nitro-pc (C 32 bits) et traduction ARM vers C, voir `native/README.md` ; `native_game` (moteur) genere le module chez le joueur depuis sa rom, le lance et affiche ses ecrans. Les modules contiennent le code du jeu : jamais commites ni publies. Apres un changement dans `native/`, augmenter `TOOLCHAIN` (`native/tools/export_recipes.py`).
- `tools/` : common / installer / updater / launcher (repris de SpaceSpore).
- `tools/modtools` (nds-mod-tools, sans Bevy) : option telechargee depuis l'onglet Moding (`engine/src/mods.rs`, release `mod-tools`, CI `mod-tools.yml`). Export PNG/OBJ/WAV/MP3/MP4/TXT, import, et `mods/<code>/jeu-modifie.nds` : copie de la rom, jamais la rom elle-meme. Reprend `asset_scan.rs` du moteur par `#[path]`. Apres un changement : augmenter `tools/modtools/version.txt`. Mods venus d'ailleurs (`engine/src/mod_packs.rs`) : fichier, dossier ou lien, dans `mods/<code>/paquets/<id>`, donnees seules, jamais rien d'execute. Essai de toute la chaine : `NDSGS_MOD_TEST_ALL=1` a l'import reimporte tout sans changement, et la rom produite doit se reexporter pareil.
- Une entite de jeu doit avoir `Engine::Main.layers()` ou `Engine::Sub.layers()` (sinon invisible) et `in_game()`.
  Logique de jeu dans `FixedUpdate` avec `.run_if(playing("<id>"))`.

## Assets : parametres par jeu

Chaque jeu peut partager un format avec d'autres et en differer sur un detail. Regles :

1. Avant d'ecrire un decodeur ou de dire qu'un fichier n'est pas lu, essayer sur ce jeu tous les parametres existants : `cargo run --release -p nds-engine --example asset_params -- <dossier des roms> <rapport.tsv> [--images=<dossier>] [--write]`.
2. Ce qui est vrai pour tous les jeux va dans le code commun. Ce qui n'est vrai que pour un jeu s'enregistre pour ce jeu dans `crates/engine/src/game_params.tsv` (`code`, `*.ext` ou `dossier/*.ext`, sorte suivie d'options : `palette=dossier/nom.ncl`, `bpp=4|8`, `bank=n`, `width=n` ; `Other` refuse une devinette fausse), jamais comme une nouvelle devinette qui change les autres jeux. Un nouveau parametre se teste sur tous les jeux.
3. Une image ne se croit qu'a l'oeil : un decodeur qui accepte un fichier ne prouve rien, sur 60 jeux presque toutes ses images etaient du bruit, et un texte de secours n'est pas une reussite. Les propositions pas encore vues restent dans `game_params_a_voir.tsv` et passent dans la table une fois vues justes.
4. Apres un changement de la table, augmenter `INDEX_VERSION` (`asset_scan.rs`).

## Branches et versions

- `experimental` = branche de travail (par defaut). Chaque push publie la pre-release `experimental`.
- `stable` = versions publiees : fusionner `experimental` dans `stable` (PR). La CI cree `vX.Y.Z` + version.json.
- Numero de version : uniquement `[workspace.package] version` dans `Cargo.toml`. Pas de tag `v*` a la main.
- Le launcher fait partie de chaque version et doit toujours etre le plus recent.
