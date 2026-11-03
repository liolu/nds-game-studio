/* Mod d'essai de l'etape 1 : accroche une fonction puis se refuse lui-meme. Le runtime doit retirer l'accroche,
 * dire pourquoi dans status.json, et le jeu doit demarrer sans lui. */
#include "../../sdk/nds_mod.h"

static nds_fn original;

static nds_u64 broken(nds_u32 r0, nds_u32 r1, nds_u32 r2, nds_u32 r3, nds_u32 a0, nds_u32 a1, nds_u32 a2, nds_u32 a3,
                      nds_u32 a4, nds_u32 a5, nds_u32 a6, nds_u32 a7, nds_u32 a8, nds_u32 a9) {
    for (;;) {} /* ne doit jamais tourner */
}

NDS_MOD_EXPORT int nds_mod_load(const nds_api *api, nds_mod *mod) {
    api->hook(mod, 0x020cea84u, (void *)broken, (void **)&original);
    api->fail(mod, "essai : ce mod se refuse lui-meme");
    return 1;
}

#ifdef _WIN32
int __stdcall DllMainCRTStartup(void *inst, nds_u32 reason, void *reserved) { return 1; }
#endif
