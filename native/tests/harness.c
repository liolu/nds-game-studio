/* Test sans rom : PRELUDE de arm2c.py (prelude.h), fonctions de arm2c_cases.s traduites (lifted.c) et
 * nds_call_target (nitro-pc/calltab.c). Compile et lance par arm2c_test.py, qui genere prelude.h, cases.h et lifted.c. */
#include <setjmp.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>

#include "prelude.h"
#include "cases.h"

void nds_code_map_init(void);

#if !NDS_IDENTITY
uptr nds_mem;
#endif
u32 nds_pc, nds_r[6], nds_cpsr = 0x1F;

/* ------------------------------------------------------------------ verifications */
static int checks, failures;

static void check_eq(u64 got, u64 want, const char *what, int line) {
    checks++;
    if (got == want) return;
    failures++;
    printf("ECHEC ligne %d : %s = 0x%08x%08x, attendu 0x%08x%08x\n", line, what, (u32)(got >> 32), (u32)got,
           (u32)(want >> 32), (u32)want);
}
#define CHECK_EQ(a, b) check_eq((u64)(a), (u64)(b), #a, __LINE__)

/* nds_fatal : attendu par certains tests (adresse sans fonction), sinon le test s'arrete */
static jmp_buf fatal_jmp;
static volatile int fatal_armed;
void nds_fatal(const char *fmt, ...) {
    if (fatal_armed) {
        fatal_armed = 0;
        longjmp(fatal_jmp, 1);
    }
    va_list ap;
    va_start(ap, fmt);
    printf("fatal : ");
    vprintf(fmt, ap);
    printf("\n");
    va_end(ap);
    exit(3);
}
#define EXPECT_FATAL(expr) do { \
        fatal_armed = 1; \
        if (!setjmp(fatal_jmp)) { (void)(expr); fatal_armed = 0; check_eq(0, 1, "fatal attendu : " #expr, __LINE__); } \
        else check_eq(1, 1, #expr, __LINE__); \
    } while (0)

/* ------------------------------------------------------------------ ce que le code traduit demande au runtime */
void nds_cpsr_changed(void) {}
void nds_unhandled(const char *f, u32 pc) { nds_fatal("instruction non traduite %s @%x", f, pc); }
u32 nds_cp15_read(u32 crn, u32 crm, u32 op2) { return 0; }
void nds_cp15_write(u32 crn, u32 crm, u32 op2, u32 v) {}
void nds_swi(u32 n, u32 *r0, u32 *r1, u32 *r2, u32 *r3) { nds_fatal("swi %x", n); }

/* registres (0x04xxxxxx) : valeur reconnaissable. Le reste de ce que IS_IO envoie ici est de la memoire : sous Windows,
 * la pile du fil principal est sous 0x01000000, que IS_IO prend pour le miroir ITCM. */
static u32 io_reads, io_writes, io_last_addr, io_last_value;
u32 nds_io_read(u32 a, u32 size) {
    if ((a >> 24) != 4) return size == 4 ? *(u32 *)DS(a) : size == 2 ? *(u16 *)DS(a) : *DS(a);
    io_reads++;
    return 0xC0DE0000u | (a & 0xFFFF);
}
void nds_io_write(u32 a, u32 v, u32 size) {
    if ((a >> 24) != 4) {
        if (size == 4) *(u32 *)DS(a) = v;
        else if (size == 2) *(u16 *)DS(a) = (u16)v;
        else *DS(a) = (u8)v;
        return;
    }
    io_writes++;
    io_last_addr = a;
    io_last_value = v;
}

#ifdef NDS_INSTRUMENT
u32 nds_watch = 1;
void nds_watch_hit(u32 a, u32 v) {}
#if !NDS_IDENTITY
u32 nds_host2ds(const volatile void *p) {
    uptr d = (uptr)p - nds_mem;
    if (d > 0xFFFFFFFFu) nds_fatal("pointeur hors de la memoire DS : code traduit sur une pile hors de la fenetre");
    return (u32)d;
}
#endif
#endif

/* fonctions C appelees par le code traduit (h_pack10 : 10 arguments, comme t_pack10) */
static u32 host_calls;
u64 h_pack10(u32 a, u32 b, u32 c, u32 d, u32 e, u32 f, u32 g, u32 h, u32 i, u32 j) {
    host_calls++;
    return (u64)(i | j << 4) << 32 | (a | b << 4 | c << 8 | d << 12 | e << 16 | f << 20 | g << 24 | h << 28);
}
u64 h_itcm(u32 a, u32 b, u32 c, u32 d, u32 e, u32 f, u32 g, u32 h, u32 i, u32 j) { return a + 1; }
u64 h_dtcm(u32 a, u32 b, u32 c, u32 d, u32 e, u32 f, u32 g, u32 h, u32 i, u32 j) { return a + 2; }

#define C1(f, a) f(a, 0, 0, 0, 0, 0, 0, 0, 0, 0)
#define C2(f, a, b) f(a, b, 0, 0, 0, 0, 0, 0, 0, 0)
#define PACK10 0x000000A987654321ull /* 1..10, un quartet chacun, r0:r1 */

/* ------------------------------------------------------------------ tests */
static void test_memory(void) {
    WR32(0x02100000u, 0x11223344u);
    CHECK_EQ(RD32(0x02100000u), 0x11223344u);
    CHECK_EQ(RD16(0x02100002u), 0x1122u);
    CHECK_EQ(RD8(0x02100003u), 0x11u);
    CHECK_EQ(LDRW(0x02100000u), 0x11223344u);
    CHECK_EQ(LDRW(0x02100001u), 0x44112233u);
    CHECK_EQ(LDRW(0x02100002u), 0x33441122u);
    CHECK_EQ(LDRW(0x02100003u), 0x22334411u);
    WR16(0x02100004u, 0xBEEFu);
    WR8(0x02100006u, 0x5Au);
    CHECK_EQ(*(u32 *)DS(0x02100004u), 0x005ABEEFu);
    P32(0x02100008u) = 7;
    CHECK_EQ(RD32(0x02100008u), 7);
    /* registres : par nds_io_read/nds_io_write, pas par la memoire */
    io_reads = io_writes = 0;
    CHECK_EQ(RD32(0x04000130u), 0xC0DE0130u);
    CHECK_EQ(RD16(0x04000004u), 0xC0DE0004u);
    WR16(0x04000208u, 1);
    CHECK_EQ(io_reads, 2);
    CHECK_EQ(io_writes, 1);
    CHECK_EQ(io_last_addr, 0x04000208u);
    CHECK_EQ(io_last_value, 1);
    /* meme chose par le code traduit */
    CHECK_EQ((u32)C1(t_ldr, 0x02100001u), 0x44112233u);
    CHECK_EQ((u32)C1(t_ldr, 0x02100003u), 0x22334411u);
    CHECK_EQ((u32)C1(t_ldr, 0x04000130u), 0xC0DE0130u);
    C2(t_store, 0x02100011u, 0xAABBCCDDu);
    CHECK_EQ(*(u32 *)DS(0x02100010u), 0xAABBCCDDu);
    CHECK_EQ(*(u16 *)DS(0x02100016u), 0xCCDDu);
    CHECK_EQ(*DS(0x0210001Au), 0xDDu);
}

static void test_flag_macros(void) {
    u32 n = 0, z = 0, c = 0, v = 0, res = 0;
#define NZCV() (n << 3 | z << 2 | c << 1 | v)
    SUBF(1u, 2u, 1); CHECK_EQ(res, 0xFFFFFFFFu); CHECK_EQ(NZCV(), 8);
    SUBF(2u, 2u, 1); CHECK_EQ(NZCV(), 6);
    SUBF(0x80000000u, 1u, 1); CHECK_EQ(NZCV(), 3);
    SUBF(0u, 0x80000000u, 1); CHECK_EQ(NZCV(), 9);
    ADDF(0xFFFFFFFFu, 1u, 0); CHECK_EQ(NZCV(), 6);
    ADDF(0x7FFFFFFFu, 1u, 0); CHECK_EQ(NZCV(), 9);
    c = 1; ADDF(1u, 1u, c); CHECK_EQ(res, 3); CHECK_EQ(NZCV(), 0);    /* adc */
    c = 0; SUBF(5u, 3u, c); CHECK_EQ(res, 1); CHECK_EQ(c, 1);          /* sbc : 5 - 3 - 1 */
    NZ(0u); CHECK_EQ(n << 1 | z, 1);
    NZ(0x80000000u); CHECK_EQ(n << 1 | z, 2);
    c = 0; SC_LSL(0x80000001u, 1); CHECK_EQ(c, 1);
    c = 1; SC_LSL(0u, 0); CHECK_EQ(c, 1);                              /* decalage nul : C ne change pas */
    c = 0; SC_LSL(1u, 32); CHECK_EQ(c, 1);
    c = 1; SC_LSL(1u, 33); CHECK_EQ(c, 0);
    c = 0; SC_LSR(3u, 1); CHECK_EQ(c, 1);
    c = 0; SC_LSR(0x80000000u, 32); CHECK_EQ(c, 1);
    c = 0; SC_ASR(0x80000000u, 40); CHECK_EQ(c, 1);
    c = 0; SC_ROR(2u, 2); CHECK_EQ(c, 1);
    c = 0; SC_ROR(0x80000000u, 32); CHECK_EQ(c, 1);
    CHECK_EQ(LSL(1u, 31), 0x80000000u);
    CHECK_EQ(LSL(1u, 32), 0);
    CHECK_EQ(LSL(1u, 256), 1);                                         /* registre : 8 bits bas */
    CHECK_EQ(LSR(0x80000000u, 32), 0);
    CHECK_EQ(ASR(0x80000000u, 40), 0xFFFFFFFFu);
    CHECK_EQ(ROR(0x12345678u, 8), 0x78123456u);
    CHECK_EQ(ROR(0x12345678u, 32), 0x12345678u);
    CHECK_EQ(CLZ(0), 32);
    CHECK_EQ(CLZ(1), 31);
    CHECK_EQ((u32)SAT32(0x80000000LL), 0x7FFFFFFFu);
    CHECK_EQ((u32)SAT32(-0x80000001LL), 0x80000000u);
    n = 1; z = 0; c = 1; v = 0; SAVEF(); n = z = c = v = 0; LOADF(); CHECK_EQ(NZCV(), 10);
#undef NZCV
}

static void test_flags_lifted(void) {
    CHECK_EQ((u32)C2(t_cmpflags, 1, 2), 8);
    CHECK_EQ((u32)C2(t_cmpflags, 2, 2), 6);
    CHECK_EQ((u32)C2(t_cmpflags, 0x80000000u, 1), 3);
    CHECK_EQ((u32)C2(t_cmpflags, 0, 0x80000000u), 9);
    CHECK_EQ((u32)C2(t_cmpflags, 5, 3), 2);
    CHECK_EQ((u32)C2(t_addflags, 0xFFFFFFFFu, 1), 6);
    CHECK_EQ((u32)C2(t_addflags, 0x7FFFFFFFu, 1), 9);
    CHECK_EQ((u32)C2(t_addflags, 1, 1), 0);
    CHECK_EQ((u32)C2(t_addflags, 0x80000000u, 0x80000000u), 7);
    CHECK_EQ((u32)C1(t_lsrs, 1), 6);
    CHECK_EQ((u32)C1(t_lsrs, 2), 0);
    CHECK_EQ((u32)C1(t_lsrs, 3), 2);
    CHECK_EQ((u32)C2(t_smax, 0xFFFFFFFFu, 1), 1);
    CHECK_EQ((u32)C2(t_smax, 5, 0xFFFFFFFDu), 5);
    CHECK_EQ((u32)C2(t_smax, 0x80000000u, 0x7FFFFFFFu), 0x7FFFFFFFu);
}

static void test_callf(void) {
    CHECK_EQ(t_pack10(1, 2, 3, 4, 5, 6, 7, 8, 9, 10), PACK10);
    CHECK_EQ(h_pack10(1, 2, 3, 4, 5, 6, 7, 8, 9, 10), PACK10);
    {   /* CALLF tel que le code traduit l'ecrit : r0..r3, puis 6 mots sous sp ; resultat dans r0:r1 */
        u32 r0, r1, r2, r3, sp = 0x02200000u, i;
        for (i = 0; i < 6; i++) P32(sp + 4 * i) = 5 + i;
        r0 = 1; r1 = 2; r2 = 3; r3 = 4;
        CALLF(h_pack10);
        CHECK_EQ(r0, 0x87654321u); CHECK_EQ(r1, 0xA9u);
        r0 = 1; r1 = 2;
        CALLF(t_pack10);
        CHECK_EQ(r0, 0x87654321u); CHECK_EQ(r1, 0xA9u);
        r0 = 1; r1 = 2;
        CALLF(nds_call_target(A_h_pack10));
        CHECK_EQ(r0, 0x87654321u); CHECK_EQ(r1, 0xA9u);
    }
    CHECK_EQ(C1(t_call10, 0), PACK10);                 /* bl d'une fonction traduite */
    host_calls = 0;
    CHECK_EQ(C1(t_callptr, 0), PACK10);                /* pointeur Thumb du pool litteral, blx, fonction C */
    CHECK_EQ(host_calls, 1);
}

static void test_gosub(void) {
    CHECK_EQ((u32)C1(t_gosub, 1), 15);                 /* 4x + 11 : bx lr, mov pc lr, pop {pc} imbrique */
    CHECK_EQ((u32)C1(t_gosub, 5), 31);
    CHECK_EQ((u32)C1(t_thumb, 1), 13);                 /* Thumb : 4x + 9 */
    CHECK_EQ((u32)C1(t_thumb, 2), 17);
}

static void test_call_target(void) {
    CHECK_EQ(nds_call_target(A_t_gosub), (uptr)t_gosub);
    CHECK_EQ(nds_call_target(A_t_gosub | 1), (uptr)t_gosub);
    CHECK_EQ(A_t_thumb & 1, 1);
    CHECK_EQ(nds_call_target(A_t_thumb), (uptr)t_thumb);
    CHECK_EQ(nds_call_target(A_t_thumb & ~1u), (uptr)t_thumb);
    CHECK_EQ(nds_call_target(A_h_pack10), (uptr)h_pack10);
    CHECK_EQ(nds_call_target(0x01FF8000u), (uptr)h_itcm);    /* ITCM : debut de la table directe */
    CHECK_EQ(nds_call_target(0x027E0100u), (uptr)h_dtcm);    /* DTCM : hors de la table directe, recherche */
    EXPECT_FATAL(nds_call_target(A_t_gosub + 4));            /* milieu d'une fonction */
    EXPECT_FATAL(nds_call_target(0x023FFFFEu));              /* derniere entree de la table directe, vide */
    EXPECT_FATAL(nds_call_target(0xFFFFFFFEu));              /* MAGIC_LR */
    EXPECT_FATAL(nds_call_target(0xFFF00000u));              /* GOSUB_TAG */
    EXPECT_FATAL(nds_call_target(0x10001000u));              /* ancienne adresse x86 : plus acceptee */
    EXPECT_FATAL(nds_call_target(0));
    /* depuis le code traduit : blx rX et bx rX */
    CHECK_EQ((u32)C2(t_callreg, A_t_thumb, 1), 13);
    CHECK_EQ((u32)C2(t_callreg, A_t_gosub, 1), 15);
    CHECK_EQ((u32)C2(t_callreg, 0x01FF8001u, 41), 42);
    CHECK_EQ((u32)C2(t_tail, A_t_thumb, 2), 17);
    EXPECT_FATAL(C2(t_callreg, A_t_gosub + 4, 1));
}

static void *run_tests(void *arg) {
    nds_code_map_init();
    test_memory();
    test_flag_macros();
    test_flags_lifted();
    test_callf();
    test_gosub();
    test_call_target();
    return arg;
}

/* ------------------------------------------------------------------ memoire DS */
#if NDS_IDENTITY
__declspec(dllimport) void *__stdcall VirtualAlloc(void *addr, unsigned long size, unsigned long type, unsigned long prot);

int main(void) {
    void *ram = (void *)0x02000000u;
    if (VirtualAlloc(ram, 0x400000, 0x3000 /* MEM_COMMIT | MEM_RESERVE */, 0x04 /* PAGE_READWRITE */) != ram) {
        printf("RAM DS impossible a 0x02000000\n");
        return 2;
    }
    run_tests(0);
#else
#include <pthread.h>
#include <sys/mman.h>

enum { STACK = 1 << 20 };

int main(void) {
    /* comme os_mem_window : 4 Go + 64 Ko sans acces, puis la RAM principale et une pile a 0x20000000 */
    void *w = mmap(0, (1ull << 32) + 0x10000, PROT_NONE, MAP_PRIVATE | MAP_ANON | MAP_NORESERVE, -1, 0);
    if (w == MAP_FAILED) { printf("fenetre DS impossible\n"); return 2; }
    nds_mem = (uptr)w;
    if (mprotect(DS(0x02000000u), 0x400000, PROT_READ | PROT_WRITE) || mprotect(DS(0x20000000u), STACK, PROT_READ | PROT_WRITE)) {
        printf("memoire DS impossible\n");
        return 2;
    }
    /* le code traduit prend sp = HOST2DS(&stack_[n]) : il tourne sur une pile dans la fenetre */
    pthread_attr_t attr;
    pthread_t t;
    pthread_attr_init(&attr);
    pthread_attr_setstack(&attr, DS(0x20000000u), STACK);
    if (pthread_create(&t, &attr, run_tests, 0)) { printf("fil de test impossible\n"); return 2; }
    pthread_join(t, 0);
#endif
    printf("%s : %d verifications, %d echecs\n",
#ifdef NDS_INSTRUMENT
           "harnais (NDS_INSTRUMENT)",
#else
           "harnais",
#endif
           checks, failures);
    return failures != 0;
}
