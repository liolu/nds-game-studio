# Jeux DS natifs : traduction + runtime nitro-pc

Un jeu DS tourne ici comme un programme natif, sans émulateur : pas d'interpréteur ARM au moment de jouer. Le module est
un programme Windows 32 bits (GCC) ou, sur Mac, un programme arm64 ou x86_64 (clang).

```
rom de l'utilisateur ──► chargée en mémoire, aux adresses du DS (données, autoload)
symbols.txt + code ARM ──► tools/arm2c.py ──► C ──► gcc -m32 ──► x86 ┐
nitro-pc/ (registres, BIOS, threads, 2D, 3D, son, sauvegarde) ───────┴─► native/build/<code>/<code>.exe
                                                                          lancé par le launcher (Jouer)
```

Sur Mac, la même chaîne donne `native/build/<code>/<code>` (sans `.exe`) avec `clang -arch arm64` ou `-arch x86_64`.

## Plateformes

| | Windows | macOS |
|---|---|---|
| Module | x86 32 bits, `gcc -m32` (WinLibs, dans la boîte) | arm64 ou x86_64, `clang` d'Apple (outils en ligne de commande de Xcode, **hors de la boîte** : le SDK d'Apple ne peut pas être redistribué) |
| Mémoire DS | adresse DS = adresse de l'hôte (`nds_mem = 0`, code identique à avant) | fenêtre de 4 Go + 64 Ko réservée n'importe où : l'adresse DS `a` est à `nds_mem + a` (macros `DS()` et `HOST2DS()`) |
| Couche système | `os_win32.c` | `os_posix.c` + `audio_coreaudio.c` (AudioQueue) |
| Mode traduit | oui | oui |
| `--with-c` | oui | **non** (pointeurs 64 bits ; le piège d'E/S n'existe qu'en Win32) |
| Boîte à outils | `native-toolchain-<v>.zip` (100 Mo) | `native-toolchain-<v>-macos-<arch>.zip` (22 Mo : traducteur, runtime, recettes, Python, capstone) |

La conception, ce qui est fait et ce qui reste : [MACOS.md](MACOS.md). Sur Mac, seul AL7E a démarré (sans fenêtre,
jusqu'à son écran titre) ; voir la colonne macOS de [GAMES.md](GAMES.md).

## Chez le joueur : le launcher génère le module

Les outils sont livrés avec le studio (`native-toolchain` à côté de l'exécutable, 100 Mo). Dès qu'il voit une rom, le
launcher prépare son module en tâche de fond, un jeu à la fois et jamais pendant une partie : il vérifie le SHA-1 de la
rom, traduit, compile (de 30 s à 1 min 30 selon le jeu) et garde le résultat dans `native/<code>-<n>.exe` du dossier des
données (sans `.exe` sur Mac). Le bouton Jouer s'active quand c'est prêt. Les jeux expérimentaux ne se préparent que sur demande (bouton Préparer).

Sans ce dossier (compilation locale), la boîte est téléchargée une fois (release `native-toolchain`).
Sur Mac, le launcher vérifie d'abord `xcrun --find clang` et, s'il manque, explique `xcode-select --install`
avant de télécharger quoi que ce soit.

**Rien de publié ne contient de code de jeu.** La boîte à outils contient le traducteur, le runtime, GCC (Windows seulement), Python et les
recettes `games/<code>.json.gz` (positions des fonctions, pointeurs et noms, pas d'instructions). Les modules, eux,
contiennent le code du jeu : ils ne doivent être ni commités ni publiés. `native/build/` est ignoré par git.

Quand le runtime, le traducteur ou les recettes changent, augmenter `TOOLCHAIN` dans `tools/export_recipes.py`, relancer
ce script : le numéro passe dans `games/index.json`, la CI publie la nouvelle boîte, et les modules sont régénérés.

Une rom d'une autre version que celle de la recette est refusée. Un `native/<code>.exe` à côté du studio passe avant
le module généré ; `NDSGS_TOOLCHAIN_ZIP` donne une boîte locale au lieu de la télécharger (sur Mac aussi : `NDSGS_TOOLCHAIN_DIR` donne
un dossier, que `native/tools/dev_toolchain.sh` prépare depuis le dépôt).

## Pour le développement

```bash
python native/tools/native_build.py "<dossier du projet>"       # un jeu, depuis son projet
python native/tools/export_recipes.py "<projets>" "<roms>"      # les recettes et l'index
python native/tools/build_all.py "<projets>" "<roms>"           # essaie tous les jeux, écrit GAMES.md
python native/tools/make_toolchain.py --gcc ... --python ... --capstone ... --out x.zip
```

Sur Mac :

```bash
python3 native/tools/native_build.py "<dossier du projet>" [--arch arm64|x86_64] [--instrument]
python3 native/tools/make_toolchain.py --platform macos-arm64 --python <python-build-standalone> --capstone <roue> --out x.zip
pip install capstone==5.0.7 && python3 native/tests/arm2c_test.py   # sans rom : traducteur, appels indirects, threads
sh native/tests/audio_smoke.sh [--arch x86_64] [--mute]               # 2 s de sinus par CoreAudio
python3 native/tests/runtime_check.py                                 # compile tout le runtime avec -Werror, comme la CI
```

`arm2c_test.py` et `runtime_check.py` ne demandent aucune rom ; la CI les exécute
(`.github/workflows/native-toolchain.yml`, tâches `runtime-macos` et `runtime-windows`).

Il faut un jeu avec ses symboles SDK nommés (`OS_SaveContext`, `OS_SelectThread`, `FSi_ReadRomCallback`…) pour qu'il
se lance : les autres sont proposés comme expérimentaux, voir [GAMES.md](GAMES.md).

## Le code vient de l'ARM, pas encore du C décompilé

Par défaut tout le jeu est traduit depuis la rom. Sur Mac, c'est le seul mode. `--with-c` (Windows) compile aussi le C matché, mais ce C compte sur le
compilateur d'origine : appels `f()` qui gardent r0-r3, retours par drapeaux, variables propres à un fichier. Ce mode
plante aujourd'hui sur Tom et Jerry (`func_0203e3f4`). Le C matché est correct pour mwcc, pas forcément pour gcc.
Depuis que la mémoire du jeu garde ses vrais pointeurs de fonctions (plus de `nds_fixups`), `--with-c` ne peut plus
appeler un pointeur lu dans les données du jeu : à reprendre si ce mode doit revivre.

## Limites connues

- Lego Star Wars II : parasites sur l'écran tactile pendant le jeu. La carte de la couche BG1 du moteur secondaire contient
  des valeurs aléatoires, sans écriture visible du code traduit (`--watch`) : cause pas encore trouvée.
- Un jeu sans symboles SDK nommés ne se lance pas encore ; les overlays ne sont pas gérés.
- 3D : pas d'ombres (polygones mode 3), de brouillard, de contours ni de table de toon ; polygones triés seulement
  opaques puis transparents.
- 2D : pas de mosaïque ni de capture d'écran ; effets ligne par ligne absents, l'image est dessinée entière au VBlank.
- Pas de préemption : un thread passe la main sur une attente, une interruption ou un accès registre.
- Son : séquenceur et mixeur simplifiés (portamento et valeurs aléatoires du SSEQ ignorés).
- Micro absent. Pas de cartouche GBA. La liaison sans fil passe par TCP (ci-dessous) ; Internet Wi-Fi du DS (DWC) absent.
- Mac : pas de `--with-c` ; les outils de Xcode sont requis ; le chemin jusqu'à l'écran du launcher n'est pas encore
  vérifié (la capture de `native_check` est noire) ; vitesse en jeu non mesurée.

## Fonctions demandées par les mods

Un mod ne contient pas de code : son `native.json` demande une de ces fonctions, que le launcher traduit en option du module (`mod_packs::native_args`).

- `--interpolate` (`gx3d.c`) : un jeu qui dessine à 30 images/s en montre 60. À chaque nouvelle liste de polygones, une image intermédiaire est calculée entre la précédente et la nouvelle ; les polygones se retrouvent par leurs sommets d'origine, avant le découpage. Rien ne change dans le code du jeu, sa vitesse reste la même. Coût : une image de retard à 60 images/s.
- `--first-person` (`first_person.c`) : la caméra du jeu est remplacée juste avant le calcul de sa vue, et dirigée par la souris (le launcher envoie le cumul de ses mouvements et retient le curseur, F7 le rend ; `--look "image:x:y,..."` pour les essais). Le jeu oriente les touches d'après son propre cap : il reçoit le nôtre, donc haut avance dans le regard et gauche, droite vont sur les côtés. Le personnage regarde dans la même direction. Son corps est caché pour ce joueur, seuls ses bras restent, levés et posés en bas de l'image (`gx3d.c`, modes `nds_fp_mode`) ; le clic gauche envoie le bouton L, et un viseur est dessiné au centre. Il faut un jeu décrit dans `games/patches.json`.
- `--free-camera` (`free_camera.c`) : le launcher envoie deux bits de plus avec les touches. C (bit 12) détache la caméra du jeu : souris pour le regard, flèches pour avancer et aller sur les côtés, Q et W (L et R du DS) pour descendre et monter, X (A du DS) pour aller plus vite, Z (B du DS) pour aller moins vite. Le jeu continue sans recevoir de touches : le personnage reste sur place et une cinématique continue. V (bit 13) gèle aussi la scène : la mise à jour du niveau est sautée (`gates` dans `patches.json`, une accroche qui peut empêcher la fonction du jeu de tourner), les tâches sont coupées, les objets reçoivent le drapeau de pause du jeu. Ces deux bits s'essaient avec `--keys` (4096 et 12288) et `--look`.
- `--skip-intro` (`net.c`) : les écrans du menu s'ouvrent dès le premier écran du jeu.
- `--net-port n`, `--net-join adresse[:port]` (`net.c`) : les fonctions `WM_*` du SDK remplacées par du TCP. Le partage de données image par image est gardé tel quel : l'hôte attend l'envoi de chaque invité et renvoie le lot à tous. `Select + X` ouvre le menu multijoueur du jeu. `--net-log` trace les échanges, `--net-menu image` ouvre ce menu à une image donnée.

Pas de partie téléchargée (une seule carte) : le jeu de l'invité y est un autre programme de la rom, sans recette.

`games/patches.json` donne, par jeu, ce que la recette ne dit pas : noms de fonctions du SDK (`names`), accroches du runtime à l'entrée d'une fonction (`hooks`) ou qui peuvent l'empêcher de tourner (`gates`), adresses (`values`) et fonctions du jeu appelées par le runtime (`functions`). `nds_hook_spin` sert aux boucles d'attente du jeu qui ne touchent aucun registre : sur le DS une interruption les débloque, ici elle n'arrive que si on lui laisse la place.

## Essais sans fenêtre

```bash
bash native/tools/run_game.sh AJTP "<rom>" 60 --mute --save test.sav --dump captures --every 300 --frames 3000 --keys "2700:0/128/150,2710:0"
```

- `--peek "image:adresse:taille:fichier,..."` copie de la mémoire dans un fichier ; `--poke "image:adresse:valeur,..."` y écrit un mot ; `--dumpfrom image` commence les captures plus tard.
- `--keys "image:masque[/x/y],..."` : touches (bits de KEYINPUT : A=1, B=2, Select=4, Start=8, droite=16, gauche=32,
  haut=64, bas=128, R=256, L=512, X=1024, Y=2048) et stylet.

## Image

Le jeu dessine ses écrans à la définition de la fenêtre, dans des fils à part : il garde ainsi sa cadence, et une image trop longue à dessiner est simplement sautée.

- 3D : recalculée à la définition demandée, textures filtrées (bilinéaire). En format large, elle montre ce que le DS coupait sur les côtés.
- 2D : les moteurs dessinent toujours 256x192, car c'est la définition des dessins du jeu. L'image est affinée par Scale2x, posée sur la 3D et reste au centre, avec du noir sur les côtés si la 3D n'est pas affichée.
- Launcher : l'option Rendu (HD ou DS) et le format (F8 ou options) s'appliquent en jeu. Sans format choisi, un jeu natif démarre en 16:9.
- Échange : mémoire partagée (`--shm nom`, voir `present.c` et `native_game.rs`) ; le paquet d'entrée donne aussi la largeur en pixels DS et le nombre de pixels par pixel DS.
- Essais sans fenêtre : `--scale k --dswidth n --dump dossier --every n --frames n`.

Limites : les ombres (mode 3 des polygones) ne sont pas dessinées. Un objet que le jeu cache hors de l'écran du DS peut apparaître sur les côtés en format large : F8 repasse en 4:3.
- `--trace`, `--prof` : état et profil ; `--wav fichier` : le son mixé en PCM brut.
- `native_build.py --instrument` : version de débogage (dernière instruction ARM, registres et derniers appels au plantage) ;
  `--watch adresse` y montre qui écrit à une adresse.
- `cargo run --release -p nds-engine --example native_check -- <code> <rom> <capture.png> <s> [enter@s] [touch@s]` :
  passe par le vrai moteur, sans focus (`NDSGS_NATIVE_MUTE=1`, `NDSGS_NATIVE_DIR`).

Essayé jusqu'au jeu : Tom et Jerry Tales (AJTP), Rayman Raving Rabbids (AR5E), Lego Star Wars II (AL7E) sur Windows ;
sur Mac, seulement AL7E, sans fenêtre.
