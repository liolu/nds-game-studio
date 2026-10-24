# Nds Game Studio

Moteur de jeux DS natifs pour PC, en Rust avec [Bevy](https://bevy.org) 0.20.
Il fait tourner sur PC des jeux pensés comme des jeux DS : deux écrans, boutons DS, stylet,
2D et 3D. Il n'y a pas d'émulation. Les jeux comptent en pixels DS (256x192), mais le rendu est
natif en qualité maximale : résolution réelle de la fenêtre, anticrénelage MSAA x4, texte net.

## Les deux écrans

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

**F1**, ou le bouton à trois traits à gauche de la barre de titre, ouvre la fenêtre Options :

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
- Les démos s'en servent : Mini Plateforme (pièces ramassées, record de temps, parties finies),
  Bulles Tactiles (5 meilleurs scores), Orbite 3D (dernière vue + 3 photos de vue, X + toucher pour
  en garder une).
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

## Un moteur pour des jeux différents

Chaque jeu a un `game.toml` qui dit ce qu'il attend de la console :

| Clé | Rôle |
| --- | --- |
| `screens.main` | écran du moteur principal (`top` / `bottom`), échangeable en jeu |
| `screens.render` | `hd` (défaut, qualité maximale) ou `pixel` (256x192 agrandi, rendu rétro) |
| `screens.main_mode` / `sub_mode` | `2d` ou `3d` pour chaque moteur (3D avec la 2D par-dessus) |
| `screens.aspect` | format imposé (`4:3`, `16:9`, `16:10`, `fill`) ; sans, celui choisi avec F8 |
| `screens.scale`, `screens.gap` | taille imposée aux fenêtres (sinon celle du joueur), écart entre les écrans fusionnés |
| `screens.main_clear` / `sub_clear` | couleur de fond |
| `input.touch`, `input.keys` | stylet utilisé, touches remplacées |
| `timing.fps` | cadence de la logique (`FixedUpdate`) |

Les trois jeux de démonstration (`app/src/games/`) utilisent tous des réglages différents :

- **Mini Plateforme** : niveau 2D qui défile en haut, carte et score en bas, boutons seulement.
- **Bulles Tactiles** : moteur principal sur l'écran tactile, jeu au stylet, Select échange les écrans.
- **Orbite 3D** : écran du haut en 3D avec du texte 2D par-dessus, pavé tactile en bas, écart entre les écrans.

Ajouter un jeu : un dossier dans `app/src/games/` avec `game.toml` et un plugin qui appelle
`app.add_ds_game(include_str!("game.toml"), setup)`, puis l'ajouter dans `main.rs`.

## Touches

| DS | Clavier (position QWERTY ; en AZERTY : même place) | Manette |
| --- | --- | --- |
| A / B / X / Y | X / Z / S / A | Est / Sud / Nord / Ouest |
| L / R | Q / W | gâchettes hautes |
| Start / Select | Entrée / Maj droite ou Retour arrière | Start / Select |
| Croix | flèches | croix ou stick gauche |
| Stylet | clic gauche sur l'écran du bas | |

Échap (ou Start + Select) revient au menu.

## Compiler et lancer

```bash
build.cmd
```

```bash
run.cmd
```

`run.cmd bubbles` lance directement un jeu (son `id`). Sous Linux/macOS : `./run.sh`.

## Versions et mises à jour

Le système de mise à jour vient de SpaceSpore : `tools/launcher` (fenêtre egui), `tools/updater`,
`tools/installer`, `tools/common`.

- Branche **`experimental`** : on travaille ici. Chaque push compile Windows/Linux/macOS et met à
  jour la pré-release `experimental` (numéro de build). Le launcher la propose en mode « Expérimentale ».
- Branche **`stable`** : pour publier, fusionner `experimental` dans `stable`. La CI crée la
  release `vX.Y.Z` (zips, installeurs, `version.json`).
- Numéro de version : seulement `[workspace.package] version` dans `Cargo.toml`. Si cette version
  existe déjà, la CI augmente le dernier chiffre.

## Pour plus tard

Le dossier [future/](future/README.md) liste les fonctions prévues, par exemple le
[multijoueur](future/multijoueur.md) en réseau local et par Internet, comme dans SpaceSpore.
