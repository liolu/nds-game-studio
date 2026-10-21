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
- **F9** fusionne ou sépare. **F8** change le format : remplir la fenêtre, 4:3 (DS), 16:9, 16:10.
  En format large, les jeux voient plus de choses sur les côtés.
- Fermer une des fenêtres quitte le studio.

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
