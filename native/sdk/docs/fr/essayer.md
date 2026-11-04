# Essayer votre mod

## Dans le launcher

Onglet Moding du jeu → **Ajouter un dossier** → le dossier du mod (avec `nds-mod.json` et la bibliothèque compilée). Ses réglages apparaissent sur sa ligne. Après une partie, la ligne dit si le runtime l'a chargé, ou pourquoi pas.

## En ligne de commande, sans le launcher

Le launcher garde le module généré du jeu dans son dossier de données : `%APPDATA%\Nds Game Studio\native\` sous Windows, `~/Library/Application Support/Nds Game Studio/native/` sur Mac (`AL7E-r2.exe` par exemple). Lancez-le avec votre rom :

```bash
AL7E-r2.exe "rom.nds" --plugin lib/windows-x86_64/my_mod.dll --mute --frames 1200 --dump captures --every 300
```

- `--plugin a.dll,b.dll` : des bibliothèques chargées sans manifeste (pas de réglages). `--plugins liste.json` : une liste comme celle du launcher, avec les réglages ; `status.json` est écrit à côté.
- `--no-mods` : aucun mod, pour comparer.
- `--frames n` : arrêt après n images. `--dump dossier --every n` (`--dumpfrom n`) : images BMP des deux écrans.
- `--fixed-clock` : les mêmes images à chaque passage, aussi vite que l'ordinateur le permet. Pour comparer avec et sans votre mod.
- `--keys "image:masque[/x/y],..."` : boutons du DS (A=1, B=2, Select=4, Start=8, droite=16, gauche=32, haut=64, bas=128, R=256, L=512, X=1024, Y=2048) et stylet.
- `--hid "image:code,..."` : une touche du clavier entier (code HID, `NDS_KEY_*`), 0 = relâchée.
- `--look "image:x:y,..."` : la souris (cumul de ses mouvements).
- `--peek "image:adresse:taille:fichier,..."` : copie de la mémoire du DS dans un fichier. `--poke "image:adresse:valeur,..."` : écrit un mot.
- `--lang fr|en` : la langue que le mod reçoit dans `api->language`.

Le journal (la console, ou `saves/native-<code>/runtime.log` du dossier de données après une partie lancée par le launcher) a chaque ligne de `api->log`, la raison d'un refus, et les mods chargés si le jeu plante.

## Avant de publier

- Lancez avec `--no-mods` puis avec votre mod : seul ce que vous vouliez doit changer.
- Essayez un autre jeu si votre mod n'a pas de `"games"` : le runtime ne refuse que les mods d'autres jeux, pas ceux qui se comportent mal.
- En multi, les joueurs doivent avoir les mêmes mods, sauf si le vôtre dit `"multiplayer": "safe"`.
