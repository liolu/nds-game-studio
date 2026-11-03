/* Mod d'essai de l'etape 1 : ne connait que native/sdk/nds_mod.h, compile a part (Windows 32 bits sans bibliotheque C).
 * Pour Lego (AL7E) : accroche OS_Halt et une de ses instructions, se sert de tous les rappels, dessine un carre sur
 * l'ecran du haut, et ecrit un bilan dans le journal a l'image 600. */
#include "../../sdk/nds_mod.h"

#define OS_HALT 0x020cea84u
#define OS_HALT_INSN 0x020cea88u /* le module doit etre genere avec --insn-hooks 0x020cea88 */

static const nds_api *api;
static nds_mod *self;
static nds_fn halt_original;
static nds_u32 halts, insns, gx, polys, audio_frames, irqs, io_writes, rom_bytes;
static nds_u32 color = 0xFFFF0000u;

static nds_u64 on_halt(nds_u32 r0, nds_u32 r1, nds_u32 r2, nds_u32 r3, nds_u32 a0, nds_u32 a1, nds_u32 a2, nds_u32 a3,
                       nds_u32 a4, nds_u32 a5, nds_u32 a6, nds_u32 a7, nds_u32 a8, nds_u32 a9) {
    halts++;
    return halt_original(r0, r1, r2, r3, a0, a1, a2, a3, a4, a5, a6, a7, a8, a9);
}

static void on_insn(void *user, nds_regs *regs) { insns++; }
static int count_gx(void *user, nds_u32 cmd, nds_u32 *params, nds_u32 count) { gx++; return 0; }
static int count_poly(void *user, nds_vertex *v, int n, nds_u32 attr, nds_u32 tex, nds_u32 pal) { polys++; return 0; }
static void count_audio(void *user, nds_s16 *stereo, nds_u32 frames) { audio_frames += frames; }
static void count_irq(void *user, nds_u32 bits) { irqs++; }
static int count_io(void *user, nds_u32 addr, nds_u32 size, nds_u32 *value) { io_writes++; return 0; }
static void count_rom(void *user, nds_u32 pos, nds_u32 size, nds_u8 *dst) { rom_bytes += size; }

static void draw(void *user, nds_u32 *top, nds_u32 *bottom, int w, int h, int stride) {
    for (int y = 4; y < 20; y++)
        for (int x = 4; x < 20; x++) top[y * stride + x] = color;
}

static int crash_at;

static void frame(void *user, nds_u32 n) {
    if (crash_at && n == (nds_u32)crash_at) *(volatile int *)0 = 1; /* reglage "planter" : le journal doit nommer ce mod */
    if (n != 600) return;
    nds_u32 *runtime_frame = api->runtime("nds_frame");
    nds_input in;
    in.size = sizeof in;
    api->input(&in);
    api->log(self, "image %u (runtime : %u) : OS_Halt %u, instruction %u, commandes 3D %u, polygones %u, son %u, interruptions %u, registres ecrits %u, rom lue %u octets, touches %x",
             n, runtime_frame ? *runtime_frame : 0, halts, insns, gx, polys, audio_frames, irqs, io_writes, rom_bytes, in.ds_keys);
    api->log(self, "code du jeu en memoire : %x, fonction OS_Halt appelable : %s, nom absent : %s", api->read32(0x027FFE0Cu),
             api->function_at(OS_HALT) ? "oui" : "non", api->runtime("pas_de_nom_pareil") ? "trouve" : "0");
}

NDS_MOD_EXPORT int nds_mod_load(const nds_api *a, nds_mod *mod) {
    api = a;
    self = mod;
    const char *c = a->setting(mod, "couleur");
    if (c && c[0] == 'v') color = 0xFF00FF00u;
    for (const char *p = a->setting(mod, "planter"); p && *p >= '0' && *p <= '9'; p++) crash_at = crash_at * 10 + *p - '0';
    a->log(mod, "charge pour %s (%s), interface %u.%u, reglage couleur = %s", a->game_code, a->platform, a->major, a->minor, c ? c : "absent");
    if (a->hook(mod, OS_HALT, (void *)on_halt, (void **)&halt_original)) return 1;
    if (a->hook_insn(mod, OS_HALT_INSN, on_insn, 0)) a->log(mod, "sans accroche a l'instruction");
    a->on_frame(mod, frame, 0);
    a->on_gx(mod, count_gx, 0);
    a->on_polygon(mod, count_poly, 0);
    a->on_present(mod, draw, 0);
    a->on_audio(mod, count_audio, 0);
    a->on_irq(mod, count_irq, 0);
    a->on_io_write(mod, count_io, 0);
    a->on_rom_read(mod, count_rom, 0);
    return 0;
}

#ifdef _WIN32
int __stdcall DllMainCRTStartup(void *inst, nds_u32 reason, void *reserved) { return 1; }
#endif
