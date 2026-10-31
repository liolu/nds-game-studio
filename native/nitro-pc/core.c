/* Demarrage : memoire aux adresses DS, chargement de la rom, exceptions pour les registres, BIOS, threads. */
#include "nitro.h"

/* sans en-tete systeme : la boite a outils du launcher n'en livre aucun */
typedef __builtin_va_list va_list;
#define va_start(ap, last) __builtin_va_start(ap, last)
#define va_arg(ap, type) __builtin_va_arg(ap, type)
#define va_end(ap) __builtin_va_end(ap)

u32 nds_cpsr = 0x1F;
u32 nds_pc, nds_r[6]; /* derniere instruction ARM et registres, dans les versions de debug */
u8 *nds_rom;
u32 nds_rom_size;

extern const struct { u32 arm; void *x86; } nds_fn_table[];
extern const unsigned nds_fn_count;
extern const struct { u32 at; void *x86; } nds_fixups[];
extern const unsigned nds_fixup_count;
extern void *const nds_entry_fn;

/* ------------------------------------------------------------------ petites fonctions sans libc */
void nds_copy(void *dst, const void *src, u32 n) {
    __asm__ volatile("rep movsb" : "+D"(dst), "+S"(src), "+c"(n) : : "memory");
}
void nds_zero(void *dst, u32 n) {
    __asm__ volatile("rep stosb" : "+D"(dst), "+c"(n) : "a"(0) : "memory");
}

static int fmt_u(char *o, u32 v, int base, int width) {
    char tmp[16];
    int n = 0, w = 0;
    do { tmp[n++] = "0123456789abcdef"[v % base]; v /= base; } while (v);
    while (n < width) tmp[n++] = '0';
    while (n) o[w++] = tmp[--n];
    return w;
}

static void vlog(const char *fmt, va_list ap) {
    char buf[1024];
    int w = 0;
    for (; *fmt && w < 900; fmt++) {
        if (*fmt != '%') { buf[w++] = *fmt; continue; }
        fmt++;
        int width = 0;
        while (*fmt >= '0' && *fmt <= '9') width = width * 10 + *fmt++ - '0';
        if (*fmt == 'x') w += fmt_u(buf + w, va_arg(ap, u32), 16, width);
        else if (*fmt == 'd') {
            int v = va_arg(ap, int);
            if (v < 0) { buf[w++] = '-'; v = -v; }
            w += fmt_u(buf + w, v, 10, width);
        } else if (*fmt == 'u') w += fmt_u(buf + w, va_arg(ap, u32), 10, width);
        else if (*fmt == 's') { const char *s = va_arg(ap, const char *); while (s && *s && w < 900) buf[w++] = *s++; }
        else buf[w++] = *fmt;
    }
    buf[w++] = '\n';
    unsigned long written;
    WriteFile(GetStdHandle((unsigned long)-12), buf, w, &written, 0);
}

void nds_log(const char *fmt, ...) { va_list ap; va_start(ap, fmt); vlog(fmt, ap); va_end(ap); }

void nds_fatal(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    vlog(fmt, ap);
    va_end(ap);
    ExitProcess(3);
}

void nds_unhandled(const char *f, u32 pc) { nds_fatal("instruction non traduite %s @%x", f, pc); }
void nds_missing(const char *name) { nds_fatal("fonction absente : %s", name); }

/* ------------------------------------------------------------------ arguments */
static const char *argv_[32];
static int argc_;

static void parse_args(void) {
    static char line[2048];
    const char *c = GetCommandLineA();
    int n = 0;
    while (*c && n < 2047) line[n++] = *c++;
    line[n] = 0;
    char *p = line;
    while (*p && argc_ < 32) {
        while (*p == ' ') p++;
        if (!*p) break;
        char end = ' ';
        if (*p == '"') { end = '"'; p++; }
        argv_[argc_++] = p;
        while (*p && *p != end) p++;
        if (*p) *p++ = 0;
    }
}

int nds_arg(const char *name, const char **value) {
    for (int i = 1; i < argc_; i++) {
        const char *a = argv_[i], *b = name;
        while (*a && *a == *b) a++, b++;
        if (*a == 0 && *b == 0) {
            if (value) *value = i + 1 < argc_ ? argv_[i + 1] : 0;
            return 1;
        }
    }
    return 0;
}

#ifdef NDS_INSTRUMENT
/* --watch adresse : qui ecrit a ce mot de memoire (ecritures du code traduit) */
u32 nds_watch = 1;
void nds_watch_hit(u32 a, u32 v) {
    static u32 hits;
    if (hits++ < 12) nds_log("ecriture %x = %x par arm=%x, r0=%x r1=%x r2=%x r3=%x image %u", a, v, nds_pc, nds_r[0], nds_r[1], nds_r[2], nds_r[3], nds_frame);
}
u32 nds_ring[4096], nds_ring_pos;
void __cyg_profile_func_enter(void *f, void *c) { nds_ring[nds_ring_pos++ & 4095] = (u32)f; }
void __cyg_profile_func_exit(void *f, void *c) {}
#endif

/* ------------------------------------------------------------------ pointeurs de fonctions ARM -> x86 */
u32 nds_call_target(u32 a) {
    u32 key = a & ~1u;
    if (key < 0x01000000u || key >= 0x03000000u) return a; /* deja une adresse x86 */
    unsigned lo = 0, hi = nds_fn_count;
    while (lo < hi) {
        unsigned mid = (lo + hi) / 2;
        if (nds_fn_table[mid].arm < key) lo = mid + 1;
        else hi = mid;
    }
    if (lo < nds_fn_count && nds_fn_table[lo].arm == key) return (u32)nds_fn_table[lo].x86;
    nds_fatal("saut vers une adresse DS sans fonction : %x", a);
    return 0;
}

/* ------------------------------------------------------------------ CP15 */
static u32 cp15_dtcm = 0x027E000Au, cp15_ctrl = 0x00052078u;

u32 nds_cp15_read(u32 crn, u32 crm, u32 op2) {
    if (crn == 1) return cp15_ctrl;
    if (crn == 9 && crm == 1) return op2 == 0 ? cp15_dtcm : 0x20u;
    if (crn == 0) return op2 == 1 ? 0x0F0D2112u : 0x41059461u;
    return 0;
}

void nds_cp15_write(u32 crn, u32 crm, u32 op2, u32 v) {
    if (crn == 1) cp15_ctrl = v;
    else if (crn == 9 && crm == 1 && op2 == 0) cp15_dtcm = v;
    else if (crn == 7 && crm == 0 && op2 == 4) io_wait_irq(); /* attente d'interruption */
}

void nds_cpsr_changed(void) {
    /* un jeu peut attendre un VBlank sans toucher aux registres : le temps avance aussi ici */
    if (!(nds_cpsr & 0x80)) {
        io_poll();
        irq_deliver();
    }
}

/* ------------------------------------------------------------------ BIOS (swi) */
static u32 lz77(u32 src, u32 dst, int vram) {
    u32 head = *(u32 *)src;
    u32 left = head >> 8, s = src + 4, d = dst;
    while (left) {
        u8 flags = *(u8 *)s++;
        for (int i = 0; i < 8 && left; i++, flags <<= 1) {
            if (flags & 0x80) {
                u32 b = (*(u8 *)s << 8) | *(u8 *)(s + 1);
                s += 2;
                u32 n = (b >> 12) + 3, off = (b & 0xFFF) + 1;
                for (u32 k = 0; k < n && left; k++, left--, d++) {
                    u8 v = *(u8 *)(d - off);
                    if (vram) {
                        u16 *p = (u16 *)(d & ~1u);
                        *p = (d & 1) ? (*p & 0xFF) | (v << 8) : (*p & 0xFF00) | v;
                    } else *(u8 *)d = v;
                }
            } else {
                u8 v = *(u8 *)s++;
                if (vram) {
                    u16 *p = (u16 *)(d & ~1u);
                    *p = (d & 1) ? (*p & 0xFF) | (v << 8) : (*p & 0xFF00) | v;
                } else *(u8 *)d = v;
                d++; left--;
            }
        }
    }
    return head >> 8;
}

static void rl_uncomp(u32 src, u32 dst, int vram) {
    u32 left = *(u32 *)src >> 8, s = src + 4, d = dst;
    while (left) {
        u8 c = *(u8 *)s++;
        u32 n = (c & 0x80) ? (c & 0x7F) + 3 : (c & 0x7F) + 1;
        u8 rep = (c & 0x80) ? *(u8 *)s++ : 0;
        for (u32 k = 0; k < n && left; k++, left--, d++) {
            u8 v = (c & 0x80) ? rep : *(u8 *)s++;
            if (vram) {
                u16 *p = (u16 *)(d & ~1u);
                *p = (d & 1) ? (*p & 0xFF) | (v << 8) : (*p & 0xFF00) | v;
            } else *(u8 *)d = v;
        }
    }
}

static void huff_uncomp(u32 src, u32 dst) {
    u32 head = *(u32 *)src;
    u32 left = head >> 8, bits = head & 15;
    u8 *tree = (u8 *)(src + 4);
    u32 *in = (u32 *)(src + 4 + (tree[0] + 1) * 2);
    u32 acc = 0, acc_bits = 0, node = 0;
    u8 *cur = tree + 1;
    (void)node;
    u32 out = 0, out_bits = 0;
    u8 *d = (u8 *)dst;
    while (left) {
        u32 word = *in++;
        for (int i = 31; i >= 0 && left; i--) {
            u32 b = (word >> i) & 1;
            u8 v = *cur;
            u8 *next = (u8 *)(((u32)cur & ~1u) + (v & 0x3F) * 2 + 2);
            u32 is_leaf = b ? (v & 0x40) : (v & 0x80);
            cur = next + b;
            if (is_leaf) {
                out |= (u32)*cur << out_bits;
                out_bits += bits;
                cur = tree + 1;
                if (out_bits >= 8) { *d++ = (u8)out; out >>= 8; out_bits -= 8; left--; }
            }
        }
    }
    (void)acc; (void)acc_bits;
}

static void bit_unpack(u32 src, u32 dst, u32 info) {
    u16 len = *(u16 *)info;
    u8 sw = *(u8 *)(info + 2), dw = *(u8 *)(info + 3);
    u32 off = *(u32 *)(info + 4);
    int zero = off >> 31;
    off &= 0x7FFFFFFF;
    u32 acc = 0, acc_bits = 0, out = dst;
    for (u32 i = 0; i < len; i++) {
        u8 b = *(u8 *)(src + i);
        for (u32 k = 0; k < 8; k += sw) {
            u32 v = (b >> k) & ((1u << sw) - 1);
            if (v || zero) v += off;
            acc |= v << acc_bits;
            acc_bits += dw;
            if (acc_bits >= 32) { *(u32 *)out = acc; out += 4; acc = 0; acc_bits = 0; }
        }
    }
}

static u32 isqrt(u32 x) {
    u32 r = 0, b = 1u << 30;
    while (b > x) b >>= 2;
    while (b) {
        if (x >= r + b) { x -= r + b; r = (r >> 1) + b; }
        else r >>= 1;
        b >>= 2;
    }
    return r;
}

void nds_swi(u32 n, u32 *r0, u32 *r1, u32 *r2, u32 *r3) {
#ifdef NDS_INSTRUMENT
    if (n >= 0x0B && n != 0x0D && n != 0x0E && *r1 <= nds_watch && nds_watch < *r1 + 0x10000) nds_log("swi %x %x -> %x r2=%x arm=%x", n, *r0, *r1, *r2, nds_pc);
#endif
    switch (n) {
    case 0x03: break;                                     /* WaitByLoop */
    case 0x04: case 0x05: {                               /* IntrWait, VBlankIntrWait */
        u32 mask = n == 5 ? 1 : *r1;
        volatile u32 *check = (volatile u32 *)0x027E3FF8u;
        if (n == 5 || *r0) *check &= ~mask;
        u32 saved = nds_cpsr;
        nds_cpsr &= ~0x80u;
        while (!(*check & mask)) io_wait_irq();
        *check &= ~mask;
        nds_cpsr = saved;
        break;
    }
    case 0x06: io_wait_irq(); break;                      /* Halt */
    case 0x09: {                                          /* Div */
        s32 num = (s32)*r0, den = (s32)*r1;
        if (den == 0) { *r0 = num < 0 ? -1 : 1; *r1 = num; *r3 = 1; break; }
        s32 q = (num == (s32)0x80000000 && den == -1) ? num : num / den;
        *r0 = q; *r1 = num - q * den; *r3 = q < 0 ? -q : q;
        break;
    }
    case 0x0B: {                                          /* CpuSet */
        u32 s = *r0, d = *r1, c = *r2, count = c & 0x1FFFFF;
        if (c & (1 << 26)) {
            u32 v = c & (1 << 24) ? *(u32 *)s : 0;
            for (u32 i = 0; i < count; i++, d += 4, s += (c & (1 << 24)) ? 0 : 4) *(u32 *)d = (c & (1 << 24)) ? v : *(u32 *)s;
        } else {
            u16 v = *(u16 *)s;
            for (u32 i = 0; i < count; i++, d += 2, s += (c & (1 << 24)) ? 0 : 2) *(u16 *)d = (c & (1 << 24)) ? v : *(u16 *)s;
        }
        break;
    }
    case 0x0C: {                                          /* CpuFastSet */
        u32 s = *r0, d = *r1, c = *r2, count = (c & 0x1FFFFF);
        count = (count + 7) & ~7u;
        u32 v = *(u32 *)s;
        for (u32 i = 0; i < count; i++, d += 4, s += 4) *(u32 *)d = (c & (1 << 24)) ? v : *(u32 *)s;
        break;
    }
    case 0x0D: *r0 = isqrt(*r0); break;                   /* Sqrt */
    case 0x0E: {                                          /* GetCRC16 */
        static const u16 tab[16] = {0x0000,0xCC01,0xD801,0x1400,0xF001,0x3C00,0x2800,0xE401,0xA001,0x6C00,0x7800,0xB401,0x5000,0x9C01,0x8801,0x4400};
        u32 crc = *r0 & 0xFFFF;
        u8 *p = (u8 *)*r1;
        for (u32 i = 0; i < *r2; i += 2) {
            u32 w = p[i] | (p[i + 1] << 8);
            for (int k = 0; k < 4; k++, w >>= 4) {
                u32 t = tab[crc & 15];
                crc = (crc >> 4) ^ t ^ tab[w & 15];
            }
        }
        *r0 = crc;
        break;
    }
    case 0x10: bit_unpack(*r0, *r1, *r2); break;
    case 0x11: lz77(*r0, *r1, 0); break;
    case 0x12: lz77(*r0, *r1, 1); break;
    case 0x13: huff_uncomp(*r0, *r1); break;
    case 0x14: rl_uncomp(*r0, *r1, 0); break;
    case 0x15: rl_uncomp(*r0, *r1, 1); break;
    default: nds_fatal("swi %x inconnu", n);
    }
}

/* ------------------------------------------------------------------ threads : contexte x86 a cote de l'OSContext du SDK */
typedef struct {
    u32 ctx, esp, cpsr, func, fresh, ebp, ebx, esi, edi, eip;
} Slot;
static Slot slots[64];
static u32 last_load_esp;

static Slot *slot_of(u32 ctx) {
    for (int i = 0; i < 64; i++)
        if (slots[i].ctx == ctx) return &slots[i];
    for (int i = 0; i < 64; i++)
        if (!slots[i].ctx) { slots[i].ctx = ctx; return &slots[i]; }
    nds_fatal("trop de threads");
    return 0;
}

Slot *nds_save_slot(u32 ctx) {
    Slot *s = slot_of(ctx);
    s->cpsr = nds_cpsr;
    s->fresh = 0;
    return s;
}

Slot *nds_load_slot(u32 ctx) {
    Slot *s = slot_of(ctx);
    nds_cpsr = s->cpsr;
    if (!s->esp) nds_fatal("contexte sans sauvegarde %x", ctx);
    last_load_esp = s->esp;
    return s;
}

/* comme setjmp/longjmp : les registres vont dans le Slot, pas sur la pile que l'appelant reutilise ensuite */
__asm__(
    ".globl _OS_SaveContext\n_OS_SaveContext:\n"
    "  mov 4(%esp), %eax\n  push %eax\n  call _nds_save_slot\n  add $4, %esp\n"
    "  mov %ebp, 20(%eax)\n  mov %ebx, 24(%eax)\n  mov %esi, 28(%eax)\n  mov %edi, 32(%eax)\n"
    "  mov (%esp), %ecx\n  mov %ecx, 36(%eax)\n  lea 4(%esp), %ecx\n  mov %ecx, 4(%eax)\n"
    "  xor %eax, %eax\n  ret\n"
    ".globl _OS_LoadContext\n_OS_LoadContext:\n"
    "  mov 4(%esp), %eax\n  push %eax\n  call _nds_load_slot\n"
    "  mov 4(%eax), %esp\n  mov 20(%eax), %ebp\n  mov 24(%eax), %ebx\n  mov 28(%eax), %esi\n  mov 32(%eax), %edi\n"
    "  mov 36(%eax), %ecx\n  mov $1, %eax\n  jmp *%ecx\n");

typedef u64 (*Fn)();

void nds_dump_threads(void) {
    for (int i = 0; i < 64; i++) {
        if (!slots[i].ctx || !slots[i].esp) continue;
        u32 state = *(u32 *)(slots[i].ctx + 0x64);
        nds_log("  thread ctx=%x etat=%u eip=%8x", slots[i].ctx, state, slots[i].eip);
        nds_log("    %8x", slots[i].eip);
        u32 *st = (u32 *)slots[i].esp;
        int k = 0;
        for (int j = 0; j < 60 && k < 8; j++)
            if (st[j] >= 0x10000000u && st[j] < 0x11000000u) { nds_log("    %8x", st[j]); k++; }
    }
}

/* piles x86 au-dessus de la zone DS, jamais sous 0x01000000 que le code traduit reserve au miroir ITCM */
static u32 next_stack = 0x20000000u;
static u8 *stack_alloc(u32 size) {
    u8 *p = VirtualAlloc((void *)next_stack, size, 0x3000, 4);
    if (!p) nds_fatal("pile impossible a %x", next_stack);
    next_stack += (size + 0xFFFF) & ~0xFFFFu;
    return p;
}

static void thread_tramp(u32 ctx) {
    Slot *s = slot_of(ctx);
    u32 *c = (u32 *)ctx;
    /* OSContext : cpsr, r0..r12, sp, lr, pc+4 */
    u32 arg = c[1], lr = c[15];
    ((Fn)s->func)(arg, 0, 0, 0);
    if (lr) ((Fn)nds_call_target(lr))(0, 0, 0, 0);
    nds_fatal("fin de thread sans suite");
}

void OS_InitContext(u32 ctx, u32 pc, u32 sp) {
    Slot *s = slot_of(ctx);
    u32 *c = (u32 *)ctx;
    enum { STACK = 1 << 20 };
    u8 *stack = stack_alloc(STACK);
    u32 *top = (u32 *)(stack + STACK - 64);
    top[1] = ctx;     /* argument de thread_tramp */
    top[0] = 0;       /* adresse de retour fictive */
    s->esp = (u32)top;
    s->eip = (u32)thread_tramp;
    s->ebp = s->ebx = s->esi = s->edi = 0;
    s->func = pc;
    s->cpsr = 0x1F;
    s->fresh = 1;
    nds_zero(c, 0x48);
    c[0] = 0x1F;
    c[14] = sp;
    c[16] = pc + 4;
}

/* ------------------------------------------------------------------ exceptions : registres a effets */
#define EXC_ACCESS 0xC0000005u
#define EXC_STEP 0x80000004u
#define EXC_CONTINUE (-1)
#define EXC_NEXT 0
#define PAGE_RW 4
#define PAGE_NONE 1

typedef struct { u32 code, flags; void *rec; void *addr; u32 nparams; u32 info[15]; } ExcRecord;
/* CONTEXT x86 : seuls Esp (196), Eip (184), EFlags (192) servent */
typedef struct { ExcRecord *rec; u8 *ctx; } ExcPointers;
#define CTX_U32(c, off) (*(u32 *)((c) + (off)))
#define CTX_EIP 184
#define CTX_EFLAGS 192
#define CTX_ESP 196

int nds_trace;
u32 nds_trace_count, nds_trace_eip, nds_trace_addr, nds_trace_stack[13];
static u32 pending_page, pending_addr, pending_write;
void nds_irq_tramp(void);
__asm__(".globl _nds_irq_tramp\n_nds_irq_tramp:\n  pushf\n  pusha\n  cld\n  call _irq_deliver\n  popa\n  popf\n  ret\n");

static void io_protect(u32 page, int on) {
    unsigned long old;
    VirtualProtect((void *)page, 0x1000, on ? PAGE_NONE : PAGE_RW, &old);
}

static long WINAPI on_exception(ExcPointers *e) {
    ExcRecord *r = e->rec;
    if (r->code == EXC_ACCESS) {
        u32 addr = r->info[1];
        u32 page = addr & ~0xFFFu;
        if (page == IO || page == 0x04100000u) {
            io_protect(IO, 0);
            io_protect(0x04100000u, 0);
            if (nds_trace) {
                nds_trace_count++;
                nds_trace_eip = (u32)r->addr;
                nds_trace_addr = addr;
                u32 *st = (u32 *)CTX_U32(e->ctx, CTX_ESP);
                int k = 0;
                for (int i = 0; i < 200 && k < 12; i++)
                    if (st[i] >= 0x10000000u && st[i] < 0x11000000u) nds_trace_stack[k++] = st[i];
                nds_trace_stack[k] = 0;
            }
            pending_page = page;
            pending_addr = addr;
            pending_write = r->info[0] == 1;
            if (!pending_write) io_before_read(addr);
            CTX_U32(e->ctx, CTX_EFLAGS) |= 0x100;
            return EXC_CONTINUE;
        }
        nds_log("acces memoire invalide %x (%s) eip=%x esp=%x arm=%x", addr, r->info[0] ? "ecriture" : "lecture", (u32)r->addr, CTX_U32(e->ctx, CTX_ESP), nds_pc);
#ifdef NDS_INSTRUMENT
        nds_log("  r0=%x r1=%x r2=%x r3=%x r4=%x r5=%x", nds_r[0], nds_r[1], nds_r[2], nds_r[3], nds_r[4], nds_r[5]);
        for (int k = 0; k < 6; k++) {
            u32 a = nds_r[k] & ~3u;
            if (a >= 0x02000000u && a < 0x02400000u) {
                nds_log("  [r%d] %8x %8x %8x %8x %8x %8x %8x %8x", k, ((u32 *)a)[0], ((u32 *)a)[1], ((u32 *)a)[2], ((u32 *)a)[3], ((u32 *)a)[4], ((u32 *)a)[5], ((u32 *)a)[6], ((u32 *)a)[7]);
            }
        }
#endif
        u32 *st = (u32 *)CTX_U32(e->ctx, CTX_ESP);
        u32 *bp = (u32 *)CTX_U32(e->ctx, 180);
#ifdef NDS_INSTRUMENT
        {
            extern u32 nds_ring[4096], nds_ring_pos;
            nds_log("  derniers appels :");
            for (int i = 4096 - 1500; i < 4096; i++) nds_log("    %8x", nds_ring[(nds_ring_pos + i) & 4095]);
        }
#endif
        nds_log("  ebp=%x retours :", (u32)bp);
        if (last_load_esp) {
            nds_log("  pile du dernier contexte charge %x :", last_load_esp);
            for (int i = 0; i < 400; i++) {
                u32 v = ((u32 *)last_load_esp)[i];
                if (v >= 0x10000000u && v < 0x11000000u) nds_log("    %8x", v);
            }
            nds_log("  fin");
        }
        for (int i = 0; i < 40; i++) {
            u32 v = st[i];
            if (v >= 0x10000000u && v < 0x11000000u) nds_log("    %x", v);
        }
        ExitProcess(4);
    }
    if (r->code == EXC_STEP && pending_page) {
        u32 page = pending_page, addr = pending_addr;
        pending_page = 0;
        if (pending_write) io_after_write(addr);
        int ready = io_irq_ready();
        io_protect(IO, 1);
        io_protect(0x04100000u, 1);
        CTX_U32(e->ctx, CTX_EFLAGS) &= ~0x100u;
        if (ready) {
            u32 esp = CTX_U32(e->ctx, CTX_ESP) - 4;
            *(u32 *)esp = CTX_U32(e->ctx, CTX_EIP);
            CTX_U32(e->ctx, CTX_ESP) = esp;
            CTX_U32(e->ctx, CTX_EIP) = (u32)nds_irq_tramp;
        }
        return EXC_CONTINUE;
    }
    nds_log("exception %x a %x", r->code, (u32)r->addr);
    ExitProcess(5);
    return EXC_NEXT;
}

/* ------------------------------------------------------------------ chargement */
static void reserve(u32 addr, u32 size) {
    void *p = VirtualAlloc((void *)addr, size, 0x3000, PAGE_RW);
    if (p != (void *)addr) nds_fatal("memoire DS indisponible a %x", addr);
}

static void load_file(const char *path) {
    void *h = CreateFileA(path, 0x80000000u, 1, 0, 3, 0, 0);
    if (h == (void *)-1) nds_fatal("rom introuvable : %s", path);
    nds_rom_size = GetFileSize(h, 0);
    nds_rom = VirtualAlloc(0, nds_rom_size, 0x3000, PAGE_RW);
    unsigned long got;
    ReadFile(h, nds_rom, nds_rom_size, &got, 0);
    CloseHandle(h);
}

/* decompression BLZ de arm9 (identique au MIi_UncompressBackward du SDK, faite ici car le module charge avant _start) */
static void blz_uncompress(u32 end) {
    u32 sizes = *(u32 *)(end - 8), extra = *(u32 *)(end - 4);
    u32 start = end - (sizes >> 24);
    u32 stop = end - (sizes & 0xFFFFFF);
    u8 *src = (u8 *)start, *dst = (u8 *)(end + extra);
    while (src > (u8 *)stop) {
        u8 flags = *--src;
        for (int i = 0; i < 8 && src > (u8 *)stop; i++, flags <<= 1) {
            if (flags & 0x80) {
                u32 b = *--src << 8;
                b |= *--src;
                u32 n = (b >> 12) + 3, off = (b & 0xFFF) + 3;
                while (n--) { dst--; *dst = dst[off]; }
            } else { *--dst = *--src; }
        }
    }
}

extern const u32 nds_build_info_offset;

static void load_arm9(void) {
    u8 *h = nds_rom;
    u32 off = *(u32 *)(h + 0x20), ram = *(u32 *)(h + 0x28), size = *(u32 *)(h + 0x2C);
    nds_copy((void *)ram, nds_rom + off, size);
    nds_copy((void *)0x027FFE00u, h, 0x170); /* en-tete de carte, lu par le SDK pour la FAT */
    u32 *info = (u32 *)(ram + nds_build_info_offset);
    if (info[5]) {
        blz_uncompress(info[5]);
        info[5] = 0;
    }
    /* _start recopie ITCM/DTCM depuis cette source : les pointeurs de fonctions sont corriges dans la source */
    u32 *list = (u32 *)info[0], *end = (u32 *)info[1];
    for (unsigned i = 0; i < nds_fixup_count; i++) {
        u32 at = nds_fixups[i].at, src = info[2];
        /* SDK des jeux DSi : entrees de 16 octets au lieu de 12 */
        u32 step = ((u32)end - (u32)list) % 12 == 0 ? 3 : 4;
        for (u32 *e = list; e < end; e += step) {
            if (at >= e[0] && at < e[0] + e[1]) { at = src + (at - e[0]); break; }
            src += e[1];
        }
        *(u32 *)at = (u32)nds_fixups[i].x86;
    }
}

/* profil par echantillons (--prof) : ou le jeu passe son temps */
IMPORT unsigned long WINAPI SuspendThread(void *);
IMPORT unsigned long WINAPI ResumeThread(void *);
IMPORT int WINAPI GetThreadContext(void *, void *);
IMPORT int WINAPI DuplicateHandle(void *, void *, void *, void **, unsigned long, int, unsigned long);
IMPORT void *WINAPI GetCurrentProcess(void);
static void *main_thread;

static unsigned long WINAPI prof_thread(void *arg) {
    static u32 addrs[256], counts[256];
    static u32 ctx[256];
    u32 total = 0;
    for (;;) {
        Sleep(5);
        ctx[0] = 0x10001;
        SuspendThread(main_thread);
        GetThreadContext(main_thread, ctx);
        ResumeThread(main_thread);
        u32 eip = ctx[184 / 4] & ~0xFu;
        total++;
        for (int i = 0; i < 256; i++) {
            if (addrs[i] == eip) { counts[i]++; break; }
            if (!addrs[i]) { addrs[i] = eip; counts[i] = 1; break; }
        }
        if (total % 1000 == 0) {
            nds_log("profil (%u echantillons, image %u) :", total, nds_frame);
#ifdef NDS_INSTRUMENT
            if (total == 2000) {
                extern u32 nds_ring[4096], nds_ring_pos;
                u32 pos = nds_ring_pos;
                for (int i = 0; i < 4096; i++) nds_log("  appel %8x", nds_ring[(pos + i) & 4095]);
            }
#endif
            {
                u32 *st = (u32 *)ctx[196 / 4];
                int shown = 0;
                for (u32 i = 0; i < 4000 && shown < 24; i++)
                    if (st[i] >= 0x10000000u && st[i] < 0x11000000u) { nds_log("  pile %8x", st[i]); shown++; }
            }
            for (int k = 0; k < 14; k++) {
                int best = -1;
                for (int i = 0; i < 256; i++) if (counts[i] && (best < 0 || counts[i] > counts[best])) best = i;
                if (best < 0) break;
                nds_log("  %u %8x", counts[best], addrs[best]);
                counts[best] = 0;
            }
            for (int i = 0; i < 256; i++) { addrs[i] = 0; counts[i] = 0; }
        }
    }
    return 0;
}

static void run_game(void) {
    ((Fn)nds_entry_fn)(0, 0, 0, 0);
    nds_fatal("le jeu s'est termine");
}

void nds_start(void) {
    parse_args();
    AddVectoredExceptionHandler(1, on_exception);
    reserve(0x01FF0000u, 0x10000);   /* ITCM (0x01FF8000), reservation alignee sur 64 Ko */
    reserve(0x02000000u, 0x02000000u); /* RAM principale, miroirs, WRAM partagee et ARM7 */
    reserve(0x04000000u, 0x00200000u); /* registres */
    reserve(0x05000000u, 0x00001000u); /* palettes */
    reserve(0x06000000u, 0x00A00000u); /* VRAM */
    reserve(0x07000000u, 0x00001000u); /* OAM */
    reserve(0x08000000u, 0x02010000u); /* slot GBA : vide */
    const char *rom = argc_ > 1 ? argv_[1] : 0;
    if (!rom) nds_fatal("usage : jeu.exe <rom.nds>");
    load_file(rom);
    load_arm9();
    arm7_init();
    io_init();
    present_init();
    {
        extern const int nds_has_c;
        if (nds_has_c) {
            io_protect(IO, 1);
            io_protect(0x04100000u, 1);
        }
    }
    nds_log("demarrage %x", (u32)nds_entry_fn);
#ifdef NDS_INSTRUMENT
    {
        const char *w;
        if (nds_arg("--watch", &w) && w) {
            u32 v = 0;
            for (; *w; w++) v = v * 16 + (*w >= 'a' ? *w - 'a' + 10 : *w - '0');
            nds_watch = v & ~3u;
        }
    }
#endif
    if (nds_arg("--prof", 0)) {
        DuplicateHandle(GetCurrentProcess(), GetCurrentThread(), GetCurrentProcess(), &main_thread, 0, 0, 2);
        CreateThread(0, 0x10000, prof_thread, 0, 0, 0);
    }
    /* grande pile, allouee apres la memoire DS pour ne pas la gener */
    enum { MAIN_STACK = 16 << 20 };
    u8 *stack = stack_alloc(MAIN_STACK);
    __asm__ volatile("mov %0, %%esp\n  call *%1\n" : : "r"(stack + MAIN_STACK - 64), "r"(run_game));
    nds_fatal("le jeu s'est termine");
}
