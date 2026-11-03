/* Mod d'essai 64 bits : bibliotheque C complete (snprintf, malloc), fichier ecrit depuis la pile d'un fil du jeu */
#include <stdio.h>
#include <stdlib.h>
#include "nds_mod.h"

static const nds_api *api;
static nds_fn original;
static unsigned calls;
static char *buf;

static u64 on_halt(u32 r0, u32 r1, u32 r2, u32 r3, u32 a0, u32 a1, u32 a2, u32 a3, u32 a4, u32 a5, u32 a6, u32 a7, u32 a8, u32 a9) {
    volatile double x = calls * 0.5;
    if (++calls == 600) {
        snprintf(buf, 256, "mod64 : OS_Halt appel %u, pile %p, double %.1f", calls, (void *)&x, x);
        api->log("%s", buf);
        FILE *f = fopen("mod64.txt", "w");
        if (f) { fputs(buf, f); fclose(f); }
    }
    return original(r0, r1, r2, r3, a0, a1, a2, a3, a4, a5, a6, a7, a8, a9);
}

__declspec(dllexport) int nds_mod_load(const nds_api *a) {
    api = a;
    buf = malloc(256);
    a->log("mod64 : charge pour %s", a->game_code);
    return a->hook(0x02051a58u, (void *)on_halt, (void **)&original);
}
