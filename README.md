# Nds Game Studio

Moteur de jeux DS natifs pour PC, en Rust avec [Bevy](https://bevy.org) 0.20.
Il fait tourner sur PC des jeux pensés comme des jeux DS : deux écrans 256x192, boutons DS,
stylet, 2D et 3D. Il n'y a pas d'émulation.

## Les deux écrans

- L'écran du haut et l'écran du bas sont deux fenêtres.
- **Liés** (au démarrage) : l'écran du bas n'a pas de barre de titre et reste collé sous l'écran
  du haut. Quand on déplace ou redimensionne l'écran du haut, l'écran du bas suit.
- **Séparer** : clic molette, ou Alt + clic gauche, sur l'écran du bas, puis glisser.
- **Lier** : glisser l'écran du bas contre le bas de l'écran du haut et lâcher : il s'aimante.
- **F9** lie ou sépare sans la souris. Fermer une des deux fenêtres quitte le studio.

## Un moteur pour des jeux différents

Chaque jeu a un `game.toml` qui dit ce qu'il attend de la console :

| Clé | Rôle |
| --- | --- |
| `screens.main` | écran du moteur principal (`top` / `bottom`), échangeable en jeu |
| `screens.main_mode` / `sub_mode` | `2d` ou `3d` pour chaque moteur (3D avec la 2D par-dessus) |
| `screens.scale`, `screens.gap` | taille des fenêtres, écart entre les écrans liés |
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
