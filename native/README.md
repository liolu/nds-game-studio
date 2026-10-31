# Jeux DS natifs : traduction + runtime nitro-pc

Un jeu DS tourne ici comme un programme Windows, sans émulateur : pas d'interpréteur ARM au moment de jouer.

```
rom de l'utilisateur ──► chargée en mémoire, aux adresses du DS (données, autoload)
symbols.txt + code ARM ──► tools/arm2c.py ──► C ──► gcc -m32 ──► x86 ┐
nitro-pc/ (registres, BIOS, threads, 2D, 3D, son, sauvegarde) ───────┴─► native/build/<code>/<code>.exe
                                                                          lancé par le launcher (Jouer)
```

## Construire un jeu

```bash
python native/tools/native_build.py "<dossier du projet>"
```

Il faut un projet avec ses symboles SDK nommés (`OS_SaveContext`, `OS_SelectThread`, `FSi_ReadRomCallback`…). Puis
copier `native/build/<code>/<code>.exe` dans le dossier `native` à côté de `nds-game-studio.exe`.

**Les `.exe` contiennent le code du jeu traduit : ils restent sur le PC de l'utilisateur et ne doivent être ni
commités ni publiés.** `native/build/` est ignoré par git.

## Le code vient de l'ARM, pas encore du C décompilé

Par défaut tout le jeu est traduit depuis la rom. `--with-c` compile aussi le C matché, mais ce C compte sur le
compilateur d'origine : appels `f()` qui gardent r0-r3, retours par drapeaux, variables propres à un fichier. Ce mode
plante aujourd'hui sur Tom et Jerry (`func_0203e3f4`). Le C matché est correct pour mwcc, pas forcément pour gcc.

## Limites connues

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
- `native_build.py --instrument` : version de débogage (dernière instruction ARM, registres et derniers appels au plantage).
- `cargo run --release -p nds-engine --example native_check -- <code> <rom> <capture.png> <s> [enter@s] [touch@s]` :
  passe par le vrai moteur, sans focus (`NDSGS_NATIVE_MUTE=1`, `NDSGS_NATIVE_DIR`).

Essayé jusqu'au jeu : Tom et Jerry Tales (AJTP), Rayman Raving Rabbids (AR5E), Lego Star Wars II (AL7E).
