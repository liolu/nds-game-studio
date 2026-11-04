/* Mod modele de Nds Game Studio : marche sur tous les jeux, sans adresse. Une touche (reglable dans le launcher) affiche ou cache
 * un petit compteur d'images en haut a gauche de l'ecran du haut, et un message le dit.
 *
 * A partir d'ici : changez l'id et le nom dans nds-mod.json, puis le code. Les guides du SDK (docs/) disent comment trouver une
 * fonction ou une variable du jeu et l'accrocher (api->hook, voir l'exemple en bas). */
#include "../sdk/nds_util.h"

static const nds_api *api;
static nds_mod *self;
static int key_toggle = NDS_KEY_F9;
static volatile int shown, frames;
static nds_u32 color = NDS_RGB(0x40, 0xC0, 0xFF);

/* fil du jeu, a chaque image : les touches */
static void each_frame(void *user, nds_u32 frame) {
    static int was_down;
    nds_input in;
    in.size = sizeof in;
    api->input(&in);
    int down = NDS_KEY_DOWN(&in, key_toggle);
    if (down && !was_down) {
        shown = !shown;
        api->notice(self, shown ? (nds_lang_fr(api) ? "Compteur affiche" : "Counter shown") : (nds_lang_fr(api) ? "Compteur cache" : "Counter hidden"), 1500);
    }
    was_down = down;
    frames = (int)frame;
}

/* fil de dessin : les images finales, que le mod peut modifier (NDS_RGB) */
static void draw(void *user, nds_u32 *top, nds_u32 *bottom, int w, int h, int stride) {
    if (!shown) return;
    int k = h / 192 > 0 ? h / 192 : 1; /* pixels de l'image par pixel du DS */
    int bits = frames;
    for (int b = 0; b < 16; b++, bits >>= 1)
        for (int y = 2 * k; y < 6 * k; y++)
            for (int x = (2 + b * 5) * k; x < (6 + b * 5) * k && x < w; x++) top[y * stride + x] = (bits & 1) ? color : NDS_RGB(0x20, 0x20, 0x20);
}

NDS_MOD_EXPORT int nds_mod_load(const nds_api *a, nds_mod *mod) {
    api = a;
    self = mod;
    key_toggle = nds_setting_int(a, mod, "toggle", NDS_KEY_F9);
    const char *c = a->setting(mod, "color");
    if (c && c[0] == 'g') color = NDS_RGB(0x40, 0xFF, 0x40);
    a->log(mod, "charge pour %s (%s), interface %u.%u", a->game_code, a->platform, a->major, a->minor);
    a->on_frame(mod, each_frame, 0);
    a->on_present(mod, draw, 0);
    return 0;
}

/* Exemple d'accroche (a adapter : l'adresse est celle d'une fonction du jeu vise, par exemple 0x020ce6c8 pour OS_GetTick
 * de Lego Star Wars II en version USA) :
 *
 *   static nds_fn original;
 *   static nds_u64 my_tick(nds_u32 r0, nds_u32 r1, nds_u32 r2, nds_u32 r3, nds_u32 a0, nds_u32 a1, nds_u32 a2, nds_u32 a3,
 *                          nds_u32 a4, nds_u32 a5, nds_u32 a6, nds_u32 a7, nds_u32 a8, nds_u32 a9) {
 *       nds_u64 r = original(r0, r1, r2, r3, a0, a1, a2, a3, a4, a5, a6, a7, a8, a9);  // le jeu, puis le mod
 *       return r;
 *   }
 *   ...dans nds_mod_load : if (a->hook(mod, 0x020ce6c8u, (void *)my_tick, (void **)&original)) return 1;
 */
