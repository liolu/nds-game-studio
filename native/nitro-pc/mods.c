/* Mods natifs, essai de l'etape 0 (ROADMAP-MODS.md) : accroches sur toutes les fonctions traduites et chargement de
 * bibliotheques (--plugin a.dll,b.dll ; --no-mods n'en charge aucune). L'interface est celle de nds_mod.h.
 * Les cases des accroches et nds_hook_install sont dans calltab.c. */
#include "nitro.h"

u32 nds_io_read(u32 a, u32 size);
void nds_io_write(u32 a, u32 v, u32 size);

typedef struct {
    u32 size;
    u32 api_major, api_minor;
    const char *game_code;
    void (*log)(const char *fmt, ...);
    int (*hook)(u32 ds_addr, void *replacement, void **original);
    u32 (*read32)(u32 ds_addr);
    void (*write32)(u32 ds_addr, u32 value);
} NdsApi;

/* comme RD32/WR32 du code traduit : les registres passent par io.c, le reste est lu directement */
#define API_IO(a) (((a) >> 24) == 4 || (a) < 0x01000000u || (s32)(a) < 0)
static u32 api_read32(u32 a) { return API_IO(a) ? nds_io_read(a, 4) : REG32(a & ~3u); }
static void api_write32(u32 a, u32 v) { if (API_IO(a)) nds_io_write(a, v, 4); else REG32(a & ~3u) = v; }

static char game_code[5];
static const NdsApi api = {sizeof(NdsApi), 0, 1, game_code, nds_log, nds_hook_install, api_read32, api_write32};

void nds_mods_load(void) {
    const char *list;
    if (nds_arg("--no-mods", 0) || !nds_arg("--plugin", &list) || !list) return;
    nds_copy(game_code, DS(0x027FFE0Cu), 4); /* en-tete de la carte, copie par le BIOS */
    char path[512];
    while (*list) {
        u32 n = 0;
        while (*list && *list != ',' && n < sizeof path - 1) path[n++] = *list++;
        path[n] = 0;
        if (*list == ',') list++;
        void *lib = os_lib_open(path);
        if (!lib) { nds_log("mod refuse : %s ne se charge pas", path); continue; }
        int (*load)(const NdsApi *) = (int (*)(const NdsApi *))os_lib_sym(lib, "nds_mod_load");
        if (!load) { nds_log("mod refuse : %s n'a pas nds_mod_load", path); continue; }
        int r = load(&api);
        nds_log(r ? "mod refuse par lui-meme (%d) : %s" : "mod charge (%d) : %s", r, path);
    }
}
