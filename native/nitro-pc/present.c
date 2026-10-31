/* Sortie : les deux ecrans vers le launcher (--pipe), ou images BMP pour les essais (--dump dossier --every n --frames n). */
#include "nitro.h"

static const char *dump_dir;
static u32 every = 60, max_frames;
static int pipe_mode;
static const char *key_script;
static unsigned long WINAPI input_thread(void *arg);
static volatile u32 keys_now;
static volatile int touch_down, touch_x, touch_y;
static void script_step(void);

void present_init(void) {
    const char *v;
    nds_arg("--dump", &dump_dir);
    if (nds_arg("--every", &v) && v) { every = 0; while (*v) every = every * 10 + *v++ - '0'; }
    if (nds_arg("--frames", &v) && v) { while (*v) max_frames = max_frames * 10 + *v++ - '0'; }
    nds_arg("--keys", &key_script);
    pipe_mode = nds_arg("--pipe", 0);
    if (pipe_mode) CreateThread(0, 0x10000, input_thread, 0, 0, 0);
}

static void write_bmp(u32 frame) {
    static u8 file[54 + 256 * 384 * 3];
    u8 *h = file;
    u32 size = sizeof(file);
    nds_zero(h, 54);
    h[0] = 'B'; h[1] = 'M';
    *(u32 *)(h + 2) = size; *(u32 *)(h + 10) = 54; *(u32 *)(h + 14) = 40;
    *(u32 *)(h + 18) = 256; *(u32 *)(h + 22) = 384;
    *(u16 *)(h + 26) = 1; *(u16 *)(h + 28) = 24;
    u8 *p = h + 54;
    for (int y = 383; y >= 0; y--) {
        const u16 *line = y < 192 ? gfx_top() + y * 256 : gfx_bottom() + (y - 192) * 256;
        for (int x = 0; x < 256; x++) {
            u16 c = line[x];
            *p++ = (u8)(((c >> 10) & 31) * 255 / 31);
            *p++ = (u8)(((c >> 5) & 31) * 255 / 31);
            *p++ = (u8)((c & 31) * 255 / 31);
        }
    }
    char name[300];
    int n = 0;
    for (const char *d = dump_dir; *d && n < 260; d++) name[n++] = *d;
    name[n++] = '/';
    u32 v = frame, div = 100000;
    while (div) { name[n++] = '0' + (v / div) % 10; div /= 10; }
    name[n++] = '.'; name[n++] = 'b'; name[n++] = 'm'; name[n++] = 'p'; name[n] = 0;
    void *f = CreateFileA(name, 0x40000000u, 0, 0, 2, 0, 0);
    unsigned long w;
    WriteFile(f, file, size, &w, 0);
    CloseHandle(f);
}

extern int nds_trace;
extern u32 nds_trace_count, nds_trace_eip, nds_trace_addr, nds_trace_stack[13];

void present_frame(void) {
    if (dump_dir && nds_frame % every == 0) write_bmp(nds_frame);
    script_step();
    /* boutons X/Y : actifs a 0 ; bit 15 a 0 = couvercle ouvert */
    {
        u32 xy = 0x2C00; /* X = bit 10, Y = bit 11, debogage = bit 13 : relaches a 1, comme KEYINPUT */
        if (keys_now & (1 << 10)) xy &= ~(1u << 10);
        if (keys_now & (1 << 11)) xy &= ~(1u << 11);
        REG16(0x027FFFA8u) = (u16)xy;
        /* dernier releve du stylet pour le SDK : x 12 bits, y 12 bits, touche */
        *(volatile u16 *)0x027FFFAAu = (u16)((touch_x & 0xFFF) | ((touch_y & 0xF) << 12));
        *(volatile u16 *)0x027FFFACu = (u16)(((touch_y >> 4) & 0xFF) | (touch_down ? 0x100 : 0));
    }
    if (pipe_mode) {
        static u16 both[256 * 192 * 2];
        unsigned long w;
        nds_copy(both, gfx_top(), 256 * 192 * 2);
        nds_copy(both + 256 * 192, gfx_bottom(), 256 * 192 * 2);
        if (!WriteFile(GetStdHandle((unsigned long)-11), both, sizeof(both), &w, 0)) ExitProcess(0);
    }
    static int trace_on = -1;
    if (trace_on < 0) trace_on = nds_arg("--trace", 0);
    if (trace_on) {
        nds_trace = 1;
        if (nds_frame % every == 0) {
            nds_log("image %u : %u acces registres, dernier %x eip=%x", nds_frame, nds_trace_count, nds_trace_addr, nds_trace_eip);
            nds_trace_count = 0;
            nds_log("  dispA=%x dispB=%x brA=%x brB=%x pow=%x vram=%x %x bg0A=%x", REG32(IO), REG32(IO + 0x1000), REG16(IO + 0x6C), REG16(IO + 0x106C), REG16(IO + 0x304), REG32(IO + 0x240), REG32(IO + 0x244), REG16(IO + 8));
            {
                u32 pal = 0, pal2 = 0, vram = 0, oam = 0;
                for (u32 i = 0; i < 0x200; i += 2) { if (REG16(0x05000000u + i)) pal++; if (REG16(0x05000400u + i)) pal2++; }
                for (u32 i = 0; i < 0x80000; i += 4) if (REG32(0x06000000u + i)) vram++;
                for (u32 i = 0; i < 0x400; i += 8) if ((REG16(0x07000000u + i) & 0x300) != 0x200) oam++;
                nds_log("  pal=%u palB=%u vramBG=%u objs=%u polys=%u etat=%x sub=%x clear=%x disp3d=%x", pal, pal2, vram, oam, REG16(IO + 0x604), REG32(0x02088438u), REG32(0x02088448u), REG32(IO + 0x350), REG16(IO + 0x60));
            }
            {
                extern u32 gx_stat[8];
                nds_log("  3d ecrits=%u rejZ=%u rejTex=%u polys=%u", gx_stat[0], gx_stat[1], gx_stat[2], gx_stat[3]);
                gx_stat[0] = gx_stat[1] = gx_stat[2] = 0;
            }
            if (nds_arg("--threads-dump", 0)) { extern void nds_dump_threads(void); nds_dump_threads(); }
            nds_log("    %8x", nds_trace_eip);
            for (int i = 0; nds_trace_stack[i]; i++) nds_log("    %8x", nds_trace_stack[i]);
        }
    }
#ifdef NDS_INSTRUMENT
    if (nds_arg("--ring", 0) && nds_frame % every == every - 1) {
        extern u32 nds_ring[4096], nds_ring_pos;
        nds_log("appels avant l'image %u :", nds_frame);
        for (int i = 0; i < 4096; i++) nds_log("    %8x", nds_ring[(nds_ring_pos + i) & 4095]);
    }
#endif
    if (max_frames && nds_frame >= max_frames) ExitProcess(0);
}

/* commandes : le launcher envoie 8 octets a chaque changement (touches, stylet) */
static volatile u32 keys_now;
static volatile int touch_down, touch_x, touch_y;

static unsigned long WINAPI input_thread(void *arg) {
    void *in = GetStdHandle((unsigned long)-10);
    u16 packet[4];
    unsigned long got;
    for (;;) {
        u32 have = 0;
        while (have < 8) {
            if (!ReadFile(in, (u8 *)packet + have, 8 - have, &got, 0) || !got) ExitProcess(0); /* launcher ferme */
            have += got;
        }
        keys_now = packet[0];
        touch_down = packet[1];
        touch_x = packet[2];
        touch_y = packet[3];
    }
    return 0;
}

/* essais sans fenetre : --keys "600:8,620:0" = a l'image 600 le masque de touches 8 (Start), etc. */

static void script_step(void) {
    while (key_script && *key_script) {
        u32 frame = 0, mask = 0;
        const char *p = key_script;
        while (*p >= '0' && *p <= '9') frame = frame * 10 + *p++ - '0';
        if (frame > nds_frame) return;
        if (*p == ':') p++;
        while (*p >= '0' && *p <= '9') mask = mask * 10 + *p++ - '0';
        keys_now = mask;
        if (*p == '/') { /* image:masque/x/y : stylet */
            int x = 0, y = 0;
            p++;
            while (*p >= '0' && *p <= '9') x = x * 10 + *p++ - '0';
            if (*p == '/') p++;
            while (*p >= '0' && *p <= '9') y = y * 10 + *p++ - '0';
            touch_down = 1;
            touch_x = x;
            touch_y = y;
        } else {
            touch_down = 0;
        }
        if (*p == ',') p++;
        key_script = p;
    }
}

u32 present_keys(void) { return keys_now & 0x3FF; }

void present_touch(int *down, int *x, int *y) {
    *down = touch_down;
    *x = touch_x;
    *y = touch_y;
}
