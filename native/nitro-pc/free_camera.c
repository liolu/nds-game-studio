/* --free-camera : la camera se detache du jeu et se dirige a la souris et aux touches, en jeu comme dans une cinematique.
 *
 * C et V du clavier (present.c, present_free_camera) : bit 0 = camera libre, bit 1 = scene gelee. Le jeu ne recoit
 * plus de touches pendant ce temps. La camera passe par l'accroche de la vue, comme la vue a la premiere personne (first_person.c).
 *
 * Pour geler une scene, rien de nouveau dans le jeu : la mise a jour du niveau ne tourne plus, ses taches sont coupees et ses objets recoivent le
 * drapeau de pause du moteur ; ils ne bougent plus mais restent dessines. Le jeu continue de dessiner chaque image. */
#include "nitro.h"

/* tables.c, d'apres patches.json ; 0 pour les autres jeux */
extern const u32 nds_fp_camera, nds_free_game_flags, nds_free_objects, nds_free_tasks, nds_free_timers, nds_free_get_tick;

#define CAM_ROTATION 0x14
#define CAM_FLAGS 0x160
#define VIEW_DIRTY 0xCu /* la vue et le volume de vue sont a refaire */
#define CAM_WORDS 21    /* position, puis matrice 4x4 de CAM_ROTATION : ce qu'on remet en quittant */
#define PI 3.14159265f
#define RADIANS_PER_COUNT 0.0025f /* souris */
#define MAX_PITCH 1.5f
#define SPEED 0.25f /* unites du jeu par image du DS ; le personnage mesure environ 6 unites */
#define SPEED_FAST 4.0f
#define SPEED_SLOW 0.2f
#define MAX_STEP 8 /* images rattrapees au plus, apres un arret du jeu */

/* bits de KEYINPUT */
#define PAD_A 1u
#define PAD_B 2u
#define PAD_RIGHT 16u
#define PAD_LEFT 32u
#define PAD_UP 64u
#define PAD_DOWN 128u
#define PAD_R 256u
#define PAD_L 512u

static inline s32 fx(float v) { return (s32)(v * 4096.0f); }
static inline float real(s32 v) { return (float)v / 4096.0f; }

int nds_free_camera(u32 camera) {
    static int on, seen_x, seen_y;
    static u32 last_frame;
    static float x, y, z, yaw, pitch;
    static s32 saved[CAM_WORDS];

    if (!nds_fp_camera || camera != nds_fp_camera) return 0;
    s32 *cam = (s32 *)DS(camera);
    int mouse_x, mouse_y;
    present_mouse(&mouse_x, &mouse_y);
    if (!present_free_camera()) {
        if (on) {
            /* le jeu reprend sa camera ou il l'avait laissee */
            on = 0;
            for (int i = 0; i < CAM_WORDS; i++) cam[i] = saved[i];
            REG32(camera + CAM_FLAGS) |= VIEW_DIRTY;
        }
        return 0;
    }
    if (!on) {
        /* on part de la ou regarde le jeu : la troisieme ligne de ses axes pointe vers l'arriere */
        on = 1;
        for (int i = 0; i < CAM_WORDS; i++) saved[i] = cam[i];
        const s32 *axes = cam + CAM_ROTATION / 4;
        float back_x = real(axes[8]), back_y = real(axes[9]), back_z = real(axes[10]);
        x = real(cam[0]);
        y = real(cam[1]);
        z = real(cam[2]);
        yaw = fp_arc_tangent(-back_x, -back_z);
        pitch = fp_arc_tangent(-back_y, -back_x * fp_sine(yaw) - back_z * fp_cosine(yaw));
        seen_x = mouse_x;
        seen_y = mouse_y;
        last_frame = nds_frame;
    }
    yaw = fp_wrap_angle(yaw - (float)(mouse_x - seen_x) * RADIANS_PER_COUNT);
    pitch -= (float)(mouse_y - seen_y) * RADIANS_PER_COUNT;
    if (pitch > MAX_PITCH) pitch = MAX_PITCH;
    if (pitch < -MAX_PITCH) pitch = -MAX_PITCH;
    seen_x = mouse_x;
    seen_y = mouse_y;

    float look_x = fp_sine(yaw), look_z = fp_cosine(yaw), up = fp_sine(pitch), level = fp_cosine(pitch);

    /* la vue se calcule une fois par image du jeu, 30 par seconde en general : la vitesse se compte en images du DS */
    u32 pad = present_pad();
    u32 step = nds_frame - last_frame;
    last_frame = nds_frame;
    if (step > MAX_STEP) step = MAX_STEP;
    float distance = SPEED * (float)step;
    if (pad & PAD_A) distance *= SPEED_FAST;
    if (pad & PAD_B) distance *= SPEED_SLOW;
    float forward = (pad & PAD_UP ? distance : 0) - (pad & PAD_DOWN ? distance : 0);
    float right = (pad & PAD_RIGHT ? distance : 0) - (pad & PAD_LEFT ? distance : 0);
    float rise = (pad & PAD_R ? distance : 0) - (pad & PAD_L ? distance : 0);
    x += look_x * level * forward - look_z * right;
    y += up * forward + rise;
    z += look_z * level * forward + look_x * right;

    cam[0] = fx(x);
    cam[1] = fx(y);
    cam[2] = fx(z);
    /* lignes de la camera : droite, haut, arriere */
    s32 *axes = cam + CAM_ROTATION / 4;
    axes[0] = fx(-look_z); axes[1] = 0; axes[2] = fx(look_x); axes[3] = 0;
    axes[4] = fx(-up * look_x); axes[5] = fx(level); axes[6] = fx(-up * look_z); axes[7] = 0;
    axes[8] = fx(-look_x * level); axes[9] = fx(-up); axes[10] = fx(-look_z * level); axes[11] = 0;
    axes[12] = 0; axes[13] = 0; axes[14] = 0; axes[15] = 0x1000;
    REG32(camera + CAM_FLAGS) |= VIEW_DIRTY;
    return 1;
}

/* ---- geler la scene ---- */

#define GAME_PAUSED 3u    /* le jeu est deja en pause : il saute lui-meme la mise a jour du niveau */
#define GAME_MULTI 0x40u  /* a plusieurs : les autres machines attendent la notre */
#define LISTS 2           /* listes d'objets et de taches, toutes deux de 0x38 octets : un en-tete, puis la tete de liste a +4 */
#define LIST_SIZE 0x38
#define OBJECT_FLAGS 8    /* drapeaux d'un objet de la scene */
#define OBJECT_PAUSED 0x40000u
#define TASK_ACTIVE 4     /* octet d'une tache */
#define MAX_TASKS 512
#define TIMERS 5          /* compteurs du jeu en temps reel, de 16 octets chacun : debut, ... ; octet +0xD = en marche */
#define TIMER_BASE 0x428
#define TIMER_MANAGER 0x68

static int frozen;
static u32 tasks[MAX_TASKS];
static int task_count;

static void objects_paused(int on) {
    for (int list = 0; list < LISTS; list++)
        for (u32 node = REG32(nds_free_objects + list * LIST_SIZE + 4); node; node = REG32(node + 4)) {
            u32 object = REG32(node);
            if (on) REG32(object + OBJECT_FLAGS) |= OBJECT_PAUSED;
            else REG32(object + OBJECT_FLAGS) &= ~OBJECT_PAUSED;
        }
}

/* Les taches coupees ici sont les seules remises en marche : le jeu peut en avoir arrete d'autres */
static void tasks_stop(void) {
    for (int list = 0; list < LISTS; list++)
        for (u32 node = REG32(nds_free_tasks + list * LIST_SIZE + 4); node; node = REG32(node + 4)) {
            u32 task = REG32(node);
            if (REG8(task + TASK_ACTIVE) && task_count < MAX_TASKS) {
                REG8(task + TASK_ACTIVE) = 0;
                tasks[task_count++] = task;
            }
        }
}

static void tasks_resume(void) {
    for (int list = 0; list < LISTS; list++)
        for (u32 node = REG32(nds_free_tasks + list * LIST_SIZE + 4); node; node = REG32(node + 4)) {
            u32 task = REG32(node);
            for (int i = 0; i < task_count; i++)
                if (tasks[i] == task) {
                    REG8(task + TASK_ACTIVE) = 1;
                    break;
                }
        }
    task_count = 0;
}

/* Ces compteurs lisent l'horloge : sans ca, le temps du gel compterait d'un coup a la reprise */
static void timers_hold(void) {
    u32 manager = REG32(nds_free_timers + TIMER_MANAGER);
    if (!manager) return;
    u32 now = (u32)((u64(*)())nds_call_target(nds_free_get_tick))(0, 0, 0, 0);
    for (int i = 0; i < TIMERS; i++) {
        u32 timer = manager + TIMER_BASE + i * 16;
        if (REG8(timer + 0xD)) REG32(timer) = now;
    }
}

static void thaw(void) {
    frozen = 0;
    tasks_resume();
    objects_paused(0);
    timers_hold();
}

/* Remplace la mise a jour du niveau tant que la scene est gelee */
int nds_gate_level_update(u32 level) {
    u32 flags = REG32(nds_free_game_flags);
    int want = (present_free_camera() & 2) && !(flags & (GAME_PAUSED | GAME_MULTI));
    if (!want) {
        if (frozen) thaw();
        return 0;
    }
    frozen = 1;
    /* a chaque image : le niveau a pu creer des objets juste avant le gel */
    tasks_stop();
    objects_paused(1);
    timers_hold();
    return 1;
}

/* On quitte le niveau : ses objets et ses taches ne doivent pas rester arretes */
void nds_hook_level_exit(u32 level) {
    if (frozen) thaw();
}
