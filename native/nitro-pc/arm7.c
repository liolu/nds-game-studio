/* Remplace le processeur ARM7 du DS (firmware son, tactile, horloge). */
#include "nitro.h"

void io_fifo_push(u32 word);
void hle_frame(void);
void snd_command(u32 id, u32 *arg);
void snd_frame(void);

typedef struct SNDCommand {
    u32 next;
    u32 id;
    u32 arg[4];
} SNDCommand;

static volatile u32 *shared; /* SNDSharedWork : son premier mot compte les listes de commandes traitees */
static u32 logged;

void snd_set_shared(u32 addr) { shared = (volatile u32 *)addr; }
volatile u32 *snd_shared(void) { return shared; }

static void snd_list(u32 addr) {
    for (SNDCommand *c = (SNDCommand *)addr; c; c = (SNDCommand *)c->next) {
        if (c->id == 29) snd_set_shared(c->arg[0]);
        snd_command(c->id, c->arg);
    }
    if (shared) shared[0]++;
}

/* ------------------------------------------------------------------ sauvegarde de la carte */
#define SAVE_SIZE (1u << 20)
static u8 *save;
static const char *save_path;
static u32 *card_cmd; /* result, ?, ?, source, destination, longueur */

static void save_flush(void) {
    if (!save_path) return;
    void *f = CreateFileA(save_path, 0x40000000u, 0, 0, 2, 0, 0);
    if (f == (void *)-1) return;
    unsigned long w;
    WriteFile(f, save, SAVE_SIZE, &w, 0);
    CloseHandle(f);
}

static void save_load(void) {
    save = VirtualAlloc(0, SAVE_SIZE, 0x3000, 4);
    for (u32 i = 0; i < SAVE_SIZE; i++) save[i] = 0xFF;
    nds_arg("--save", &save_path);
    if (!save_path) return;
    void *f = CreateFileA(save_path, 0x80000000u, 1, 0, 3, 0, 0);
    if (f == (void *)-1) return;
    unsigned long r;
    ReadFile(f, save, SAVE_SIZE, &r, 0);
    CloseHandle(f);
}

static void card_request(u32 req) {
    u32 src = card_cmd[3], dst = card_cmd[4], len = card_cmd[5];
    switch (req) {
    case 6: /* lecture */
        if (src + len <= SAVE_SIZE) nds_copy((void *)dst, save + src, len);
        break;
    case 7: case 8: /* ecriture */
        if (dst + len <= SAVE_SIZE) { nds_copy(save + dst, (void *)src, len); save_flush(); }
        break;
    case 10: case 11: /* effacement */
        if (dst + len <= SAVE_SIZE) { for (u32 i = 0; i < len; i++) save[dst + i] = 0xFF; save_flush(); }
        break;
    case 12:
        for (u32 i = 0; i < SAVE_SIZE; i++) save[i] = 0xFF;
        save_flush();
        break;
    default: /* identification, verification : rien a faire */
        break;
    }
    card_cmd[0] = 0;
}

static void card_fifo(u32 data) {
    if (data == 0) return; /* la commande d'init : l'adresse du bloc de commande suit */
    if (data >= 0x02000000u) card_cmd = (u32 *)data;
    else if (card_cmd) card_request(data);
    io_fifo_push(11u | (1u << 5));
}

void arm7_fifo_send(u32 word) {
    u32 tag = word & 31, data = word >> 6;
    if (tag == 7) {
        if (data) snd_list(data);
        return;
    }
    if (tag == 11) {
        card_fifo(data);
        return;
    }
    if (tag == 6 || tag == 8 || tag == 9) {
        /* tactile, alimentation, memoire du firmware : un message commence au bit 25, finit au bit 24, reponse = commande | 0x80 et 0 (succes) */
        static u32 command[32];
        if (data & (1u << 25)) command[tag] = (data >> 8) & 0x7F;
        if (data & (1u << 24)) {
            io_fifo_push(tag | ((((command[tag] | 0x80) << 8)) << 6));
            io_raise(1u << 18);
        }
        return;
    }
    if (tag == 5) {
        /* horloge : la reponse suffit, RTC_GetDateTime est remplace dans hle.c */
        io_fifo_push(5u | ((((data >> 8) & 0x7F) | 0x80) << 8) << 6);
        io_raise(1u << 18);
        return;
    }
    if (logged++ < 40) nds_log("pxi tag=%u err=%u data=%x", tag, (word >> 5) & 1, data);
}

void snd_start_output(void);
void arm7_init(void) {
    save_load();
    snd_start_output();
}
void arm7_tick(void) {
    hle_frame();
    snd_frame();
}
