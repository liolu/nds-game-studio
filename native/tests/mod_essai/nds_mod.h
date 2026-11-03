/* Interface des mods natifs, version d'essai (etape 0 de ROADMAP-MODS.md). Le mod ne connait que ce fichier. */
#ifndef NDS_MOD_H
#define NDS_MOD_H

typedef unsigned int u32;
typedef unsigned long long u64;
typedef unsigned char u8;
typedef __UINTPTR_TYPE__ uptr;

#define NDS_API_MAJOR 0
#define NDS_API_MINOR 1

/* Toute fonction traduite du jeu : r0-r3, puis 10 mots de pile ; retour r0 (bas) et r1 (haut). */
typedef u64 (*nds_fn)(u32 r0, u32 r1, u32 r2, u32 r3, u32 a0, u32 a1, u32 a2, u32 a3, u32 a4, u32 a5, u32 a6, u32 a7, u32 a8, u32 a9);

typedef struct nds_api {
    u32 size;                 /* sizeof(nds_api) du runtime : un champ au-dela n'existe pas */
    u32 api_major, api_minor;
    const char *game_code;    /* "AL7E" */
    void (*log)(const char *fmt, ...);                         /* %x %u %d %s, une ligne */
    int (*hook)(u32 ds_addr, void *replacement, void **original); /* 0 si accroche ; *original = suite de la chaine */
    u32 (*read32)(u32 ds_addr);
    void (*write32)(u32 ds_addr, u32 value);
} nds_api;

#ifdef _WIN32
#define NDS_MOD_EXPORT __declspec(dllexport)
#else
#define NDS_MOD_EXPORT __attribute__((visibility("default")))
#endif

/* Appelee une fois, avant la premiere instruction du jeu. 0 = charge, autre = refuse. */
NDS_MOD_EXPORT int nds_mod_load(const nds_api *api);

#endif
