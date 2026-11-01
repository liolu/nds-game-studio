/* Essai du son de nitro-pc sur macOS (native/MACOS.md 3.4) : 2 secondes de sinus a 440 Hz par le meme chemin que le
 * fil du son de snd.c : os_thread_start, os_thread_set_high_priority, os_audio_open(RATE, BUF_FRAMES), amorce de
 * os_audio_depth tampons, puis acquire / remplir / submit. La 1re seconde a gauche, la 2e a droite.
 *
 *   sh native/tests/audio_smoke.sh            # s'entend
 *   sh native/tests/audio_smoke.sh --mute     # sans sortie : cadence par os_sleep, comme snd.c avec --mute
 *   sh native/tests/audio_smoke.sh --arch x86_64
 *
 * Verifie aussi le rythme : entre le 1er tampon rendu par la file et la fin du sinus, la sortie doit tirer le son en
 * temps reel. Le demarrage de la sortie est affiche a part (sous Rosetta, la 1re fois, il peut prendre plus d'une seconde). */
#include <math.h>
#include <stdio.h>
#include <string.h>

#include "nitro.h"

/* memes valeurs que snd.c */
#define RATE 32768
#define BUF_FRAMES 384

#define SECONDS 2
#define TONE 440.0
#define FADE (RATE / 100) /* 10 ms, sans claquement aux changements de cote */

static int mute;
static OsEvent done;
static double startup, elapsed, expected;

/* image f de 0 a SECONDS*RATE : sinus, gauche puis droite */
static void fill(s16 *out, u32 frames, u32 *pos) {
    for (u32 i = 0; i < frames; i++, (*pos)++) {
        u32 f = *pos;
        s16 l = 0, r = 0;
        if (f < SECONDS * RATE) {
            u32 in_half = f % RATE;
            double env = 1.0;
            if (in_half < FADE) env = (double)in_half / FADE;
            else if (in_half > RATE - FADE) env = (double)(RATE - in_half) / FADE;
            s16 v = (s16)(0.25 * 32767 * env * sin(2 * M_PI * TONE * f / RATE));
            if (f < RATE) l = v;
            else r = v;
        }
        out[i * 2] = l;
        out[i * 2 + 1] = r;
    }
}

static void audio_thread(void *arg) {
    static s16 idle_buf[BUF_FRAMES * 2];
    OsAudio *dev = 0;
    if (!mute) {
        dev = os_audio_open(RATE, BUF_FRAMES);
        if (!dev) printf("pas de sortie son\n");
    }
    os_thread_set_high_priority();
    u32 pos = 0, total = SECONDS * RATE;
    s64 freq = os_time_freq(), t0 = os_time_now();
    if (dev) {
        for (u32 i = os_audio_depth(dev); i; i--) {
            s16 *b = os_audio_acquire(dev);
            fill(b, BUF_FRAMES, &pos);
            os_audio_submit(dev);
        }
    }
    /* apres la fin du sinus, encore un tour de file en silence : tout ce qui precede est alors joue */
    u32 tail = dev ? os_audio_depth(dev) * BUF_FRAMES : 0;
    /* avec sortie : le chrono part quand la file rend son 1er tampon, la fin du sinus est alors a total - 1 tampon */
    s64 t1 = t0;
    expected = (double)(dev ? total - BUF_FRAMES : total) / RATE;
    while (pos < total + tail) {
        if (dev) {
            s16 *b = os_audio_acquire(dev);
            if (t1 == t0) {
                t1 = os_time_now();
                startup = (double)(t1 - t0) / freq;
            }
            fill(b, BUF_FRAMES, &pos);
            os_audio_submit(dev);
        } else {
            os_sleep(BUF_FRAMES * 1000 / RATE);
            fill(idle_buf, BUF_FRAMES, &pos);
        }
    }
    elapsed = (double)(os_time_now() - t1) / freq;
    os_event_set(done);
}

int main(int argc, char **argv) {
    for (int i = 1; i < argc; i++)
        if (!strcmp(argv[i], "--mute")) mute = 1;
    printf("sinus %.0f Hz, %d s a %d Hz : gauche puis droite%s\n", TONE, SECONDS, RATE, mute ? " (--mute : rien ne sort)" : "");
    fflush(stdout);
    done = os_event_create();
    if (!os_thread_start(audio_thread, 0, 0x40000)) {
        printf("ECHEC : fil du son\n");
        return 1;
    }
    os_event_wait(done);
    /* sortie : 3 % pres (la file avance par tampons de 12 ms) ; sans son, os_sleep(11) dure un peu plus que 384 images */
    double lo = expected * 0.97, hi = expected * (mute ? 1.25 : 1.03);
    int ok = elapsed > lo && elapsed < hi;
    if (!mute && startup > 0) printf("demarrage de la sortie : %.0f ms\n", startup * 1000);
    printf("%s : %.3f s de son tirees en %.3f s (attendu %.3f-%.3f)\n", ok ? "OK" : "ECHEC", expected, elapsed, lo, hi);
    return ok ? 0 : 1;
}
