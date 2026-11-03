/* Mod d'essai de l'etape 0 : ne connait que nds_mod.h. Accroche VBlankIntr de Lego (0x02001464), appelle l'original,
 * ecrit dans le journal a quelques images et montre la pile sur laquelle il tourne. */
#include "nds_mod.h"

#ifndef MOD_ADDR
#define MOD_ADDR 0x02001464u
#endif
#ifndef MOD_NAME
#define MOD_NAME "essai"
#endif

static const nds_api *api;
static nds_fn original;
static u32 calls;

static u64 on_vblank(u32 r0, u32 r1, u32 r2, u32 r3, u32 a0, u32 a1, u32 a2, u32 a3, u32 a4, u32 a5, u32 a6, u32 a7, u32 a8, u32 a9) {
    volatile u32 local = 0;
    calls++;
    if (calls == 1 || calls == 60 || calls == 600 || calls == 1200)
        api->log("mod %s : appel %u, pile %x, code du jeu lu en memoire %x", MOD_NAME, calls, (u32)(uptr)&local, api->read32(0x027FFE0Cu));
    return original(r0, r1, r2, r3, a0, a1, a2, a3, a4, a5, a6, a7, a8, a9);
}

NDS_MOD_EXPORT int nds_mod_load(const nds_api *a) {
    api = a;
    if (a->api_major != NDS_API_MAJOR) return -1;
    a->log("mod %s : charge pour %s, interface %u.%u", MOD_NAME, a->game_code, a->api_major, a->api_minor);
    if (a->hook(MOD_ADDR, (void *)on_vblank, (void **)&original)) {
        a->log("mod %s : accroche refusee", MOD_NAME);
        return -2;
    }
    return 0;
}

#ifdef _WIN32
/* sans bibliotheque C : le point d'entree de la DLL ne fait rien */
int __stdcall DllMainCRTStartup(void *inst, u32 reason, void *reserved) { return 1; }
#endif
