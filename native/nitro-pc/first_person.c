/* --first-person : la camera du jeu devient les yeux du joueur, et la souris la dirige.
 *
 * Le jeu construit sa matrice de vue depuis une position et trois axes. native/games/patches.json dit, pour les jeux qui le permettent, ou sont
 * cette camera et le personnage du joueur ; une accroche a l'entree de la fonction qui calcule la vue remplace ces valeurs juste avant le calcul. */
#include "nitro.h"

/* tables.c, d'apres patches.json ; 0 pour les autres jeux */
extern const u32 nds_fp_camera, nds_fp_players, nds_fp_index, nds_fp_matrix, nds_fp_yaw, nds_fp_heading_a, nds_fp_heading_b, nds_fp_render_obj, nds_fp_arm_slots;
extern int nds_fp_mode;

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

static float yaw, pitch;
static int started;

static inline u32 player(void) { return REG32(nds_fp_players + (u32)REG8(nds_fp_index) * 4); }

/* Le jeu oriente les touches de direction d'apres le cap de sa propre camera, qui suit le personnage de loin. On lui donne le notre, a chaque fois
 * qu'il demande sa camera : haut avance alors dans le regard, gauche et droite vont sur les cotes. Sur le DS le cap vise a l'oppose du regard. */
void nds_hook_camera_yaw(u32 r0) {
    if (!started) return;
    s32 angle = (s32)(yaw / (2 * PI) * 65536.0f);
    REG16(nds_fp_yaw) = (u16)(angle + 0x8000);
    /* le personnage regarde ou regarde la camera : l'arme vise au centre de l'image */
    u32 hero = player();
    if (hero) {
        REG16(hero + nds_fp_heading_a) = (u16)angle;
        REG16(hero + nds_fp_heading_b) = (u16)angle;
    }
}

/* Dessin d'un modele, forme par forme : celui du joueur perd sa tete et ses jambes ; le torse ne garde que les bras, separes par leurs os. L'arme
 * est un petit modele a part, dessine a la main du personnage : la ou seul le joueur peut en avoir un, puisque c'est sous la camera. */
#define SHAPE_TORSO 2
#define MODEL_NODE_COUNT 0x17 /* octet du modele : nombre d'os */
#define MODEL_MAX_NODES_OF_WEAPON 8

void nds_hook_model_shape(u32 rs) {
    if (!started) return;
    u32 hero = player();
    if (!hero) return;
    u32 object = REG32(rs + 4);
    if (object == hero + nds_fp_render_obj) {
        u32 shape = REG8(REG32(rs) + 1);
        nds_fp_mode = shape == SHAPE_TORSO ? 2 : 1;
    } else if (REG8(REG32(object + 4) + MODEL_NODE_COUNT) <= MODEL_MAX_NODES_OF_WEAPON) {
        nds_fp_mode = 3;
    }
}

void nds_hook_model_shape_end(void) { nds_fp_mode = 0; }

/* Sous la camera et pas devant : seul ce que tient le joueur peut etre la */
int nds_fp_in_hand(const float *view) { return view[0] > -4.5f && view[0] < 4.5f && view[1] > -5.0f && view[1] < -0.5f && view[2] > -1.0f && view[2] < 3.5f; }

int nds_fp_arm_slot(int slot) { return slot >= 0 && slot < 32 && (nds_fp_arm_slots >> slot & 1); }

/* Les bras du modele pendent le long du corps et sont sous la camera : on annule la pente de la camera (ils la suivent), on les leve vers l'avant
 * autour de l'epaule, puis on les pose en bas de l'image. Unites du jeu, espace de la vue (la camera regarde vers -z). */
#define ARM_SHOULDER_Z 1.0f
#define ARM_RAISE_SIN 0.94f
#define ARM_RAISE_COS 0.34f
#define ARM_SPREAD 0.8f
#define ARM_X 0.0f
#define ARM_Y -1.15f
#define ARM_Z -0.8f

void nds_fp_hold_arms(float *view) {
    float s = sine(pitch), c = cosine(pitch);
    float y = view[1] * c + view[2] * s, z = view[2] * c - view[1] * s - ARM_SHOULDER_Z;
    view[0] = view[0] * ARM_SPREAD + ARM_X;
    view[1] = y * ARM_RAISE_COS - z * ARM_RAISE_SIN + ARM_Y;
    view[2] = z * ARM_RAISE_COS + y * ARM_RAISE_SIN + ARM_Z;
}

/* Un viseur, en couleur inverse pour rester visible sur tous les decors */
void nds_fp_crosshair(u32 *screen, int w, int h, int k, int stride) {
    if (!started) return;
    int cx = w / 2, cy = h / 2, gap = 2 * k, arm = 5 * k, t = k > 1 ? k / 2 : 1;
    for (int i = gap; i < gap + arm; i++)
        for (int j = 0; j < t; j++) {
            int points[4][2] = {{cx + i, cy + j}, {cx - i, cy + j}, {cx + j, cy + i}, {cx + j, cy - i}};
            for (int p = 0; p < 4; p++) {
                int x = points[p][0], y = points[p][1];
                if (x >= 0 && y >= 0 && x < w && y < h) screen[y * stride + x] ^= 0x00FFFFFFu;
            }
        }
}

void nds_hook_camera(u32 camera) {
    static int active = -1;
    static int seen_x, seen_y;
    if (active < 0) active = nds_fp_camera && nds_arg("--first-person", 0);
    if (!active || camera != nds_fp_camera) return;
    u32 hero = player();
    if (!hero) {
        started = 0;
        return;
    }
    /* matrice du personnage : axes X, Y, Z (son avant), puis position */
    const s32 *body = (const s32 *)DS(hero + nds_fp_matrix);
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
    s32 *pos = (s32 *)DS(camera);
    pos[0] = body[9] + fx(look_x * EYE_FORWARD);
    pos[1] = body[10] + fx(EYE_HEIGHT);
    pos[2] = body[11] + fx(look_z * EYE_FORWARD);
    /* lignes de la camera : droite, haut, arriere */
    s32 *axes = (s32 *)DS(camera + CAM_ROTATION);
    axes[0] = fx(-look_z); axes[1] = 0; axes[2] = fx(look_x); axes[3] = 0;
    axes[4] = fx(-up * look_x); axes[5] = fx(level); axes[6] = fx(-up * look_z); axes[7] = 0;
    axes[8] = fx(-look_x * level); axes[9] = fx(-up); axes[10] = fx(-look_z * level); axes[11] = 0;
    axes[12] = 0; axes[13] = 0; axes[14] = 0; axes[15] = 0x1000;
    REG32(camera + CAM_FLAGS) |= VIEW_DIRTY;
}
