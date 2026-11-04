# Mods d'essai

Ils ne connaissent que `native/sdk/nds_mod.h` (interface 1.0) et se compilent à part, sans le jeu ni le runtime.

- `essai_v1.c` : pour Lego (AL7E). Accroche `OS_Halt` et une de ses instructions (module généré avec `--insn-hooks 0x020cea88`), se sert de tous les rappels, dessine un carré sur l'écran du haut (réglage `couleur` : `vert` ou rouge par défaut) et écrit un bilan à l'image 600. Le réglage `planter` (un numéro d'image) le fait planter, pour vérifier que le journal le nomme.
- `refus_v1.c` : pose une accroche puis se refuse avec `api->fail`. Le runtime retire l'accroche et le jeu démarre sans lui.

```bash
# Windows 64 bits (le module des jeux depuis l'etape 6), sans bibliotheque C
gcc -m64 -O2 -shared -nostdlib -fno-builtin -fno-stack-protector -mno-stack-arg-probe -Wl,-e,DllMainCRTStartup essai_v1.c -o essai_v1.dll
# un module 32 bits (native_build.py sans --arch)
gcc -m32 -O2 -fno-builtin -fno-stack-protector -mno-stack-arg-probe -c essai_v1.c -o essai_v1.o
ld -m i386pe --dll -e _DllMainCRTStartup@12 --enable-reloc-section --dynamicbase essai_v1.o -o essai_v1.dll
```

Lancement : `AL7E.exe <rom> --plugins plugins.json` ; l'état de chaque mod est écrit dans `status.json`, à côté.
