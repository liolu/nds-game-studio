# Mods en script (Luau)

Un mod en script ne demande ni compilateur ni release : le dépôt est le mod. Les joueurs collent son lien dans l'onglet Moding ; le launcher télécharge une fois l'hôte des scripts (à l'installation du premier mod en script) et fait tourner votre script [Luau](https://luau.org) dans le jeu.

```
mon-mod-en-script/
  nds-mod.json
  scripts/main.luau
```

```json
{ "format": 2, "id": "skip-intro-script", "version": "1.0.0", "type": "script", "api": "1.0",
  "script": "scripts/main.luau", "games": { "AL7E": ["<SHA-1 de la rom>"] } }
```

`template-script/` dans ce SDK en est un complet (Lego Star Wars II démarre directement sur son menu).

## La table `nds`

Les mêmes accès qu'un mod natif (`nds_mod.h`), avec des nombres au lieu de pointeurs :

| | |
|---|---|
| `nds.read8/16/32(adresse)`, `nds.write8/16/32(adresse, valeur)` | mémoire et registres du DS |
| `nds.call(adresse, r0, r1, r2, r3)` | appelle une fonction du jeu ; rend `r0, r1` |
| `nds.hook(adresse, function(r0, r1, r2, r3, original) ... end)` | remplace une fonction du jeu ; `original(r0, r1, r2, r3)` fait tourner le code du jeu (ou du mod suivant) et rend `r0, r1` ; rendez `r0, r1` |
| `nds.hook_insn(adresse, function(regs) ... end)` | avant une instruction (déclarée dans `"instruction_hooks"`) ; `regs.r0` ... `regs.r15`, `regs.cpsr` modifiables |
| `nds.on_frame(function(image))`, `nds.on_irq(function(bits))` | à chaque VBlank, à chaque interruption |
| `nds.on_gx(function(cmd, params) -> enlever)`, `nds.on_polygon(function(sommets, attr, tex, pal) -> enlever)` | 3D : changez `params[i]` ou `sommets[i].x` (`x y z w r g b s t`), rendez `true` pour enlever |
| `nds.on_present(function(w, h))` + `nds.pixel(ecran, x, y[, couleur])` | les images finales (écran 0 = haut, 1 = bas) ; une couleur est `0xAABBGGRR` |
| `nds.on_audio(function(echantillons))` + `nds.sample(i[, gauche, droite])` | le son mixé |
| `nds.on_io_read(function(adresse, taille, valeur) -> valeur)`, `nds.on_io_write(function(adresse, taille, valeur) -> valeur ou false)` | registres |
| `nds.on_rom_read(function(position, taille))` + `nds.rom_byte(i[, valeur])` | ce que le jeu lit de la rom |
| `nds.input()` → `{ds_keys, touch_down, touch_x, touch_y, mouse_x, mouse_y, mouse_buttons, wheel, cursor_held}`, `nds.key_down(code)` | entrées ; les codes de touches sont les `NDS_KEY_*` de `nds_mod.h` (HID) |
| `nds.take_input(oui)`, `nds.capture_mouse(oui)`, `nds.notice(texte[, ms])`, `nds.log(...)` | |
| `nds.setting(clé)`, `nds.frame()`, `nds.game_code`, `nds.language`, `nds.platform` | |

## Bac à sable et permissions

Le script tourne dans un bac à sable : les bibliothèques standard de Luau sont en lecture seule, et il n'a accès ni aux fichiers, ni au réseau, ni au système. Un mod qui a besoin de plus le dit dans `nds-mod.json` :

```json
"permissions": ["files"]
```

Le launcher l'affiche avant l'installation, et installer l'accepte. Avec `files`, `nds.read_file(chemin)` et `nds.write_file(chemin, texte)` marchent. `network` et `system` ne sont pas encore pris en charge par cet hôte : un mod qui les demande est refusé, avec la raison.

## Pas de limite de temps, sauf une boucle infinie

Un script peut prendre le temps qu'il lui faut. Seul un rappel qui ne rend pas la main en 10 secondes est arrêté : le script est retiré du jeu, le jeu continue, et la ligne du mod dans le launcher dit pourquoi. Une erreur dans le script fait de même, avec son message. Utilisez `pcall` pour gérer vos propres erreurs.

La plupart des rappels tournent dans le fil du jeu, mais `on_present` et `on_audio` viennent des fils de dessin et de son : l'hôte les fait s'attendre, donc un `on_present` lent ralentit le jeu.
