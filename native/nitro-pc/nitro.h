/* nitro-pc : runtime natif des jeux DS decompiles. Le code du jeu tourne en natif, la memoire DS est dans une fenetre (DS()). */
#ifndef NITRO_H
#define NITRO_H

typedef unsigned int u32;
typedef int s32;
typedef unsigned short u16;
typedef short s16;
typedef unsigned char u8;
typedef signed char s8;
typedef unsigned long long u64;
typedef long long s64;
typedef __UINTPTR_TYPE__ uptr; /* entier de la taille d'un pointeur de l'hote */

/* Memoire DS : l'adresse DS a vit a nds_mem + a, dans une fenetre de 4 Go reservee au demarrage (native/MACOS.md 3.1).
 * Windows 32 bits : nds_mem est la constante 0, la memoire est aux adresses DS et DS()/HOST2DS() ne coutent rien.
 * DS(a) : adresse DS -> pointeur de l'hote. HOST2DS(p) : pointeur de l'hote dans la fenetre -> adresse DS.
 * Meme definitions dans le PRELUDE de tools/arm2c.py. */
#if defined(_WIN32) && defined(__i386__)
#define NDS_IDENTITY 1
#define nds_mem ((uptr)0)
#else
#define NDS_IDENTITY 0
extern uptr nds_mem;
#endif
#define DS(a) ((u8 *)(nds_mem + (u32)(a)))
#if defined(NDS_INSTRUMENT) && !NDS_IDENTITY
u32 nds_host2ds(const volatile void *p); /* verifie que p est dans la fenetre */
#define HOST2DS(p) nds_host2ds(p)
#else
#define HOST2DS(p) ((u32)((uptr)(p) - nds_mem))
#endif

/* Donnees du jeu que le runtime nomme. Windows 32 bits : symbole absolu a l'adresse DS (dsabs.s de native_build.py).
 * Ailleurs (ld64 n'a pas de symboles absolus utilisables) : le build fournit nds_addr_<nom>, voir MACOS.md 3.6. */
#if NDS_IDENTITY
#define GAME_SYM(name) extern char name[]
#define GAME_ADDR(name) ((u32)(uptr)(name))
#else
#define GAME_SYM(name) extern const u32 nds_addr_##name
#define GAME_ADDR(name) (nds_addr_##name)
#endif

/* registres et memoire DS a une adresse DS fixe */
#define REG32(a) (*(volatile u32 *)DS(a))
#define REG16(a) (*(volatile u16 *)DS(a))
#define REG8(a) (*(volatile u8 *)DS(a))
#define IO 0x04000000u

/* plateforme : memoire, fichiers, fils, temps, son (os_win32.c) */
#include "os.h"

/* core.c */
extern u32 nds_cpsr;
extern u8 *nds_rom;
extern u32 nds_rom_size;
extern u32 nds_frame;
void nds_log(const char *fmt, ...);
void nds_fatal(const char *fmt, ...);
void nds_copy(void *dst, const void *src, u32 n);
void nds_zero(void *dst, u32 n);
int nds_arg(const char *name, const char **value);
uptr nds_call_target(u32 a); /* adresse DS (ou deja x86) d'une fonction -> fonction de l'hote */

/* io.c */
void io_init(void);
void io_before_read(u32 addr);
void io_after_write(u32 addr);
int io_irq_ready(void);
void io_poll(void);
void io_wait_irq(void);
void io_raise(u32 bits);
u32 io_cycles(void);
void irq_deliver(void);
void dma_vblank(void);

/* gx3d.c */
void gx_write(u32 reg, u32 value);
void gx_fifo_word(u32 value);
void gx_begin_frame(void);
u32 gx_status(void);
const u32 *gx_render(int w, int h, int k, int margin);

/* gfx.c */
void gfx_render(void);
void vram_cnt_changed(void);
u8 *vram_bank(int index);
u8 *vram_tex_slot(int slot);
u8 *vram_texpal_slot(int slot);
void gfx_parallel(void (*fn)(void *arg, int band, int bands), void *arg);

/* present.c */
void present_init(void);
void present_frame(void);
u32 present_keys(void);
void present_touch(int *down, int *x, int *y);
void present_config(int *scale, int *ds_width);
void present_back(u32 **top, u32 **bottom);
void present_flip(int w, int h, u32 frame);

/* arm7.c */
void arm7_fifo_send(u32 word);
void arm7_init(void);
void arm7_tick(void);
void snd_start_output(void);

#endif
