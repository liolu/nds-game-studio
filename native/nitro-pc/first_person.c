/* --first-person : la camera du jeu devient les yeux du joueur.
 *
 * Le jeu construit sa matrice de vue depuis une position et trois axes. native/games/patches.json dit, pour les jeux qui le permettent, ou sont
 * cette camera et le personnage du joueur ; une accroche a l'entree de la fonction qui calcule la vue remplace ces valeurs juste avant le calcul. */
#include "nitro.h"

/* tables.c, d'apres patches.json ; 0 pour les autres jeux */
extern const u32 nds_fp_camera, nds_fp_players, nds_fp_index, nds_fp_matrix;

#define CAM_FLAGS 0x160
#define CAM_ROTATION 0x14
#define VIEW_DIRTY 0xCu /* la vue et le volume de vue sont a refaire */
#define EYE_HEIGHT 4.1f
#define EYE_FORWARD 0.9f
#define TURN_SMOOTHING 0.3f

static inline float root(float v) {
    float r;
    __asm__("sqrtss %1, %0" : "=x"(r) : "x"(v));
    return r;
}

static inline s32 fx(float v) { return (s32)(v * 4096.0f); }
static inline float real(s32 v) { return (float)v / 4096.0f; }

void nds_hook_camera(u32 camera) {
    static int active = -1;
    static float look_x, look_z;
    static u32 last_frame;
    if (active < 0) active = nds_fp_camera && nds_arg("--first-person", 0);
    if (!active || camera != nds_fp_camera) return;
    u32 hero = REG32(nds_fp_players + (u32)REG8(nds_fp_index) * 4);
    if (!hero) return;
    /* matrice du personnage : axes X, Y, Z (son avant), puis position */
    const s32 *body = (const s32 *)(hero + nds_fp_matrix);
    float fx_x = real(body[6]), fx_z = real(body[8]);
    if (fx_x == 0 && fx_z == 0) return;
    if (last_frame != nds_frame) {
        /* le regard suit le personnage en douceur ; un demi-tour d'un coup passe par le cote */
        int first = look_x == 0 && look_z == 0;
        float keep = first ? 0 : 1.0f - TURN_SMOOTHING;
        float x = look_x * keep + fx_x * (1.0f - keep), z = look_z * keep + fx_z * (1.0f - keep);
        float size = root(x * x + z * z);
        if (size < 0.01f) {
            x = -look_z;
            z = look_x;
            size = 1;
        }
        look_x = x / size;
        look_z = z / size;
        last_frame = nds_frame;
    }
    s32 *pos = (s32 *)camera;
    pos[0] = body[9] + fx(look_x * EYE_FORWARD);
    pos[1] = body[10] + fx(EYE_HEIGHT);
    pos[2] = body[11] + fx(look_z * EYE_FORWARD);
    /* lignes de la camera : droite, haut, arriere */
    s32 *axes = (s32 *)(camera + CAM_ROTATION);
    axes[0] = fx(-look_z); axes[1] = 0; axes[2] = fx(look_x); axes[3] = 0;
    axes[4] = 0; axes[5] = 0x1000; axes[6] = 0; axes[7] = 0;
    axes[8] = fx(-look_x); axes[9] = 0; axes[10] = fx(-look_z); axes[11] = 0;
    axes[12] = 0; axes[13] = 0; axes[14] = 0; axes[15] = 0x1000;
    REG32(camera + CAM_FLAGS) |= VIEW_DIRTY;
}
