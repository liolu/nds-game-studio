/* --first-person : la camera du jeu devient les yeux du joueur, et la souris la dirige.
 *
 * Le jeu construit sa matrice de vue depuis une position et trois axes. native/games/patches.json dit, pour les jeux qui le permettent, ou sont
 * cette camera et le personnage du joueur ; une accroche a l'entree de la fonction qui calcule la vue remplace ces valeurs juste avant le calcul. */
#include "nitro.h"

/* tables.c, d'apres patches.json ; 0 pour les autres jeux */
extern const u32 nds_fp_camera, nds_fp_players, nds_fp_index, nds_fp_matrix, nds_fp_yaw;

#define CAM_FLAGS 0x160
#define CAM_ROTATION 0x14
#define VIEW_DIRTY 0xCu /* la vue et le volume de vue sont a refaire */
#define EYE_HEIGHT 5.6f
#define EYE_FORWARD 0.9f
#define PI 3.14159265f
#define RADIANS_PER_COUNT 0.0025f /* souris */
#define MAX_PITCH 1.3f

static inline s32 fx(float v) { return (s32)(v * 4096.0f); }
static inline float real(s32 v) { return (float)v / 4096.0f; }

static float wrap_angle(float a) {
    while (a > PI) a -= 2 * PI;
    while (a < -PI) a += 2 * PI;
    return a;
}

/* pas de bibliotheque mathematique dans le module : serie de Taylor, sur [-pi/2, pi/2] */
static float sine(float a) {
    a = wrap_angle(a);
    if (a > PI / 2) a = PI - a;
    else if (a < -PI / 2) a = -PI - a;
    float x2 = a * a;
    return a * (1 - x2 / 6 * (1 - x2 / 20 * (1 - x2 / 42 * (1 - x2 / 72))));
}

static float cosine(float a) { return sine(a + PI / 2); }

static float arc_tangent(float y, float x) {
    /* assez juste pour un cap de depart : par pas de dichotomie sur le sinus et le cosinus */
    float low = -PI, high = PI;
    for (int i = 0; i < 24; i++) {
        float mid = (low + high) / 2;
        /* signe du produit vectoriel entre (x, y) et la direction mid */
        if (cosine(mid) * y - sine(mid) * x > 0) low = mid;
        else high = mid;
    }
    return (low + high) / 2;
}

static float yaw;
static int started;

/* Le jeu oriente les touches de direction d'apres le cap de sa propre camera, qui suit le personnage de loin. On lui donne le notre, a chaque fois
 * qu'il demande sa camera : haut avance alors dans le regard, gauche et droite vont sur les cotes. Sur le DS le cap vise a l'oppose du regard. */
void nds_hook_camera_yaw(u32 r0) {
    if (started) REG16(nds_fp_yaw) = (u16)((s32)(yaw / (2 * PI) * 65536.0f) + 0x8000);
}

void nds_hook_camera(u32 camera) {
    static int active = -1;
    static float pitch;
    static int seen_x, seen_y;
    if (active < 0) active = nds_fp_camera && nds_arg("--first-person", 0);
    if (!active || camera != nds_fp_camera) return;
    u32 hero = REG32(nds_fp_players + (u32)REG8(nds_fp_index) * 4);
    if (!hero) {
        started = 0;
        return;
    }
    /* matrice du personnage : axes X, Y, Z (son avant), puis position */
    const s32 *body = (const s32 *)(hero + nds_fp_matrix);
    int mouse_x, mouse_y;
    present_mouse(&mouse_x, &mouse_y);
    if (!started) {
        /* nouveau personnage ou nouveau niveau : le regard part dans la direction du personnage */
        started = 1;
        yaw = arc_tangent(real(body[6]), real(body[8]));
        pitch = 0;
        seen_x = mouse_x;
        seen_y = mouse_y;
    }
    yaw = wrap_angle(yaw - (float)(mouse_x - seen_x) * RADIANS_PER_COUNT);
    pitch -= (float)(mouse_y - seen_y) * RADIANS_PER_COUNT;
    if (pitch > MAX_PITCH) pitch = MAX_PITCH;
    if (pitch < -MAX_PITCH) pitch = -MAX_PITCH;
    seen_x = mouse_x;
    seen_y = mouse_y;
    float look_x = sine(yaw), look_z = cosine(yaw), up = sine(pitch), level = cosine(pitch);
    s32 *pos = (s32 *)camera;
    pos[0] = body[9] + fx(look_x * EYE_FORWARD);
    pos[1] = body[10] + fx(EYE_HEIGHT);
    pos[2] = body[11] + fx(look_z * EYE_FORWARD);
    /* lignes de la camera : droite, haut, arriere */
    s32 *axes = (s32 *)(camera + CAM_ROTATION);
    axes[0] = fx(-look_z); axes[1] = 0; axes[2] = fx(look_x); axes[3] = 0;
    axes[4] = fx(-up * look_x); axes[5] = fx(level); axes[6] = fx(-up * look_z); axes[7] = 0;
    axes[8] = fx(-look_x * level); axes[9] = fx(-up); axes[10] = fx(-look_z * level); axes[11] = 0;
    axes[12] = 0; axes[13] = 0; axes[14] = 0; axes[15] = 0x1000;
    REG32(camera + CAM_FLAGS) |= VIEW_DIRTY;
}
