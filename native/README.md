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

Rien ne se prépare tout seul : un module ne se génère que lorsque le joueur clique sur Préparer, Jouer ou une mise à jour, jamais au démarrage du launcher. Le démarrage ne télécharge que l'index des jeux (`index.json` de la release `native-toolchain`). La génération vérifie le SHA-1 de la rom, traduit et compile (de 30 s à 2 min selon le jeu), puis garde le résultat dans `native/` du dossier des données (sans `.exe` sur Mac). Chaque jeu est indépendant : préparer, mettre à jour ou casser l'un ne touche pas aux autres.

**Un runtime par jeu** (ROADMAP-MODS.md, étape 3 bis). Un jeu dont l'index donne `"runtime": {"repo": "...", "version": n}` prend son runtime dans la release `runtime-v<n>` de son dépôt (`native-runtime-<n>.zip`, contrôlé par `SHA256SUMS`, 170 Ko). Le compilateur et Python viennent de la boîte commune (`native-common-<c>.zip`, 35 Mo), téléchargée une fois au premier jeu joué. Son module s'appelle `<code>-r<n>`. La version installée est notée dans `native/<code>.json`. Quand l'index en annonce une plus récente, la page du jeu la propose (« Installer la mise à jour », « Ne plus la proposer »), sans jamais l'imposer. L'ancien module reste sur le disque jusqu'à ce que le nouveau ait tourné dix secondes ; s'il s'arrête avant, le launcher revient à l'ancienne version et le dit.

Dans le dépôt d'un jeu, `native/` (licence MIT, `RUNTIME-LICENSE.txt`) se remplit avec `tools/runtime_sync.py`, qui montre l'écart avec le studio et ne reporte que si on le demande. Sa CI (`tools/runtime-ci.yml`, copiée en `.github/workflows/native-runtime.yml`) vérifie le runtime sans rom, puis publie l'archive à l'étiquette `runtime-v<n>` (`tools/runtime_package.py`). Les jeux sans `"runtime"` gardent la boîte à outils commune (`native-toolchain`, module `<code>-<TOOLCHAIN>`).

Le paquet Windows n'emporte plus la boîte à outils : elle se télécharge au premier jeu joué. Une boîte à côté de l'exécutable (paquets d'avant) ne sert que si sa version est celle de l'index.

Pour les essais : `NDSGS_NATIVE_INDEX` (adresse de l'index), `NDSGS_TOOLCHAIN_URL` (release de la boîte commune), `NDSGS_GITHUB_DOWNLOAD` (à la place de `https://github.com` pour les releases des jeux).

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

Un mod de données peut encore demander, par son `native.json`, une de ces fonctions du runtime, que le launcher traduit en option du module (`mod_packs::native_args`). Tout nouveau comportement passe par un mod natif (section « Mods natifs »).

- `--interpolate` (`gx3d.c`) : un jeu qui dessine à 30 images/s en montre 60. À chaque nouvelle liste de polygones, une image intermédiaire est calculée entre la précédente et la nouvelle ; les polygones se retrouvent par leurs sommets d'origine, avant le découpage. Rien ne change dans le code du jeu, sa vitesse reste la même. Coût : une image de retard à 60 images/s.
- La vue à la première personne, la caméra libre et skip-intro ne sont plus ici : ce sont des mods natifs (format 2, `nds_mod.h`), dans le dépôt des mods de base (`premiere-personne`, `camera-libre`, `skip-intro`, avec leurs sources). Le runtime ne garde rien de propre à eux. Pour les essais sans fenêtre : `--plugins`, `--keys`, `--look` et `--hid "image:code,..."` (touches du clavier entier, codes HID).
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
- `--fixed-clock` : images reproductibles, pour comparer deux modules image par image. L'horloge avance de 1000 cycles à chaque lecture, sans attente ; le dessin et le son tournent dans le fil du jeu. Le jeu va aussi vite que le PC le permet : le temps d'un passage mesure le coût d'une image. Tom et Jerry et Rayman donnent les mêmes images à chaque passage ; Lego, en jeu, varie encore.

## Mods natifs (interface 1.0)

Un mod natif est une bibliothèque qui ne connaît que [`sdk/nds_mod.h`](sdk/nds_mod.h) (MIT) : elle exporte `nds_mod_load(api, mod)` et reçoit la table `nds_api`. Exemples : `tests/mod_essai`.

- **Chargement** (`mods.c`) : `--plugins plugins.json` donne la liste, dans l'ordre. Pour chaque mod : `id`, `version`, `api` (`"1.0"`), `games` (liste de codes, ou objet code → SHA-1 vérifiés par le launcher), `plugin` (objet plateforme → chemin : `windows-x86`, `windows-x86_64`, `macos-arm64`, `macos-x86_64`) et `settings`. Le module refuse un mod d'une autre majeure, d'une mineure plus récente, d'un autre jeu ou sans bibliothèque pour cette machine, puis écrit l'état de chacun dans `status.json`, à côté (ou `--mod-status`). Un mod refusé ne bloque pas le jeu. `--plugin a.dll,b.dll` charge des bibliothèques sans manifeste, pour les essais ; `--no-mods` n'en charge aucune.
- **Accroches** : chaque fonction traduite passe par une entrée en assembleur (`HOOK_ENTRY` de `arm2c.py`). Si un mod a rempli sa case `nds_hook_slot[k]`, elle saute à l'accroche, sinon au corps traduit `f__o`, sans recopier les arguments (`--no-hooks` enlève ce test, pour les mesures). Le dernier mod chargé passe en premier, et `original` mène au suivant, puis au jeu. Un mod qui se refuse pendant son chargement perd ses accroches.
- **Accroches à une instruction** : `native_build.py --insn-hooks 0x...,0x...` place `NDS_INSN` avant ces instructions. Les registres passent par `nds_regs`, modifiables. Le launcher donnera les adresses `"instruction_hooks"` des mods actifs (étape 3).
- **Moteur** : `on_frame`, `on_gx` (commandes 3D), `on_polygon` (sommets modifiables), `on_present` (images finales), `on_audio`, `on_io_read`, `on_io_write`, `on_irq`, `on_rom_read`. Sans mod, chacun ne coûte qu'un test de pointeur. `api->runtime(nom)` donne toute fonction ou variable globale du runtime (`nds_runtime_syms` de `tables.c`), sans promesse de stabilité.
- **Entrées** : `input` donne les touches du DS, le stylet et la souris ; `take_input` retire les touches au jeu. Le clavier entier, `capture_mouse` et `notice` attendent le nouveau contrat avec le launcher (étape 2) : pour l'instant, `notice` écrit seulement dans le journal.
- Un plantage écrit les mods chargés dans le journal.
- **Dans le launcher** (`mod_packs.rs`) : un manifeste `format: 2` avec `"type": "native"` et `"plugin"` (machine → bibliothèque) s'installe par lien ou par fichier comme un mod de données. Seule la bibliothèque de la machine est gardée ; d'une release, `<id>-<version>-<machine>.zip` passe avant `<id>-<version>.zip`, contrôlé par `SHA256SUMS`. Avant l'installation, puis sur la ligne du mod, le launcher affiche « Exécute du code sur cet ordinateur » et la source. Les réglages du manifeste (`bool`, `int`, `float`, `choice`, `string`, `keybind`, `presets`) se changent sur cette ligne et vont dans `reglages.json`. Au lancement, `plugins.json` est écrit à côté, et la ligne montre ensuite ce que `status.json` a dit à la dernière partie.
- Le launcher refuse lui-même un mod dont `games` liste des SHA-1 qui ne sont pas celle de la rom (champ `refused` de `plugins.json`). Avec `"save": "separate"`, la sauvegarde devient `backup-<mods>.sav`. Avec `"instruction_hooks": {"AL7E": [...]}`, le module est généré avec ces adresses, sous un autre nom (`<code>-<version>-h<empreinte>`), et l'ancien reste sur le disque.
- Une touche (`keybind`) se garde par son code HID ; le manifeste peut la donner par son nom (`"KeyC"`). Le mod reçoit le nombre (`api->setting` rend `"6"`).

## Contrat launcher-module (protocole 2)

Le launcher ne sait rien des mods : il envoie tout ce que fait le joueur, et c'est le module qui décide.

- **Entrée standard** (`input_thread` de `present.c`, `send_input` de `native_game.rs`) : paquet de 64 octets qui commence par `0x8002`. Il contient les touches du DS, le stylet, la largeur et l'échelle de l'image, le cumul de la souris, l'état de la fenêtre (curseur retenu, fenêtre active), les boutons et la molette de la souris, puis le clavier entier en 256 bits (codes HID, `NDS_KEY_*` de `sdk/nds_mod.h`). Le paquet de 16 octets d'avant reste accepté.
- **En-tête de la mémoire partagée**, écrit par le module : `protocol` (2), `flags` (bit 0 : retenir le curseur), `notice_seq` et un message en UTF-8 à l'octet 64, que le launcher affiche. Un module qui n'écrit pas `protocol` reçoit l'ancien paquet.
- `--lang fr|en` : la langue des messages du runtime et `api->language` des mods.
- **Multi** : à la connexion, l'hôte envoie son empreinte (`MSG_MODS` de `net.c`) : la version du runtime (`nds_runtime_version`, empreinte de ses sources) et les mods chargés, sauf ceux qui déclarent `"multiplayer": "safe"`. L'invité refuse si elle diffère, sinon il renvoie la sienne. L'hôte refuse tout invité dont l'empreinte n'est pas la même. Le joueur voit un message, et le journal donne les deux empreintes.

## Windows 64 bits (essai)

`native_build.py --arch x86_64` sous Windows : la mémoire DS passe dans une fenêtre de 4 Go, comme sur Mac (`os_mem_window` de `os_win32.c`), et `context.c` suit l'ABI Windows x64. Sur Tom et Jerry et Rayman, les images sont les mêmes qu'en 32 bits sur 6000 images. Le module est plus gros (+37 % sur Tom et Jerry, +33 % sur Lego) et pas plus lent. Un mod 64 bits peut se servir de la bibliothèque C (UCRT). Le launcher lance toujours le 32 bits.

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

Essayé jusqu'au jeu : Tom et Jerry Tales (AJTP), Rayman Raving Rabbids (AR5E), Lego Star Wars II (AL7E), Pokémon Dash (APDE) sur Windows ;
sur Mac, seulement AL7E, sans fenêtre.

## Jeux au SDK ancien : Pokémon Dash

SDK plus ancien, pilote de carte propre au jeu et vraies exceptions C++. Tout est réservé à ce jeu dans `games/patches.json` : `nds_old_sdk` (fils, numéros des commandes de son, DMA, carte), `overlays`, `eh_end_catch` (pile ARM réelle et registres transmis, `NDS_EH` de `arm2c.py` ; le dérouleur du jeu tourne tel quel). Les autres jeux gardent leur code.
