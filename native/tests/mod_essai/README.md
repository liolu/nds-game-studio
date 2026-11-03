# Mods d'essai de l'étape 0

Ils ne connaissent que `nds_mod.h` (interface 0.1, pas figée) et se chargent avec `--plugin` (`nitro-pc/mods.c`).

- `test_mod.c` : Windows 32 bits, sans bibliothèque C. Accroche `MOD_ADDR` (par défaut `VBlankIntr` de Lego), appelle `original` et écrit dans le journal aux appels 1, 60, 600 et 1200.
- `mod64.c` : Windows 64 bits (`native_build.py --arch x86_64`), avec la bibliothèque C. Accroche `OS_Halt` de Tom et Jerry (0x02051a58).

```bash
gcc -m32 -O2 -fno-builtin -fno-stack-protector -mno-stack-arg-probe -DMOD_NAME='"halt"' -DMOD_ADDR=0x020cea84u -c test_mod.c -o halt.o
ld -m i386pe --dll -e _DllMainCRTStartup@12 --enable-reloc-section --dynamicbase halt.o -o halt.dll
gcc -m64 -O2 -shared mod64.c -o mod64.dll
```
