# Nds Game Studio

Moteur de jeux DS natifs pour PC, en Rust avec [Bevy](https://bevy.org) 0.20.
Il fait tourner sur PC des jeux pensés comme des jeux DS : deux écrans, boutons DS, stylet,
2D et 3D. Il n'y a pas d'émulation. Les jeux comptent en pixels DS (256x192), mais le rendu est
natif en qualité maximale : résolution réelle de la fenêtre, anticrénelage MSAA x4, texte net.

## Langue

L'interface est en anglais par défaut. **Options > Launcher > Langue** la passe en français, tout de
suite et sans relancer ; le choix est gardé dans `options.toml` (`language = "fr"`). La fenêtre de mise
à jour suit ce choix ; l'installeur est en anglais, puisqu'aucun choix n'existe encore à ce moment-là.
Un texte s'écrit là où il sert, les deux langues côte à côte : `tr("Play", "Jouer")` ou
`t!("{n} games", "{n} jeux")` (voir `crates/engine/src/i18n.rs`).

## Le launcher

La fenêtre du studio s'ouvre sur le launcher, la bibliothèque des jeux de l'utilisateur.

- **Ajouter un jeu** : glisser une rom DS (`.nds`, `.dsi`) dans la fenêtre. Elle est copiée dans le dossier
  `rom`, à côté du launcher (le déposer soi-même dans ce dossier marche aussi). Le nom, l'éditeur et
  l'icône viennent de la rom. Sans rom, aucun jeu n'est montré : seulement un message qui explique ça.
- **A gauche** : le logo, le bouton à 4 carrés (la vue de tous les jeux) et les jeux favoris.
- **Vue des jeux** : en grille ou en liste avec les infos (éditeur, code, région, taille).
- **Page d'un jeu** : son image, son nom, l'étoile des favoris et quatre boutons pour la zone du bas :
  - **Jouer** : lance le module natif du jeu, reconnu par le `game_code` de son `game.toml`. Il n'y a
    pas d'émulation : un jeu devient jouable avec sa décompilation et le runtime (étape 6 de la
    feuille de route de DS Decomp Studio, pas encore écrite). Sans module, le bouton le dit ;
  - **Moding** : la fenêtre Assets (F6) sur la rom de ce jeu et la fenêtre Code (F5) ;
  - **Description** : ce que dit la rom ;
  - **Parametres** : le fichier de la rom, le dossier des roms, retirer le jeu (deux clics, efface le fichier).
- **Options**, toujours en bas à droite : la fenêtre Options (F1), avec une page Launcher (vue, dossier
  des roms, versions). Échap revient à la vue des jeux.
- Favoris et vue sont gardés dans `%APPDATA%\Nds Game Studio\launcher.toml`.

## Les deux écrans

Les écrans se règlent quand un jeu tourne (le launcher tient dans une seule fenêtre).

- **Fusionnés** (au démarrage) : une seule fenêtre montre l'écran du haut et l'écran du bas l'un
  sous l'autre. C'est une fenêtre normale : on la déplace, on l'aimante à une moitié d'écran,
  on la redimensionne par n'importe quel bord, on la réduit.
- **Séparer** : Alt + clic gauche (ou clic molette) sur l'écran du bas, puis glisser. L'écran du
  bas part dans sa propre fenêtre, qui suit la souris.
- **Fusionner** : lâcher la fenêtre de l'écran du bas juste sous l'écran du haut.
- **Bouton fusion** dans la barre de titre, à côté de « réduire » : fusionne ou sépare. La barre de
  titre est dessinée par le studio (glisser pour déplacer, double-clic pour agrandir, bords pour
  redimensionner).
- **F9** fusionne ou sépare aussi. **F8** change le format : remplir la fenêtre, 4:3 (DS), 16:9, 16:10.
  En format large, les jeux voient plus de choses sur les côtés.
- Fermer une des fenêtres quitte le studio.

## Options (F1)

**F1**, le bouton Options du launcher ou le bouton à trois traits de la barre de titre ouvre la fenêtre Options :

- **Launcher** : vue des jeux, dossier des roms, choix de la version et mises à jour.
- **Affichage** : format (remplir, 4:3, 16:9, 16:10), écrans fusionnés ou séparés, plein écran,
  rendu HD ou pixel.
- **Manettes** : les manettes branchées et le modèle reconnu, un menu déroulant avec tous les
  modèles (Xbox One/Series, Xbox 360, PS5 DualSense, PS4 DualShock 4, PS3 DualShock 3, Switch Pro,
  Joy-Con, 8BitDo SN30 Pro, générique) et le dessin de la manette choisie. Une touche pressée sur la
  vraie manette s'allume sur le dessin (les sticks bougent) et son nom s'affiche avec le bouton DS
  qu'elle commande ; cliquer une touche du dessin l'affiche aussi.
- **Clavier** : la touche clavier et la touche manette de chaque bouton DS.
- **Développement** : recompilation automatique oui / non, fenêtre Code.

Ajouter une manette : un fichier `crates/engine/controllers/<id>.toml` (format dans
[FORMAT.md](crates/engine/controllers/FORMAT.md)), puis l'ajouter à la liste de `controllers.rs`.

## Sauvegardes

- **Options** : gardées d'un lancement à l'autre dans `%APPDATA%\Nds Game Studio\options.toml`
  (format, fusion, plein écran, rendu, recompilation auto, modèle de manette choisi, place et taille
  de la fenêtre). Bouton « Réinitialiser les options » dans Options > Affichage.
- **Jeux** : chaque jeu enregistre ce qu'il veut avec la ressource `GameSaves`
  (`saves.save("record", &valeur)`, `saves.load("record")`), dans
  `%APPDATA%\Nds Game Studio\saves\<jeu>\<emplacement>.json`. L'écriture est sûre (fichier
  temporaire puis renommage) et l'ancienne version est gardée en `.bak`, relue si le fichier est abîmé.
- **Options > Sauvegardes** liste les sauvegardes de tous les jeux (taille, date), permet d'en
  supprimer (deux clics) et ouvre le dossier.

## Voir et changer le code en direct

- **F5** ouvre la fenêtre « Code » : un éditeur des fichiers du jeu en cours (un onglet par
  fichier, molette pour défiler, clic pour placer le curseur, copier-coller). **Ctrl+S** enregistre.
  Pendant qu'on tape, les touches du jeu ne réagissent pas. Un fichier changé par un autre éditeur
  est relu s'il n'y a pas de modification en cours.
- **Enregistrer un fichier** du jeu ou du moteur (dans la fenêtre Code ou un autre éditeur) : le studio
  recompile en arrière-plan puis se relance tout seul sur le même jeu, avec les fenêtres à la même
  place et la fenêtre Code rouverte. S'il y a une erreur, elle s'affiche en rouge dans la fenêtre
  Code et le jeu continue avec l'ancien code.
- Marche avec le dépôt sur le PC (compilation locale), pas dans une version installée.

## Assets des jeux (F6)

**F6** (ou Options > Développement, ou l'onglet Moding d'un jeu) ouvre la fenêtre « Assets » : les
fichiers des jeux, pour voir leurs modèles 3D et leurs images et nommer les objets du jeu.

- Les jeux sont les roms du launcher (dossier `rom`) : tout est lu **dans la rom**, sans extraction et
  sans dossier de travail.
- La liste donne le **chemin de chaque fichier dans NitroFS** (celui que le code du jeu charge ;
  `archive.narc#n` = n-ième fichier d'une archive NARC). Les fichiers sont reconnus par leur
  signature (aussi compressés en LZ77). Taper du texte filtre, Retour arrière / Échap efface, la
  molette fait défiler. « Copier le chemin » le met dans le presse-papiers (pour le relier aux
  fonctions du jeu dans DS Decomp Studio).
- **Grille** : le bouton « Grille » montre des vignettes à la place de la liste, dessinées en arrière-plan. Les
  animations `.nanr` et `.nsbca` se jouent dans leur vignette. Clic = aperçu à droite, molette = défiler.
- **Animations** : `.nsbca` jouées sur un modèle du jeu au même squelette (flèches pour en changer ; sans
  modèle lisible dans la rom, ses infos), `.nanr` jouées avec leurs cellules, tuiles et palette.
- **Sons** : `.sad` / `sadl` (IMA et Procyon ADPCM), et dans les archives `.sdat` les musiques `STRM` et les
  banques de sons `SWAR` (listés `sound_data.sdat#n` avec leur nom) : forme d'onde et bouton pour écouter. Les
  séquences `SSEQ` ne se jouent pas (il faudrait un synthétiseur).
- **Vidéos** `.vx` (Actimagine), `.mods` (Mobiclip) et `.bik` (Bink) : image et son, avec le bouton pour jouer, et
  une vignette animée dans la grille. Un Bink sans image (les musiques de Viva Piñata) se montre comme un son.
  Le code de lecture est dans `crates/video`, portage de FFmpeg et de mobipeg, sous LGPL 2.1 ou ultérieure.
- **Polices** `.nftr` (planche des caractères), **messages** `.bmg` (les textes du jeu).
- **Fichiers bruts** sans en-tête, reconnus par leur extension : tuiles `.nbfc`, palettes `.nbfp` / `.ntfp` / `.pal`,
  écrans `.nbfs` / `.scr`. Les fichiers source NITRO-CHARACTER `.ncg`, `.ncl`, `.nsc` s'ouvrent comme les autres images.
- **Moteur maison** de Rayman Raving Rabbids 2, Battle of Giants et des deux Star Wars (fichiers sans extension) :
  textures `LXET` / `TXET` avec la palette qu'elles nomment, palettes `TTLP`, images ` PMB`, textes `GNAL`, modèles
  3D `GEOM` (sans texture) et `MESH` (avec).
- **Compression** : tout fichier compressé par le BIOS de la DS (LZ77, RLE, Huffman) est décompressé, même s'il
  s'appelle `.bin` ; un fichier Nitro dedans s'ouvre normalement. MySims : `NCG`, `NCL`, `NSC` s'ouvrent aussi.
- **Animations de matériaux** `.nsbta`, `.nsbtp`, `.nsbma`, `.nsbva` : leur contenu, pas encore jouées.
- **`.z`** (Tom and Jerry) : images, écrans, textures et maillages du monde (avec leurs textures, listées par le `.xof`) ; le reste (polices, collisions) montre ses octets.
- **Planches de tuiles** : assemblées par leur écran quand il est connu (`.nscr` du même nom, bloc LINK, ou écran qui suit dans une archive sans noms) ; le contrôle Ecran permet de revenir à la planche. Dans une archive sans noms, la palette d'une planche 8 bits est celle qui colore le plus de ses pixels.
- **`.bin` bruts** : `*Tiles.bin`, `*Palette.bin` et les `.bin` d'un dossier `bg` sont lus comme tuiles et palettes, en gris si aucune palette n'a un nom proche.
  **`.anm`** : un résumé, pas jouées.
- **Autres images** : `.bmp`, `.bstga` (MySims), `.ntft` avec sa palette `.ntfp` (taille devinée), palettes `.plb`.
- **Vidéos** `.bik`, `.vx`, `.mods` : leurs caractéristiques, pas lues. **Archives** `.zip` : la liste de leurs fichiers.
- **Autres** : sons CRI `.adx` ; textes `.txt`, `.ini`... ; `.lz` et `.cat` décompressés (ses textes et octets) ; mini-roms
  `.srl` (icône, titre, et leurs fichiers listés `jeu.srl#n`).
- **Nintendogs** : `.bncd` (cellules) et `.bncl` (mise en page sur l'écran).
- **Tables de positions** : un `.dat` ou `.bin` qui commence par une table est découpé en `fichier.dat#n`. Heroes of
  Mana : ses planches de tuiles (palette au début) s'ouvrent, `.dat` compressés entiers compris ; ses sprites
  (`faceshap`, `monstershap`), modèles (`model00.bin`) et `skana.dat` montrent leurs octets.
- **Fichiers maison** (`.dat`, `.bnd`...) : les fichiers Nitro collés dedans sont listés `fichier.dat#n`. Tout
  fichier qu'on ne sait pas décoder (`.sb`, `.scripb`, `.pu`...) montre ses textes (ASCII et Shift-JIS) et
  ses octets ; la molette fait défiler.
- Formats Nintendo standard : modèles `.nsbmd` en 3D (glisser = tourner, clic droit = déplacer,
  molette = zoom) avec leurs textures (celles du fichier, ou le `.nsbtx` qui en donne le plus),
  textures `.nsbtx`, palettes `.nclr`, tuiles `.ncgr` / `.ncbr`, écrans `.nscr`, cellules `.ncer`.
  La palette et les tuiles qui vont avec sont devinées (même dossier, nom le plus proche) et se
  changent avec les flèches `<` `>`. Les formats maison (Traveller's Tales, EA...) ne sont pas décodés.
- **rom.bin** (Lego Star Wars II, Les Sims 2...) : les fichiers de l'archive numérotée, lus et
  dépaquetés depuis la rom, sont listés `rom.bin#n` (filtre « rom.bin »). « Copier le chemin » copie
  alors le numéro n seul, la clé de DS Decomp Studio (onglet Data, « Archive (rom.bin) »). Un modèle
  sans textures prend le `.nsbtx` au numéro le plus proche qui les a (flèches pour en changer, ou
  « celles du modele » = sans texture). Le codeur propre à ces jeux (sorte 6) est réécrit en Rust
  (`crates/nitro/src/romarc.rs`), rien à exécuter.
- **Menu des jeux** : de haut en bas, avec une recherche (taper du texte, Entree = premier resultat,
  Echap = fermer) et deux pourcentages. *Extrait* = part des octets de donnees lisibles fichier par
  fichier : ne comptent pas comme extraits un `rom.bin` qui n'est pas une table de fichiers, un `.narc` illisible, et tout
  gros fichier (4 Mo et plus) de signature inconnue, comme `Glob.bin` ; le son et les videos ne
  comptent pas. *Regardable* = part des fichiers (ceux des archives compris) que l'app sait montrer.
- **Menu des formats** : toutes les extensions des jeux, avec une coche : `[x]` regardable, `[~]` en
  partie, `[!]` l'essai a echoue, `[ ]` pas dans l'app. Un fichier de chaque format de chaque jeu est
  vraiment decode pour le dire. Clic = ne lister que ce format dans le jeu ouvert.
- Ces chiffres sont calcules en arriere-plan (une seule fois par rom) et gardes dans
  `assets_index.json` ; un jeu est refait quand sa rom change. Essai sans fenetre :
  `cargo run --release -p nds-engine --example asset_audit -- <dossier des roms> <sortie.tsv> [--fresh]`.
- Lancer directement : `nds-game-studio --assets=<rom ou titre> --asset=<chemin>`.
- Lecture des formats : crate `crates/nitro` (la 3D reprend le code
  d'[apicula](https://github.com/scurest/apicula), licence 0BSD).

## Un moteur pour des jeux différents

Chaque jeu a un `game.toml` qui dit ce qu'il attend de la console :

| Clé | Rôle |
| --- | --- |
| `game_code` | code de la rom de ce jeu (`AQIE`) : le bouton Jouer du launcher lance ce jeu pour cette rom, qu'il reçoit dans `ActiveRom` |
| `screens.main` | écran du moteur principal (`top` / `bottom`), échangeable en jeu |
| `screens.render` | `hd` (défaut, qualité maximale) ou `pixel` (256x192 agrandi, rendu rétro) |
| `screens.main_mode` / `sub_mode` | `2d` ou `3d` pour chaque moteur (3D avec la 2D par-dessus) |
| `screens.aspect` | format imposé (`4:3`, `16:9`, `16:10`, `fill`) ; sans, celui choisi avec F8 |
| `screens.scale`, `screens.gap` | taille imposée aux fenêtres (sinon celle du joueur), écart entre les écrans fusionnés |
| `screens.main_clear` / `sub_clear` | couleur de fond |
| `input.touch`, `input.keys` | stylet utilisé, touches remplacées |
| `timing.fps` | cadence de la logique (`FixedUpdate`) |

Ajouter un module de jeu : un dossier dans `app/src/games/` avec `game.toml` et un plugin qui appelle
`app.add_ds_game(include_str!("game.toml"), setup)`, puis l'ajouter dans `main.rs`.

## Touches

| DS | Clavier (position QWERTY ; en AZERTY : même place) | Manette |
| --- | --- | --- |
| A / B / X / Y | X / Z / S / A | Est / Sud / Nord / Ouest |
| L / R | Q / W | gâchettes hautes |
| Start / Select | Entrée / Maj droite ou Retour arrière | Start / Select |
| Croix | flèches | croix ou stick gauche |
| Stylet | clic gauche sur l'écran du bas | |

Échap (ou Start + Select) revient au launcher.

## Compiler et lancer

```bash
build.cmd
```

```bash
run.cmd
```

`run.cmd <id>` lance directement un jeu enregistré. Sous Linux/macOS : `./run.sh`.

## Versions et mises à jour

Le système de mise à jour vient de SpaceSpore : `tools/launcher` (fenêtre egui), `tools/updater`,
`tools/installer`, `tools/common`. S'il n'y a rien à installer, `tools/launcher` ouvre tout de suite le
studio ; pour choisir une version : Options > Launcher (il se rouvre avec `--versions`).

- Branche **`experimental`** : on travaille ici. Chaque push compile Windows/Linux/macOS et met à
  jour la pré-release `experimental` (numéro de build). Le launcher la propose en mode « Expérimentale ».
- Branche **`stable`** : pour publier, fusionner `experimental` dans `stable`. La CI crée la
  release `vX.Y.Z` (zips, installeurs, `version.json`).
- Numéro de version : seulement `[workspace.package] version` dans `Cargo.toml`. Si cette version
  existe déjà, la CI augmente le dernier chiffre.

## Pour plus tard

Le dossier [future/](future/README.md) liste les fonctions prévues, par exemple le
[multijoueur](future/multijoueur.md) en réseau local et par Internet, comme dans SpaceSpore.
