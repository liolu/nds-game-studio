# Jeux DS natifs : traduction + runtime nitro-pc

Un jeu DS tourne ici comme un programme Windows, sans émulateur : pas d'interpréteur ARM au moment de jouer.

```
rom de l'utilisateur ──► chargée en mémoire, aux adresses du DS (données, autoload)
symbols.txt + code ARM ──► tools/arm2c.py ──► C ──► gcc -m32 ──► x86 ┐
nitro-pc/ (registres, BIOS, threads, 2D, 3D, son, sauvegarde) ───────┴─► native/build/<code>/<code>.exe
                                                                          lancé par le launcher (Jouer)
```

## Chez le joueur : le launcher génère le module

Les outils sont livrés avec le studio (`native-toolchain` à côté de l'exécutable, 100 Mo). Dès qu'il voit une rom, le
launcher prépare son module en tâche de fond, un jeu à la fois et jamais pendant une partie : il vérifie le SHA-1 de la
rom, traduit, compile (de 30 s à 1 min 30 selon le jeu) et garde le résultat dans `native/<code>-<n>.exe` du dossier des
données. Le bouton Jouer s'active quand c'est prêt. Les jeux expérimentaux ne se préparent que sur demande (bouton Préparer).

Sans ce dossier (compilation locale), la boîte est téléchargée une fois (release `native-toolchain`).

**Rien de publié ne contient de code de jeu.** La boîte à outils contient le traducteur, le runtime, GCC, Python et les
recettes `games/<code>.json.gz` (positions des fonctions, pointeurs et noms, pas d'instructions). Les modules, eux,
contiennent le code du jeu : ils ne doivent être ni commités ni publiés. `native/build/` est ignoré par git.

Quand le runtime, le traducteur ou les recettes changent, augmenter `TOOLCHAIN` dans `tools/export_recipes.py`, relancer
ce script : le numéro passe dans `games/index.json`, la CI publie la nouvelle boîte, et les modules sont régénérés.

Une rom d'une autre version que celle de la recette est refusée. Un `native/<code>.exe` à côté du studio passe avant
le module généré ; `NDSGS_TOOLCHAIN_ZIP` donne une boîte locale au lieu de la télécharger.

## Pour le développement

```bash
python native/tools/native_build.py "<dossier du projet>"       # un jeu, depuis son projet
python native/tools/export_recipes.py "<projets>" "<roms>"      # les recettes et l'index
python native/tools/build_all.py "<projets>" "<roms>"           # essaie tous les jeux, écrit GAMES.md
python native/tools/make_toolchain.py --gcc ... --python ... --capstone ... --out x.zip
```

Il faut un jeu avec ses symboles SDK nommés (`OS_SaveContext`, `OS_SelectThread`, `FSi_ReadRomCallback`…) pour qu'il
se lance : les autres sont proposés comme expérimentaux, voir [GAMES.md](GAMES.md).

## Le code vient de l'ARM, pas encore du C décompilé

Par défaut tout le jeu est traduit depuis la rom. `--with-c` compile aussi le C matché, mais ce C compte sur le
compilateur d'origine : appels `f()` qui gardent r0-r3, retours par drapeaux, variables propres à un fichier. Ce mode
plante aujourd'hui sur Tom et Jerry (`func_0203e3f4`). Le C matché est correct pour mwcc, pas forcément pour gcc.

## Limites connues

- Lego Star Wars II : parasites sur l'écran tactile pendant le jeu. La carte de la couche BG1 du moteur secondaire contient
  des valeurs aléatoires, sans écriture visible du code traduit (`--watch`) : cause pas encore trouvée.
- Un jeu sans symboles SDK nommés ne se lance pas encore ; les overlays ne sont pas gérés.
- 3D : pas d'ombres (polygones mode 3), de brouillard, de contours ni de table de toon ; polygones triés seulement
  opaques puis transparents.
- 2D : pas de mosaïque ni de capture d'écran ; effets ligne par ligne absents, l'image est dessinée entière au VBlank.
- Pas de préemption : un thread passe la main sur une attente, une interruption ou un accès registre.
- Son : séquenceur et mixeur simplifiés (portamento et valeurs aléatoires du SSEQ ignorés).
- Wi-Fi et micro absents. Pas de cartouche GBA.

## Essais sans fenêtre

```bash
bash native/tools/run_game.sh AJTP "<rom>" 60 --mute --save test.sav --dump captures --every 300 --frames 3000 --keys "2700:0/128/150,2710:0"
```

- `--keys "image:masque[/x/y],..."` : touches (bits de KEYINPUT : A=1, B=2, Select=4, Start=8, droite=16, gauche=32,
  haut=64, bas=128, R=256, L=512, X=1024, Y=2048) et stylet.
- `--trace`, `--prof` : état et profil ; `--wav fichier` : le son mixé en PCM brut.
- `native_build.py --instrument` : version de débogage (dernière instruction ARM, registres et derniers appels au plantage) ;
  `--watch adresse` y montre qui écrit à une adresse.
- `cargo run --release -p nds-engine --example native_check -- <code> <rom> <capture.png> <s> [enter@s] [touch@s]` :
  passe par le vrai moteur, sans focus (`NDSGS_NATIVE_MUTE=1`, `NDSGS_NATIVE_DIR`).

Essayé jusqu'au jeu : Tom et Jerry Tales (AJTP), Rayman Raving Rabbids (AR5E), Lego Star Wars II (AL7E).
