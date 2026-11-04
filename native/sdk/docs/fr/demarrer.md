# Démarrer : votre premier mod natif

Un mod natif est une petite bibliothèque (`.dll` sous Windows, `.dylib` sur Mac) que le runtime du jeu charge. Elle ne connaît que `sdk/nds_mod.h` : le runtime lui donne une table de fonctions (`nds_api`) pour lire et écrire la mémoire du DS, remplacer n'importe quelle fonction du jeu, suivre les commandes 3D, dessiner sur les images finales, toucher au son, lire le clavier et la souris.

Les joueurs l'installent avec le lien de votre dépôt, depuis l'onglet **Moding** du jeu. Le launcher télécharge l'archive de leur ordinateur dans votre dernière release et la contrôle avec `SHA256SUMS`. Rien n'est compilé chez eux.

## 1. Votre dépôt, depuis le modèle

1. Copiez le dossier `template/` de ce SDK dans un nouveau dépôt (GitHub, public).
2. Dans `nds-mod.json`, changez `id` (lettres, chiffres et `-`), `name`, `name_fr`, `description`, `author`.
3. Renommez `src/my_mod.c` si vous voulez, et les noms des bibliothèques dans `"plugin"`.

`nds-mod.json` :

| Champ | Sens |
|---|---|
| `"format": 2`, `"type": "native"` | Un mod qui apporte du code |
| `"api": "1.0"` | La version de `nds_mod.h` pour laquelle il est fait. Un runtime d'une autre version majeure le refuse, en disant pourquoi |
| `"games"` | Codes des jeux (`["AL7E"]`), ou code → SHA-1 des roms essayées (`{"AL7E": ["7c1a..."]}`). Absent : tous les jeux. Les adresses changent d'une version d'un jeu à l'autre : listez les roms dès que vous en utilisez |
| `"plugin"` | Ordinateur → bibliothèque : `windows-x86`, `macos-arm64`, `macos-x86_64` |
| `"settings"` | Ce que le joueur peut changer dans le launcher : `bool`, `int`, `float` (`min`, `max`, `step`), `choice` (`choices`), `string`, `keybind` (touche par défaut par son nom : `"KeyC"`, `"F9"`). `api->setting(mod, clé)` donne la valeur en texte ; une touche est son code HID (`NDS_KEY_*`) |
| `"presets"` | Des valeurs pour plusieurs réglages d'un coup : `{"name": "...", "values": {...}}` |
| `"instruction_hooks"` | `{"AL7E": ["0x020cea88"]}` : les instructions que le mod accroche avec `api->hook_insn`. Le launcher regénère le jeu avec elles |
| `"multiplayer": "safe"` | Le mod ne change pas la partie : des joueurs avec et sans lui jouent ensemble |
| `"save": "separate"` | Le mod touche à la sauvegarde : il a la sienne |

## 2. Le code

```c
#include "../sdk/nds_util.h"

NDS_MOD_EXPORT int nds_mod_load(const nds_api *api, nds_mod *mod) {
    api->log(mod, "charge pour %s", api->game_code);
    return 0; /* autre chose : refuse, avec api->fail(mod, "pourquoi") */
}
```

- **Remplacer une fonction du jeu** : `api->hook(mod, adresse, ma_fonction, &original)`. Toutes les fonctions du jeu ont la même signature (`nds_fn`) : `r0`-`r3`, puis les 10 premiers mots de la pile ; retour `r0` (bas) et `r1` (haut). Appelez `original` pour faire tourner le code du jeu (ou du mod suivant), ou ne l'appelez pas.
- **Rappels** : `on_frame` (à chaque VBlank), `on_gx` (commandes 3D), `on_polygon` (sommets modifiables), `on_present` (images finales), `on_audio`, `on_io_read`, `on_io_write`, `on_irq`, `on_rom_read`.
- **Mémoire** : toujours par `api->read32`, `api->write32` (et 8, 16 bits) ou `api->memory(adresse)`. Jamais `*(u32 *)0x02100000` : sur un ordinateur 64 bits, la mémoire du DS n'est pas à son adresse DS.
- **Entrées** : `api->input(&in)` donne les boutons du DS, le stylet, la souris et tout le clavier ; `api->take_input` retire les touches au jeu, `api->capture_mouse` retient le curseur (F7 le rend toujours au joueur), `api->notice` affiche un message.
- **Fils** : `on_present` tourne dans le fil de dessin et `on_audio` dans celui du son ; tout le reste dans le fil du jeu.
- **Windows sans bibliothèque C** : le module Windows 32 bits n'en a pas. Écrivez du C simple (ni `printf`, ni `malloc`, ni `sin`) : `sdk/nds_util.h` a sinus, cosinus, arc tangente et la lecture des réglages. Sur Mac, la bibliothèque C est là.

## 3. Compiler et essayer

```bash
./build.sh windows-x86      # Windows : gcc de WinLibs (https://winlibs.com, la version 64 bits, elle compile en -m32) dans le PATH
./build.sh macos-arm64      # Mac
```

Ensuite, [essayer.md](essayer.md) : lancer le jeu en ligne de commande avec votre bibliothèque, ou installer votre dossier dans le launcher (onglet Moding, Ajouter un dossier).

## 4. Publier

Poussez, puis posez une étiquette qui donne la version de `nds-mod.json` :

```bash
git tag v1.0.0
git push --tags
```

`.github/workflows/release.yml` compile pour les trois ordinateurs et publie `<id>-1.0.0-<ordinateur>.zip` avec `SHA256SUMS`. Les joueurs collent le lien de votre dépôt dans l'onglet Moding. Avant l'installation, le launcher affiche « Exécute du code sur cet ordinateur » et la source.

Ensuite : [trouver une adresse](trouver-une-adresse.md), [essayer](essayer.md).
