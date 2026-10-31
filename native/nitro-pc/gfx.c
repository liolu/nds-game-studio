/* Rendu des deux moteurs 2D du DS, image entiere a chaque VBlank, a partir des registres et de la VRAM. */
#include "nitro.h"

static u16 top[256 * 192], bottom[256 * 192];
u16 *gfx_top(void) { return top; }
u16 *gfx_bottom(void) { return bottom; }

/* ------------------------------------------------------------------ banques VRAM */
#define KB(n) ((n) * 1024u)
static const u32 bank_size[9] = {KB(128), KB(128), KB(128), KB(128), KB(64), KB(16), KB(16), KB(32), KB(16)};
static const u32 lcdc_addr[9] = {0x06800000, 0x06820000, 0x06840000, 0x06860000, 0x06880000, 0x06890000, 0x06894000, 0x06898000, 0x068A0000};
static u8 bank_store[9][KB(128)];
static u32 bank_cpu[9];   /* adresse CPU actuelle de la banque, 0 si aucune */
static u8 bank_cnt[9];
static u8 *tex_slot[4];
static u8 *texpal_slot[6];
static u8 *bgext_a[4], *objext_a, *bgext_b[4], *objext_b;

static void map_special(int b, u32 mst, u32 ofs) {
    u8 *st = bank_store[b];
    switch (b) {
    case 0: case 1: if (mst == 3) tex_slot[ofs] = st; break;
    case 2: if (mst == 3) tex_slot[ofs] = st; break;
    case 3: if (mst == 3) tex_slot[ofs] = st; break;
    case 4:
        if (mst == 3) for (int i = 0; i < 4; i++) texpal_slot[i] = st + i * KB(16);
        if (mst == 4) for (int i = 0; i < 4; i++) bgext_a[i] = st + i * KB(8);
        if (mst == 5) objext_a = st;
        break;
    case 5: case 6:
        if (mst == 3) texpal_slot[(ofs & 1) + ((ofs >> 1) & 1) * 4] = st;
        if (mst == 4) { bgext_a[(ofs & 1) * 2] = st; bgext_a[(ofs & 1) * 2 + 1] = st + KB(8); }
        if (mst == 5) objext_a = st;
        break;
    case 7:
        if (mst == 2) for (int i = 0; i < 4; i++) bgext_b[i] = st + i * KB(8);
        break;
    case 8:
        if (mst == 3) objext_b = st;
        break;
    }
}

static u32 map_cpu(int b, u32 mst, u32 ofs) {
    if (mst == 0) return lcdc_addr[b];
    switch (b) {
    case 0: case 1:
        if (mst == 1) return 0x06000000 + ofs * KB(128);
        if (mst == 2) return 0x06400000 + (ofs & 1) * KB(128);
        break;
    case 2:
        if (mst == 1) return 0x06000000 + ofs * KB(128);
        if (mst == 4) return 0x06200000;
        break;
    case 3:
        if (mst == 1) return 0x06000000 + ofs * KB(128);
        if (mst == 4) return 0x06600000;
        break;
    case 4:
        if (mst == 1) return 0x06000000;
        if (mst == 2) return 0x06400000;
        break;
    case 5: case 6:
        if (mst == 1) return 0x06000000 + (ofs & 1) * KB(16) + ((ofs >> 1) & 1) * KB(64);
        if (mst == 2) return 0x06400000 + (ofs & 1) * KB(16) + ((ofs >> 1) & 1) * KB(64);
        break;
    case 7:
        if (mst == 1) return 0x06200000;
        break;
    case 8:
        if (mst == 1) return 0x06208000;
        if (mst == 2) return 0x06600000;
        break;
    }
    return 0;
}

static const u8 vram_reg[9] = {0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x48, 0x49};

/* la banque suit sa position CPU : son contenu est recopie quand elle change de place */
void vram_cnt_changed(void) {
    for (int b = 0; b < 9; b++) {
        u8 v = REG8(IO + 0x200 + vram_reg[b]);
        if (v == bank_cnt[b]) continue;
        if (bank_cpu[b]) {
            nds_copy(bank_store[b], (void *)bank_cpu[b], bank_size[b]);
            nds_zero((void *)bank_cpu[b], bank_size[b]);
            bank_cpu[b] = 0;
        }
        bank_cnt[b] = v;
    }
    /* rebatir les pointeurs speciaux puis reposer les banques en ordre */
    for (int i = 0; i < 4; i++) { tex_slot[i] = 0; bgext_a[i] = 0; bgext_b[i] = 0; }
    for (int i = 0; i < 6; i++) texpal_slot[i] = 0;
    objext_a = objext_b = 0;
    for (int b = 0; b < 9; b++) {
        u8 v = bank_cnt[b];
        if (!(v & 0x80)) continue;
        u32 mst = v & 7, ofs = (v >> 3) & 3;
        u32 addr = map_cpu(b, mst, ofs);
        if (addr && !bank_cpu[b]) {
            nds_copy((void *)addr, bank_store[b], bank_size[b]);
            bank_cpu[b] = addr;
        } else if (!addr) {
            map_special(b, mst, ofs);
        }
    }
}

u8 *vram_tex_slot(int slot) { return tex_slot[slot & 3]; }
u8 *vram_texpal_slot(int slot) { return texpal_slot[slot % 6]; }

/* ------------------------------------------------------------------ une ligne */
typedef struct {
    u32 io, pal_bg, pal_obj, vram_bg, vram_obj, ext_mask;
    u8 **bgext;
    u8 *objext;
    int main;
    u32 dispcnt;
} Eng;

#define NOPIX 0xFFFF
static u16 line_bg[4][256];
static u16 line_obj[256];
static u8 obj_prio[256], obj_semi[256], obj_win[256];
static u8 win_mask[256];

static inline u16 rd16(u32 a) { return *(volatile u16 *)a; }

static void bg_text(const Eng *e, int n, int y, u32 cnt) {
    u16 *out = line_bg[n];
    u32 hof = rd16(e->io + 0x10 + n * 4) & 0x1FF, vof = rd16(e->io + 0x12 + n * 4) & 0x1FF;
    u32 cb = ((cnt >> 2) & 15) * KB(16) + (e->main ? ((e->dispcnt >> 24) & 7) * KB(64) : 0);
    u32 sb = ((cnt >> 8) & 31) * KB(2) + (e->main ? ((e->dispcnt >> 27) & 7) * KB(64) : 0);
    u32 size = cnt >> 14;
    u32 w = (size & 1) ? 512 : 256, h = (size & 2) ? 512 : 256;
    int c256 = (cnt >> 7) & 1;
    u32 yy = (y + vof) & (h - 1);
    u8 *ext = 0;
    if (c256 && (e->dispcnt >> 30) & 1) {
        int slot = n;
        if (n < 2 && (cnt >> 13) & 1) slot = n + 2;
        ext = e->bgext[slot];
    }
    for (int x = 0; x < 256; x++) {
        u32 xx = (x + hof) & (w - 1);
        u32 map = e->vram_bg + sb;
        u32 tx = xx >> 3, ty = yy >> 3;
        if (tx >= 32) { map += KB(2); tx -= 32; }
        if (ty >= 32) { map += (w == 512) ? KB(4) : KB(2); ty -= 32; }
        u16 ent = rd16(map + (ty * 32 + tx) * 2);
        u32 tile = ent & 0x3FF;
        u32 px = xx & 7, py = yy & 7;
        if (ent & 0x400) px = 7 - px;
        if (ent & 0x800) py = 7 - py;
        u32 c;
        if (c256) {
            c = *(volatile u8 *)(e->vram_bg + cb + tile * 64 + py * 8 + px);
            if (!c) { out[x] = NOPIX; continue; }
            out[x] = ext ? *(u16 *)(ext + ((ent >> 12) * 256 + c) * 2) & 0x7FFF : rd16(e->pal_bg + c * 2) & 0x7FFF;
        } else {
            u8 b = *(volatile u8 *)(e->vram_bg + cb + tile * 32 + py * 4 + px / 2);
            c = (px & 1) ? b >> 4 : b & 15;
            if (!c) { out[x] = NOPIX; continue; }
            out[x] = rd16(e->pal_bg + ((ent >> 12) * 16 + c) * 2) & 0x7FFF;
        }
    }
}

static s32 aff_x[2][2], aff_y[2][2];

static void aff_line(const Eng *e, int n, int y, s32 *sx, s32 *sy, s32 *dx, s32 *dy) {
    u32 base = e->io + 0x20 + (n - 2) * 16;
    s32 pa = (s16)rd16(base), pb = (s16)rd16(base + 2), pc = (s16)rd16(base + 4), pd = (s16)rd16(base + 6);
    s32 x0 = (s32)(*(volatile u32 *)(base + 8) << 4) >> 4, y0 = (s32)(*(volatile u32 *)(base + 12) << 4) >> 4;
    *sx = x0 + pb * y;
    *sy = y0 + pd * y;
    *dx = pa;
    *dy = pc;
}

static void bg_affine(const Eng *e, int n, int y, u32 cnt) {
    u16 *out = line_bg[n];
    u32 cb = ((cnt >> 2) & 15) * KB(16) + (e->main ? ((e->dispcnt >> 24) & 7) * KB(64) : 0);
    u32 sb = ((cnt >> 8) & 31) * KB(2) + (e->main ? ((e->dispcnt >> 27) & 7) * KB(64) : 0);
    u32 size = 128u << (cnt >> 14);
    int wrap = (cnt >> 13) & 1;
    s32 sx, sy, dx, dy;
    aff_line(e, n, y, &sx, &sy, &dx, &dy);
    for (int x = 0; x < 256; x++, sx += dx, sy += dy) {
        s32 tx = sx >> 8, ty = sy >> 8;
        if (wrap) { tx &= size - 1; ty &= size - 1; }
        else if (tx < 0 || ty < 0 || tx >= (s32)size || ty >= (s32)size) { out[x] = NOPIX; continue; }
        u8 tile = *(volatile u8 *)(e->vram_bg + sb + (ty >> 3) * (size >> 3) + (tx >> 3));
        u8 c = *(volatile u8 *)(e->vram_bg + cb + tile * 64 + (ty & 7) * 8 + (tx & 7));
        out[x] = c ? rd16(e->pal_bg + c * 2) & 0x7FFF : NOPIX;
    }
}

static void bg_extended(const Eng *e, int n, int y, u32 cnt) {
    u16 *out = line_bg[n];
    u32 base = ((cnt >> 8) & 31);
    int wrap = (cnt >> 13) & 1;
    s32 sx, sy, dx, dy;
    aff_line(e, n, y, &sx, &sy, &dx, &dy);
    if (!(cnt & 0x80)) {
        /* carte 16 bits */
        u32 cb = ((cnt >> 2) & 15) * KB(16) + (e->main ? ((e->dispcnt >> 24) & 7) * KB(64) : 0);
        u32 sb = base * KB(2) + (e->main ? ((e->dispcnt >> 27) & 7) * KB(64) : 0);
        u32 size = 128u << (cnt >> 14);
        u8 *ext = ((e->dispcnt >> 30) & 1) ? e->bgext[n] : 0;
        for (int x = 0; x < 256; x++, sx += dx, sy += dy) {
            s32 tx = sx >> 8, ty = sy >> 8;
            if (wrap) { tx &= size - 1; ty &= size - 1; }
            else if (tx < 0 || ty < 0 || tx >= (s32)size || ty >= (s32)size) { out[x] = NOPIX; continue; }
            u16 ent = rd16(e->vram_bg + sb + ((ty >> 3) * (size >> 3) + (tx >> 3)) * 2);
            u32 px = tx & 7, py = ty & 7;
            if (ent & 0x400) px = 7 - px;
            if (ent & 0x800) py = 7 - py;
            u8 c = *(volatile u8 *)(e->vram_bg + cb + (ent & 0x3FF) * 64 + py * 8 + px);
            if (!c) { out[x] = NOPIX; continue; }
            out[x] = ext ? *(u16 *)(ext + ((ent >> 12) * 256 + c) * 2) & 0x7FFF : rd16(e->pal_bg + c * 2) & 0x7FFF;
        }
        return;
    }
    static const u16 ws[4] = {128, 256, 512, 512}, hs[4] = {128, 256, 256, 512};
    u32 w = ws[cnt >> 14], h = hs[cnt >> 14];
    u32 addr = e->vram_bg + base * KB(16);
    int direct = (cnt >> 2) & 1;
    for (int x = 0; x < 256; x++, sx += dx, sy += dy) {
        s32 tx = sx >> 8, ty = sy >> 8;
        if (wrap) { tx &= w - 1; ty &= h - 1; }
        else if (tx < 0 || ty < 0 || tx >= (s32)w || ty >= (s32)h) { out[x] = NOPIX; continue; }
        if (direct) {
            u16 c = rd16(addr + (ty * w + tx) * 2);
            out[x] = (c & 0x8000) ? c & 0x7FFF : NOPIX;
        } else {
            u8 c = *(volatile u8 *)(addr + ty * w + tx);
            out[x] = c ? rd16(e->pal_bg + c * 2) & 0x7FFF : NOPIX;
        }
    }
}

static void render_objs(const Eng *e, int y) {
    static const u8 sw[4][4] = {{8, 16, 32, 64}, {16, 32, 32, 64}, {8, 8, 16, 32}, {8, 8, 8, 8}};
    static const u8 sh[4][4] = {{8, 16, 32, 64}, {8, 8, 16, 32}, {16, 32, 32, 64}, {8, 8, 8, 8}};
    u32 oam = e->main ? 0x07000000u : 0x07000400u;
    for (int x = 0; x < 256; x++) { line_obj[x] = NOPIX; obj_prio[x] = 4; obj_semi[x] = 0; obj_win[x] = 0; }
    if (!(e->dispcnt & 0x1000)) return;
    int map1d = (e->dispcnt >> 4) & 1;
    u32 boundary = map1d ? 32u << ((e->dispcnt >> 20) & 3) : 32;
    for (int i = 127; i >= 0; i--) {
        u16 a0 = rd16(oam + i * 8), a1 = rd16(oam + i * 8 + 2), a2 = rd16(oam + i * 8 + 4);
        int affine = (a0 >> 8) & 1;
        if (!affine && (a0 & 0x200)) continue;
        u32 mode = (a0 >> 10) & 3, shape = a0 >> 14, size = a1 >> 14;
        if (shape == 3) continue;
        int w = sw[shape][size], h = sh[shape][size];
        int dbl = affine && (a0 & 0x200);
        int bw = dbl ? w * 2 : w, bh = dbl ? h * 2 : h;
        int oy = a0 & 0xFF;
        int ox = a1 & 0x1FF;
        if (ox >= 256) ox -= 512;
        int ly = (y - oy) & 0xFF;
        if (ly >= bh) continue;
        s32 pa = 256, pb = 0, pc = 0, pd = 256;
        if (affine) {
            u32 p = oam + ((a1 >> 9) & 31) * 32;
            pa = (s16)rd16(p + 6); pb = (s16)rd16(p + 14); pc = (s16)rd16(p + 22); pd = (s16)rd16(p + 30);
        }
        u32 prio = (a2 >> 10) & 3, tile = a2 & 0x3FF, pal = a2 >> 12;
        int c256 = (a0 >> 13) & 1;
        for (int bx = 0; bx < bw; bx++) {
            int sx = ox + bx;
            if (sx < 0 || sx >= 256) continue;
            int tx, ty;
            if (affine) {
                int cx = bx - bw / 2, cy = ly - bh / 2;
                tx = ((pa * cx + pb * cy) >> 8) + w / 2;
                ty = ((pc * cx + pd * cy) >> 8) + h / 2;
                if (tx < 0 || ty < 0 || tx >= w || ty >= h) continue;
            } else {
                tx = (a1 & 0x1000) ? w - 1 - bx : bx;
                ty = (a1 & 0x2000) ? h - 1 - ly : ly;
            }
            u16 color = NOPIX;
            if (mode == 3) {
                u32 addr;
                if ((e->dispcnt >> 6) & 1) {
                    u32 bnd = (e->dispcnt >> 22) & 1 ? 256 : 128;
                    addr = e->vram_obj + tile * bnd + (ty * w + tx) * 2;
                } else if ((e->dispcnt >> 5) & 1) {
                    addr = e->vram_obj + ((tile & 0x1F) * 8 + ((tile >> 5) & 0x1F) * 256 + ty * 256 + tx) * 2;
                } else {
                    addr = e->vram_obj + ((tile & 0xF) * 8 + ((tile >> 4) & 0x1F) * 128 + ty * 128 + tx) * 2;
                }
                u16 c = rd16(addr);
                if (!(c & 0x8000) || pal == 0) continue;
                color = c & 0x7FFF;
            } else {
                u32 c;
                if (c256) {
                    u32 off;
                    if (map1d) off = tile * boundary + (ty >> 3) * (w >> 3) * 64 + (tx >> 3) * 64;
                    else off = (((tile & ~1u) + (ty >> 3) * 32 + (tx >> 3) * 2) & 0x3FF) * 32;
                    c = *(volatile u8 *)(e->vram_obj + off + (ty & 7) * 8 + (tx & 7));
                    if (!c) continue;
                    if ((e->dispcnt >> 31) && e->objext) color = *(u16 *)(e->objext + (pal * 256 + c) * 2) & 0x7FFF;
                    else color = rd16(e->pal_obj + c * 2) & 0x7FFF;
                } else {
                    u32 off;
                    if (map1d) off = tile * boundary + (ty >> 3) * (w >> 3) * 32 + (tx >> 3) * 32;
                    else off = ((tile + (ty >> 3) * 32 + (tx >> 3)) & 0x3FF) * 32;
                    u8 b = *(volatile u8 *)(e->vram_obj + off + (ty & 7) * 4 + (tx & 7) / 2);
                    c = (tx & 1) ? b >> 4 : b & 15;
                    if (!c) continue;
                    color = rd16(e->pal_obj + (pal * 16 + c) * 2) & 0x7FFF;
                }
            }
            if (mode == 2) { obj_win[sx] = 1; continue; }
            if (prio <= obj_prio[sx] || line_obj[sx] == NOPIX) {
                line_obj[sx] = color;
                obj_prio[sx] = prio;
                obj_semi[sx] = mode == 1;
            }
        }
    }
}

static void compute_windows(const Eng *e, int y) {
    u32 d = e->dispcnt;
    u32 winin = rd16(e->io + 0x48), winout = rd16(e->io + 0x4A);
    int any = (d >> 13) & 7;
    for (int x = 0; x < 256; x++) win_mask[x] = any ? (u8)(winout & 0x3F) : 0x3F;
    if (!any) return;
    if ((d >> 15) & 1)
        for (int x = 0; x < 256; x++) if (obj_win[x]) win_mask[x] = (u8)((winout >> 8) & 0x3F);
    for (int w = 1; w >= 0; w--) {
        if (!((d >> (13 + w)) & 1)) continue;
        u32 hreg = rd16(e->io + 0x40 + w * 2), vreg = rd16(e->io + 0x44 + w * 2);
        int x1 = hreg >> 8, x2 = hreg & 0xFF, y1 = vreg >> 8, y2 = vreg & 0xFF;
        int in = y1 <= y2 ? (y >= y1 && y < y2) : (y >= y1 || y < y2);
        if (!in) continue;
        u32 m = (winin >> (w * 8)) & 0x3F;
        for (int x = 0; x < 256; x++) {
            int inside = x1 <= x2 ? (x >= x1 && x < x2) : (x >= x1 || x < x2);
            if (inside) win_mask[x] = (u8)m;
        }
    }
}

static inline u16 blend(u16 a, u16 b, u32 eva, u32 evb) {
    u32 r = ((a & 31) * eva + (b & 31) * evb) >> 4;
    u32 g = (((a >> 5) & 31) * eva + ((b >> 5) & 31) * evb) >> 4;
    u32 bl = (((a >> 10) & 31) * eva + ((b >> 10) & 31) * evb) >> 4;
    if (r > 31) r = 31;
    if (g > 31) g = 31;
    if (bl > 31) bl = 31;
    return (u16)(r | (g << 5) | (bl << 10));
}

static inline u16 lighten(u16 c, u32 evy) {
    u32 r = c & 31, g = (c >> 5) & 31, b = (c >> 10) & 31;
    return (u16)((r + ((31 - r) * evy >> 4)) | ((g + ((31 - g) * evy >> 4)) << 5) | ((b + ((31 - b) * evy >> 4)) << 10));
}

static inline u16 darken(u16 c, u32 evy) {
    u32 r = c & 31, g = (c >> 5) & 31, b = (c >> 10) & 31;
    return (u16)((r - (r * evy >> 4)) | ((g - (g * evy >> 4)) << 5) | ((b - (b * evy >> 4)) << 10));
}

static void render_engine(const Eng *base, u16 *screen) {
    Eng e = *base;
    e.dispcnt = *(volatile u32 *)e.io;
    u32 dm = (e.dispcnt >> 16) & 3;
    if (dm == 0 || (e.main && dm == 2)) {
        if (dm == 0) {
            for (int i = 0; i < 256 * 192; i++) screen[i] = 0x7FFF;
        } else {
            u32 bank = (e.dispcnt >> 18) & 3;
            u16 *src = bank_cpu[bank] == lcdc_addr[bank] ? (u16 *)lcdc_addr[bank] : (u16 *)bank_store[bank];
            nds_copy(screen, src, 256 * 192 * 2);
            for (int i = 0; i < 256 * 192; i++) screen[i] &= 0x7FFF;
        }
    } else {
        u32 mode = e.dispcnt & 7;
        u16 backdrop = rd16(e.pal_bg) & 0x7FFF;
        u32 bldcnt = rd16(e.io + 0x50), bldalpha = rd16(e.io + 0x52), bldy = rd16(e.io + 0x54) & 31;
        u32 eva = bldalpha & 31, evb = (bldalpha >> 8) & 31;
        if (eva > 16) eva = 16;
        if (evb > 16) evb = 16;
        if (bldy > 16) bldy = 16;
        u32 bm = (bldcnt >> 6) & 3;
        const u16 *f3d = e.main && (e.dispcnt & 8) ? gx_frame3d() : 0;
        const u8 *a3d = f3d ? gx_alpha3d() : 0;
        for (int y = 0; y < 192; y++) {
            u32 cnts[4], prio[4];
            int on[4];
            for (int n = 0; n < 4; n++) {
                cnts[n] = rd16(e.io + 8 + n * 2);
                prio[n] = cnts[n] & 3;
                on[n] = (e.dispcnt >> (8 + n)) & 1;
                if (!on[n]) continue;
                if (n == 0 && f3d) {
                    for (int x = 0; x < 256; x++) line_bg[0][x] = a3d[y * 256 + x] ? f3d[y * 256 + x] & 0x7FFF : NOPIX;
                    continue;
                }
                int kind = 0; /* texte */
                if (n == 2) kind = mode == 2 || mode == 4 ? 1 : mode == 5 ? 2 : 0;
                if (n == 3) kind = mode == 1 || mode == 2 ? 1 : mode >= 3 ? 2 : 0;
                if (mode == 6) kind = n == 2 ? 2 : 0;
                if (kind == 0) bg_text(&e, n, y, cnts[n]);
                else if (kind == 1) bg_affine(&e, n, y, cnts[n]);
                else bg_extended(&e, n, y, cnts[n]);
            }
            render_objs(&e, y);
            compute_windows(&e, y);
            u16 *out = screen + y * 256;
            for (int x = 0; x < 256; x++) {
                /* les deux couches du dessus, pour le melange */
                u16 c1 = backdrop, c2 = backdrop;
                int l1 = 5, l2 = 5;
                u32 mask = win_mask[x];
                int found = 0;
                for (u32 p = 0; p < 4 && found < 2; p++) {
                    if (line_obj[x] != NOPIX && obj_prio[x] == p && (mask & 16)) {
                        if (found == 0) { c1 = line_obj[x]; l1 = 4; } else { c2 = line_obj[x]; l2 = 4; }
                        found++;
                        if (found == 2) break;
                    }
                    for (int n = 0; n < 4 && found < 2; n++) {
                        if (!on[n] || prio[n] != p || !(mask & (1u << n)) || line_bg[n][x] == NOPIX) continue;
                        if (found == 0) { c1 = line_bg[n][x]; l1 = n; } else { c2 = line_bg[n][x]; l2 = n; }
                        found++;
                    }
                }
                u16 col = c1;
                int semi = l1 == 4 && obj_semi[x];
                int is3d = l1 == 0 && f3d;
                if (is3d && ((bldcnt >> (8 + l2)) & 1)) {
                    u32 a = a3d[y * 256 + x] >> 1; /* 0..31 -> 0..15 */
                    col = blend(c1, c2, a + 1, 16 - (a + 1));
                } else if (semi && ((bldcnt >> (8 + l2)) & 1)) {
                    col = blend(c1, c2, eva, evb);
                } else if ((mask & 32) && ((bldcnt >> l1) & 1)) {
                    if (bm == 1 && ((bldcnt >> (8 + l2)) & 1)) col = blend(c1, c2, eva, evb);
                    else if (bm == 2) col = lighten(c1, bldy);
                    else if (bm == 3) col = darken(c1, bldy);
                }
                out[x] = col;
            }
        }
    }
    u32 mb = rd16(e.io + 0x6C);
    u32 f = mb & 31;
    if (f > 16) f = 16;
    if ((mb >> 14) == 1) for (int i = 0; i < 256 * 192; i++) screen[i] = lighten(screen[i], f);
    else if ((mb >> 14) == 2) for (int i = 0; i < 256 * 192; i++) screen[i] = darken(screen[i], f);
}

void gfx_render(void) {
    static u8 *ext_none[4];
    Eng a = {0x04000000, 0x05000000, 0x05000200, 0x06000000, 0x06400000, 0, bgext_a, objext_a, 1, 0};
    Eng b = {0x04001000, 0x05000400, 0x05000600, 0x06200000, 0x06600000, 0, bgext_b, objext_b, 0, 0};
    gx_begin_frame();
    u32 pow = REG16(IO + 0x304);
    u16 *sa = (pow >> 15) ? top : bottom, *sb = (pow >> 15) ? bottom : top;
    render_engine(&a, sa);
    render_engine(&b, sb);
    (void)ext_none;
}
