/* nitro-pc : runtime natif des jeux DS decompiles. Le code du jeu tourne en x86, la memoire est aux adresses DS. */
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

#define REG32(a) (*(volatile u32 *)(a))
#define REG16(a) (*(volatile u16 *)(a))
#define REG8(a) (*(volatile u8 *)(a))
#define IO 0x04000000u

/* Windows */
#define WINAPI __attribute__((stdcall))
#define IMPORT __attribute__((dllimport))
IMPORT void *WINAPI VirtualAlloc(void *, unsigned long, unsigned long, unsigned long);
IMPORT int WINAPI VirtualProtect(void *, unsigned long, unsigned long, unsigned long *);
IMPORT void *WINAPI AddVectoredExceptionHandler(unsigned long, void *);
IMPORT void *WINAPI CreateFileA(const char *, unsigned long, unsigned long, void *, unsigned long, unsigned long, void *);
IMPORT int WINAPI ReadFile(void *, void *, unsigned long, unsigned long *, void *);
IMPORT int WINAPI WriteFile(void *, const void *, unsigned long, unsigned long *, void *);
IMPORT unsigned long WINAPI GetFileSize(void *, unsigned long *);
IMPORT int WINAPI CloseHandle(void *);
IMPORT void *WINAPI GetStdHandle(unsigned long);
IMPORT void WINAPI ExitProcess(unsigned);
IMPORT void WINAPI Sleep(unsigned long);
IMPORT int WINAPI QueryPerformanceCounter(s64 *);
IMPORT int WINAPI QueryPerformanceFrequency(s64 *);
IMPORT const char *WINAPI GetCommandLineA(void);
IMPORT void *WINAPI CreateFileMappingA(void *, void *, unsigned long, unsigned long, unsigned long, const char *);
IMPORT void *WINAPI MapViewOfFile(void *, unsigned long, unsigned long, unsigned long, unsigned long);
IMPORT void *WINAPI CreateThread(void *, unsigned long, void *, void *, unsigned long, unsigned long *);
IMPORT int WINAPI SetThreadPriority(void *, int);
IMPORT void *WINAPI GetCurrentThread(void);
IMPORT void *WINAPI OpenFileMappingA(unsigned long, int, const char *);
IMPORT void *WINAPI CreateEventA(void *, int, int, const char *);
IMPORT int WINAPI SetEvent(void *);
IMPORT unsigned long WINAPI WaitForSingleObject(void *, unsigned long);
IMPORT unsigned long WINAPI GetActiveProcessorCount(unsigned short);

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
