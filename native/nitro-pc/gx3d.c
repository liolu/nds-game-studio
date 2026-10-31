/* Moteur 3D du DS : commandes de geometrie -> liste de polygones -> image 256x192 au moment du VBlank. */
#include "nitro.h"

static u16 frame3d[256 * 192];
static u8 alpha3d[256 * 192];
const u16 *gx_frame3d(void) { return frame3d; }
const u8 *gx_alpha3d(void) { return alpha3d; }

typedef struct { s32 m[16]; } Mtx;
typedef struct { float x, y, z, w, r, g, b, s, t; } Vtx;
typedef struct { Vtx v[10]; int n; u32 attr, tex, pal; int translucent; float ysort; } Poly;

#define MAX_POLYS 8192
static Poly polys[2][MAX_POLYS];
static int poly_count[2];
static int build = 0;
static int swap_pending, swap_flags, render_flags;

static Mtx proj, pos, vec, tex, clip;
static Mtx proj_stack[2], pos_stack[32], vec_stack[32], tex_stack[2];
static int proj_sp, pos_sp, tex_sp, mtx_mode;
static s32 cur_v[3];
static u32 cur_color = 0x7FFF;
static s32 cur_s, cur_t, raw_s, raw_t;
static u32 next_attr, cur_attr, cur_tex, cur_pal;
static u32 dif_amb, spe_emi;
static s32 light_vec[4][3];
static u32 light_col[4];
static u8 shininess[128];
static int use_shine;
static int prim, prim_count;
static Vtx prim_v[4];
static int vp_x1, vp_y1, vp_x2 = 255, vp_y2 = 191;

static void m_id(Mtx *m) {
    nds_zero(m, sizeof(*m));
    m->m[0] = m->m[5] = m->m[10] = m->m[15] = 0x1000;
}

/* out = a * b, au sens du DS (lignes) */
static void m_mul(Mtx *out, const Mtx *a, const Mtx *b) {
    Mtx r;
    for (int i = 0; i < 4; i++)
        for (int j = 0; j < 4; j++) {
            s64 s = 0;
            for (int k = 0; k < 4; k++) s += (s64)a->m[i * 4 + k] * b->m[k * 4 + j];
            r.m[i * 4 + j] = (s32)(s >> 12);
        }
    *out = r;
}

static void update_clip(void) { m_mul(&clip, &pos, &proj); }

static Mtx *cur_mtx(void) {
    return mtx_mode == 0 ? &proj : mtx_mode == 3 ? &tex : &pos;
}

static void mul_cur(const Mtx *b) {
    if (mtx_mode == 2) {
        Mtx t;
        m_mul(&t, b, &vec);
        vec = t;
    }
    Mtx *c = cur_mtx();
    Mtx t;
    m_mul(&t, b, c);
    *c = t;
    if (mtx_mode != 3) update_clip();
}

static void load_cur(const Mtx *b) {
    if (mtx_mode == 2) vec = *b;
    *cur_mtx() = *b;
    if (mtx_mode != 3) update_clip();
}

/* ------------------------------------------------------------------ polygones */
static inline u32 c5(u32 c, int shift) { return (c >> shift) & 31; }

static void emit_poly(const Vtx *in, int n);

static void vertex(void) {
    s32 x = cur_v[0], y = cur_v[1], z = cur_v[2];
    Vtx o;
    o.x = ((s64)x * clip.m[0] + (s64)y * clip.m[4] + (s64)z * clip.m[8] + (s64)4096 * clip.m[12]) / (4096.0f * 4096.0f);
    o.y = ((s64)x * clip.m[1] + (s64)y * clip.m[5] + (s64)z * clip.m[9] + (s64)4096 * clip.m[13]) / (4096.0f * 4096.0f);
    o.z = ((s64)x * clip.m[2] + (s64)y * clip.m[6] + (s64)z * clip.m[10] + (s64)4096 * clip.m[14]) / (4096.0f * 4096.0f);
    o.w = ((s64)x * clip.m[3] + (s64)y * clip.m[7] + (s64)z * clip.m[11] + (s64)4096 * clip.m[15]) / (4096.0f * 4096.0f);
    o.r = (float)c5(cur_color, 0);
    o.g = (float)c5(cur_color, 5);
    o.b = (float)c5(cur_color, 10);
    u32 mode = cur_tex >> 30;
    if (mode == 3) {
        /* coordonnees de texture calculees depuis le sommet */
        o.s = (float)((((s64)x * tex.m[0] + (s64)y * tex.m[4] + (s64)z * tex.m[8]) >> 12) + raw_s * 16) / 256.0f;
        o.t = (float)((((s64)x * tex.m[1] + (s64)y * tex.m[5] + (s64)z * tex.m[9]) >> 12) + raw_t * 16) / 256.0f;
    } else {
        o.s = cur_s / 16.0f;
        o.t = cur_t / 16.0f;
    }
    if (prim < 0) return;
    prim_v[prim_count++] = o;
    switch (prim) {
    case 0: /* triangles */
        if (prim_count == 3) { emit_poly(prim_v, 3); prim_count = 0; }
        break;
    case 1: /* quads */
        if (prim_count == 4) { emit_poly(prim_v, 4); prim_count = 0; }
        break;
    case 2: /* bande de triangles */
        if (prim_count == 3) {
            emit_poly(prim_v, 3);
            prim_v[0] = prim_v[1];
            prim_v[1] = prim_v[2];
            prim_count = 2;
            /* l'ordre alterne : on echange pour garder le meme sens */
            static int odd;
            (void)odd;
        }
        break;
    case 3: /* bande de quads */
        if (prim_count == 4) {
            Vtx q[4] = {prim_v[0], prim_v[1], prim_v[3], prim_v[2]};
            emit_poly(q, 4);
            prim_v[0] = prim_v[2];
            prim_v[1] = prim_v[3];
            prim_count = 2;
        }
        break;
    }
}

static int strip_flip;

static inline float plane(const Vtx *v, int p) {
    switch (p) {
    case 0: return v->w + v->x;
    case 1: return v->w - v->x;
    case 2: return v->w + v->y;
    case 3: return v->w - v->y;
    case 4: return v->w + v->z;
    default: return v->w - v->z;
    }
}

static void lerp(Vtx *o, const Vtx *a, const Vtx *b, float t) {
    o->x = a->x + (b->x - a->x) * t;
    o->y = a->y + (b->y - a->y) * t;
    o->z = a->z + (b->z - a->z) * t;
    o->w = a->w + (b->w - a->w) * t;
    o->r = a->r + (b->r - a->r) * t;
    o->g = a->g + (b->g - a->g) * t;
    o->b = a->b + (b->b - a->b) * t;
    o->s = a->s + (b->s - a->s) * t;
    o->t = a->t + (b->t - a->t) * t;
}

static void emit_poly(const Vtx *in, int n) {
    /* decoupage aux 6 plans */
    Vtx a[12], b[12];
    int na = n;
    for (int i = 0; i < n; i++) a[i] = in[i];
    if (prim == 2) {
        /* dans une bande, un triangle sur deux a le sens inverse */
        if (strip_flip) { Vtx t = a[1]; a[1] = a[2]; a[2] = t; }
        strip_flip = !strip_flip;
    }
    /* face cachee */
    {
        float ax = a[1].x * a[0].w - a[0].x * a[1].w, ay = a[1].y * a[0].w - a[0].y * a[1].w;
        float bx = a[2].x * a[0].w - a[0].x * a[2].w, by = a[2].y * a[0].w - a[0].y * a[2].w;
        float area = ax * by - ay * bx;
        /* signe : w peut etre negatif avant decoupage, on le compense */
        float sign = a[0].w * a[1].w * a[2].w;
        if (sign < 0) area = -area;
        int front = area > 0;
        if (front && !(cur_attr & 0x80)) return;
        if (!front && !(cur_attr & 0x40)) return;
    }
    for (int p = 0; p < 6; p++) {
        int nb = 0;
        for (int i = 0; i < na; i++) {
            const Vtx *c = &a[i], *d = &a[(i + 1) % na];
            float pc = plane(c, p), pd = plane(d, p);
            if (pc >= 0) b[nb++] = *c;
            if ((pc >= 0) != (pd >= 0) && nb < 11) lerp(&b[nb++], c, d, pc / (pc - pd));
            if (nb >= 11) break;
        }
        if (nb < 3) return;
        na = nb;
        for (int i = 0; i < na; i++) a[i] = b[i];
    }
    if (na > 10) na = 10;
    if (poly_count[build] >= MAX_POLYS) return;
    Poly *q = &polys[build][poly_count[build]++];
    q->n = na;
    for (int i = 0; i < na; i++) q->v[i] = a[i];
    q->attr = cur_attr;
    q->tex = cur_tex;
    q->pal = cur_pal;
    u32 alpha = (cur_attr >> 16) & 31;
    u32 fmt = (cur_tex >> 26) & 7;
    q->translucent = (alpha != 31 && alpha != 0) || fmt == 1 || fmt == 6;
}

static s32 sext10(u32 v) { return (s32)(v << 22) >> 22; }

static void normal(u32 p) {
    s32 n[3] = {sext10(p) << 3, sext10(p >> 10) << 3, sext10(p >> 20) << 3};
    if ((cur_tex >> 30) == 2) {
        /* coordonnees de texture depuis la normale */
        cur_s = (s32)(((s64)n[0] * tex.m[0] + (s64)n[1] * tex.m[4] + (s64)n[2] * tex.m[8]) >> 21) + raw_s;
        cur_t = (s32)(((s64)n[0] * tex.m[1] + (s64)n[1] * tex.m[5] + (s64)n[2] * tex.m[9]) >> 21) + raw_t;
    }
    s32 v[3];
    for (int i = 0; i < 3; i++) v[i] = (s32)(((s64)n[0] * vec.m[0 + i] + (s64)n[1] * vec.m[4 + i] + (s64)n[2] * vec.m[8 + i]) >> 12);
    int r = c5(spe_emi, 16), g = c5(spe_emi, 21), b = c5(spe_emi, 26);
    r <<= 0;
    for (int l = 0; l < 4; l++) {
        if (!(cur_attr & (1u << l))) continue;
        s32 d = -(s32)(((s64)light_vec[l][0] * v[0] + (s64)light_vec[l][1] * v[1] + (s64)light_vec[l][2] * v[2]) >> 12);
        s32 dif = d < 0 ? 0 : d > 0x1000 ? 0x1000 : d;
        /* reflet : direction moitie entre lumiere et vue (0,0,-1) */
        s32 hx = light_vec[l][0], hy = light_vec[l][1], hz = light_vec[l][2] - 0x1000;
        s32 sd = -(s32)(((s64)(hx / 2) * v[0] + (s64)(hy / 2) * v[1] + (s64)(hz / 2) * v[2]) >> 12);
        s32 spec = sd < 0 ? 0 : sd > 0x1000 ? 0x1000 : sd;
        spec = (s32)(((s64)spec * spec) >> 12);
        if (use_shine) spec = shininess[spec * 127 / 0x1000] << 4;
        u32 lc = light_col[l];
        r += (c5(spe_emi, 0) * c5(lc, 0) * spec >> 17) + (c5(dif_amb, 0) * c5(lc, 0) * dif >> 17) + (c5(dif_amb, 16) * c5(lc, 0) >> 5);
        g += (c5(spe_emi, 5) * c5(lc, 5) * spec >> 17) + (c5(dif_amb, 5) * c5(lc, 5) * dif >> 17) + (c5(dif_amb, 21) * c5(lc, 5) >> 5);
        b += (c5(spe_emi, 10) * c5(lc, 10) * spec >> 17) + (c5(dif_amb, 10) * c5(lc, 10) * dif >> 17) + (c5(dif_amb, 26) * c5(lc, 10) >> 5);
    }
    if (r > 31) r = 31;
    if (g > 31) g = 31;
    if (b > 31) b = 31;
    cur_color = r | (g << 5) | (b << 10);
}

/* ------------------------------------------------------------------ commandes */
static const u8 param_count[256] = {
    [0x10] = 1, [0x12] = 1, [0x13] = 1, [0x14] = 1, [0x16] = 16, [0x17] = 12, [0x18] = 16, [0x19] = 12, [0x1A] = 9,
    [0x1B] = 3, [0x1C] = 3, [0x20] = 1, [0x21] = 1, [0x22] = 1, [0x23] = 2, [0x24] = 1, [0x25] = 1, [0x26] = 1,
    [0x27] = 1, [0x28] = 1, [0x29] = 1, [0x2A] = 1, [0x2B] = 1, [0x30] = 1, [0x31] = 1, [0x32] = 1, [0x33] = 1,
    [0x34] = 32, [0x40] = 1, [0x50] = 1, [0x60] = 1, [0x70] = 3, [0x71] = 2, [0x72] = 1,
};

static void write_clip_readback(void) {
    for (int i = 0; i < 16; i++) REG32(IO + 0x640 + i * 4) = (u32)clip.m[i];
    for (int i = 0; i < 3; i++)
        for (int j = 0; j < 3; j++) REG32(IO + 0x680 + (i * 3 + j) * 4) = (u32)vec.m[i * 4 + j];
}

static void exec(u32 cmd, const u32 *p) {
    switch (cmd) {
    case 0x10: mtx_mode = p[0] & 3; break;
    case 0x11:
        if (mtx_mode == 0) proj_stack[proj_sp++ & 1] = proj;
        else if (mtx_mode == 3) tex_stack[tex_sp++ & 1] = tex;
        else { pos_stack[pos_sp & 31] = pos; vec_stack[pos_sp & 31] = vec; pos_sp++; }
        break;
    case 0x12: {
        s32 n = (s32)(p[0] << 26) >> 26;
        if (mtx_mode == 0) proj = proj_stack[--proj_sp & 1];
        else if (mtx_mode == 3) tex = tex_stack[--tex_sp & 1];
        else { pos_sp -= n; pos = pos_stack[pos_sp & 31]; vec = vec_stack[pos_sp & 31]; }
        if (mtx_mode != 3) update_clip();
        break;
    }
    case 0x13:
        if (mtx_mode == 0) proj_stack[0] = proj;
        else if (mtx_mode == 3) tex_stack[0] = tex;
        else { pos_stack[p[0] & 31] = pos; vec_stack[p[0] & 31] = vec; }
        break;
    case 0x14:
        if (mtx_mode == 0) proj = proj_stack[0];
        else if (mtx_mode == 3) tex = tex_stack[0];
        else { pos = pos_stack[p[0] & 31]; vec = vec_stack[p[0] & 31]; }
        if (mtx_mode != 3) update_clip();
        break;
    case 0x15: { Mtx m; m_id(&m); load_cur(&m); break; }
    case 0x16: { Mtx m; for (int i = 0; i < 16; i++) m.m[i] = (s32)p[i]; load_cur(&m); break; }
    case 0x17: {
        Mtx m; m_id(&m);
        for (int i = 0; i < 4; i++) for (int j = 0; j < 3; j++) m.m[i * 4 + j] = (s32)p[i * 3 + j];
        load_cur(&m);
        break;
    }
    case 0x18: { Mtx m; for (int i = 0; i < 16; i++) m.m[i] = (s32)p[i]; mul_cur(&m); break; }
    case 0x19: {
        Mtx m; m_id(&m);
        for (int i = 0; i < 4; i++) for (int j = 0; j < 3; j++) m.m[i * 4 + j] = (s32)p[i * 3 + j];
        mul_cur(&m);
        break;
    }
    case 0x1A: {
        Mtx m; m_id(&m);
        for (int i = 0; i < 3; i++) for (int j = 0; j < 3; j++) m.m[i * 4 + j] = (s32)p[i * 3 + j];
        mul_cur(&m);
        break;
    }
    case 0x1B: {
        /* l'echelle ne touche pas la matrice des vecteurs */
        Mtx m, t; m_id(&m);
        m.m[0] = (s32)p[0]; m.m[5] = (s32)p[1]; m.m[10] = (s32)p[2];
        Mtx *c = cur_mtx();
        m_mul(&t, &m, c);
        *c = t;
        if (mtx_mode != 3) update_clip();
        break;
    }
    case 0x1C: {
        Mtx m; m_id(&m);
        m.m[12] = (s32)p[0]; m.m[13] = (s32)p[1]; m.m[14] = (s32)p[2];
        mul_cur(&m);
        break;
    }
    case 0x20: cur_color = p[0] & 0x7FFF; break;
    case 0x21: normal(p[0]); break;
    case 0x22: {
        raw_s = (s16)p[0];
        raw_t = (s16)(p[0] >> 16);
        if ((cur_tex >> 30) == 1) {
            cur_s = (s32)(((s64)raw_s * tex.m[0] + (s64)raw_t * tex.m[4] + (s64)16 * tex.m[8] + (s64)16 * tex.m[12]) >> 12);
            cur_t = (s32)(((s64)raw_s * tex.m[1] + (s64)raw_t * tex.m[5] + (s64)16 * tex.m[9] + (s64)16 * tex.m[13]) >> 12);
        } else {
            cur_s = raw_s;
            cur_t = raw_t;
        }
        break;
    }
    case 0x23: cur_v[0] = (s16)p[0]; cur_v[1] = (s16)(p[0] >> 16); cur_v[2] = (s16)p[1]; vertex(); break;
    case 0x24: cur_v[0] = sext10(p[0]) << 6; cur_v[1] = sext10(p[0] >> 10) << 6; cur_v[2] = sext10(p[0] >> 20) << 6; vertex(); break;
    case 0x25: cur_v[0] = (s16)p[0]; cur_v[1] = (s16)(p[0] >> 16); vertex(); break;
    case 0x26: cur_v[0] = (s16)p[0]; cur_v[2] = (s16)(p[0] >> 16); vertex(); break;
    case 0x27: cur_v[1] = (s16)p[0]; cur_v[2] = (s16)(p[0] >> 16); vertex(); break;
    case 0x28: cur_v[0] += sext10(p[0]); cur_v[1] += sext10(p[0] >> 10); cur_v[2] += sext10(p[0] >> 20); vertex(); break;
    case 0x29: next_attr = p[0]; break;
    case 0x2A: cur_tex = p[0]; break;
    case 0x2B: cur_pal = p[0] & 0x1FFF; break;
    case 0x30:
        dif_amb = p[0];
        if (p[0] & 0x8000) cur_color = p[0] & 0x7FFF;
        break;
    case 0x31:
        spe_emi = p[0];
        use_shine = (p[0] >> 15) & 1;
        break;
    case 0x32: {
        int l = p[0] >> 30;
        s32 v[3] = {sext10(p[0]) << 3, sext10(p[0] >> 10) << 3, sext10(p[0] >> 20) << 3};
        for (int i = 0; i < 3; i++) light_vec[l][i] = (s32)(((s64)v[0] * vec.m[0 + i] + (s64)v[1] * vec.m[4 + i] + (s64)v[2] * vec.m[8 + i]) >> 12);
        break;
    }
    case 0x33: light_col[p[0] >> 30] = p[0] & 0x7FFF; break;
    case 0x34: for (int i = 0; i < 32; i++) nds_copy(&shininess[i * 4], &p[i], 4); break;
    case 0x40:
        cur_attr = next_attr;
        prim = p[0] & 3;
        prim_count = 0;
        strip_flip = 0;
        break;
    case 0x41: prim = -1; break;
    case 0x50: swap_pending = 1; swap_flags = p[0]; break;
    case 0x60:
        vp_x1 = p[0] & 255; vp_y1 = (p[0] >> 8) & 255;
        vp_x2 = (p[0] >> 16) & 255; vp_y2 = p[0] >> 24;
        break;
    case 0x71: {
        /* POS_TEST */
        s32 x = (s16)p[0], y = (s16)(p[0] >> 16), z = (s16)p[1];
        cur_v[0] = x; cur_v[1] = y; cur_v[2] = z;
        for (int i = 0; i < 4; i++)
            REG32(IO + 0x620 + i * 4) = (u32)(s32)(((s64)x * clip.m[i] + (s64)y * clip.m[4 + i] + (s64)z * clip.m[8 + i] + (s64)4096 * clip.m[12 + i]) >> 12);
        break;
    }
    case 0x72: {
        s32 n[3] = {sext10(p[0]) << 3, sext10(p[0] >> 10) << 3, sext10(p[0] >> 20) << 3};
        for (int i = 0; i < 3; i++)
            REG16(IO + 0x630 + i * 2) = (u16)(((s64)n[0] * vec.m[0 + i] + (s64)n[1] * vec.m[4 + i] + (s64)n[2] * vec.m[8 + i]) >> 12);
        break;
    }
    }
    if (cmd >= 0x10 && cmd <= 0x1C) write_clip_readback();
}

/* commande isolee ecrite sur son port */
static u32 port_buf[32];
static int port_n;

void gx_write(u32 reg, u32 value) {
    u32 cmd = (reg - (IO + 0x400)) / 4;
    if (cmd >= 0x80) return;
    u32 need = param_count[cmd];
    if (!need) { exec(cmd, 0); return; }
    port_buf[port_n++] = value;
    if ((u32)port_n >= need) { exec(cmd, port_buf); port_n = 0; }
}

/* FIFO empaquetee : un mot de 1 a 4 commandes, puis leurs parametres */
static u32 pack_cmds;
static int pack_left;
static u32 pack_params[32];
static int pack_have;

static void pack_next(void) {
    while (pack_left > 0) {
        u32 c = pack_cmds & 0xFF;
        u32 need = param_count[c];
        if ((u32)pack_have < need) return;
        exec(c, pack_params);
        pack_have = 0;
        pack_cmds >>= 8;
        pack_left--;
    }
}

void gx_fifo_word(u32 value) {
    if (pack_left == 0) {
        pack_cmds = value;
        pack_left = 4;
        /* les zeros de fin ne sont pas des commandes */
        while (pack_left > 1 && ((pack_cmds >> ((pack_left - 1) * 8)) & 0xFF) == 0) pack_left--;
        pack_have = 0;
        pack_next();
        return;
    }
    pack_params[pack_have++] = value;
    pack_next();
}

u32 gx_status(void) {
    return 0x06000002u | (pos_sp & 31) << 8 | (proj_sp & 1) << 13;
}

/* ------------------------------------------------------------------ textures */
static u8 *tex_ptr(u32 byte) {
    u8 *s = vram_tex_slot((byte >> 17) & 3);
    return s ? s + (byte & 0x1FFFF) : 0;
}
static u8 *pal_ptr(u32 byte) {
    u8 *s = vram_texpal_slot((byte >> 14) % 6);
    return s ? s + (byte & 0x3FFF) : 0;
}

/* renvoie 0 si transparent ; sinon alpha sur 5 bits en haut, couleur 555 */
static inline u32 sample(u32 texparam, u32 palbase, s32 s, s32 t) {
    u32 fmt = (texparam >> 26) & 7;
    u32 w = 8u << ((texparam >> 20) & 7), h = 8u << ((texparam >> 23) & 7);
    if (texparam & (1u << 16)) {
        if ((texparam & (1u << 18)) && (s & w)) s = ~s;
        s &= w - 1;
    } else s = s < 0 ? 0 : s >= (s32)w ? (s32)w - 1 : s;
    if (texparam & (1u << 17)) {
        if ((texparam & (1u << 19)) && (t & h)) t = ~t;
        t &= h - 1;
    } else t = t < 0 ? 0 : t >= (s32)h ? (s32)h - 1 : t;
    u32 base = (texparam & 0xFFFF) << 3;
    u32 c0trans = (texparam >> 29) & 1;
    u32 pb = palbase << 4;
    switch (fmt) {
    case 1: {
        u8 *p = tex_ptr(base + t * w + s);
        if (!p) return 0;
        u32 idx = *p & 31, a = *p >> 5;
        u8 *q = pal_ptr(pb + idx * 2);
        if (!q || !a) return 0;
        return (*(u16 *)q & 0x7FFF) | ((a * 4 + a / 2) << 16);
    }
    case 2: {
        u8 *p = tex_ptr(base + (t * w + s) / 4);
        if (!p) return 0;
        u32 idx = (*p >> (((t * w + s) & 3) * 2)) & 3;
        if (c0trans && !idx) return 0;
        u8 *q = pal_ptr((palbase << 3) + idx * 2);
        return q ? (*(u16 *)q & 0x7FFF) | (31u << 16) : 0;
    }
    case 3: {
        u8 *p = tex_ptr(base + (t * w + s) / 2);
        if (!p) return 0;
        u32 idx = (*p >> (((t * w + s) & 1) * 4)) & 15;
        if (c0trans && !idx) return 0;
        u8 *q = pal_ptr(pb + idx * 2);
        return q ? (*(u16 *)q & 0x7FFF) | (31u << 16) : 0;
    }
    case 4: {
        u8 *p = tex_ptr(base + t * w + s);
        if (!p) return 0;
        if (c0trans && !*p) return 0;
        u8 *q = pal_ptr(pb + *p * 2);
        return q ? (*(u16 *)q & 0x7FFF) | (31u << 16) : 0;
    }
    case 5: {
        u32 bx = s >> 2, by = t >> 2;
        u32 off = base + (by * (w >> 2) + bx) * 4;
        u8 *p = tex_ptr(off);
        if (!p) return 0;
        u32 code = (p[t & 3] >> ((s & 3) * 2)) & 3;
        u32 slotoff = off & 0x1FFFF;
        u32 idxbase = 0x20000 + ((off >> 17) == 2 ? 0x10000 : 0) + slotoff / 2;
        u8 *ip = tex_ptr(idxbase & ~1u);
        if (!ip) return 0;
        u16 info = *(u16 *)ip;
        u32 pa = pb + (info & 0x3FFF) * 4;
        u32 mode = info >> 14;
#define PAL(i) (pal_ptr(pa + (i) * 2) ? *(u16 *)pal_ptr(pa + (i) * 2) & 0x7FFF : 0)
        switch (code) {
        case 0: return PAL(0) | (31u << 16);
        case 1: return PAL(1) | (31u << 16);
        case 2:
            if (mode == 0 || mode == 2) return PAL(2) | (31u << 16);
            if (mode == 1 || mode == 3) {
                u32 a = PAL(0), b = PAL(1), r;
                if (mode == 1) r = ((a & 31) + (b & 31)) / 2 | ((((a >> 5) & 31) + ((b >> 5) & 31)) / 2) << 5 | ((((a >> 10) & 31) + ((b >> 10) & 31)) / 2) << 10;
                else r = ((a & 31) * 5 + (b & 31) * 3) / 8 | ((((a >> 5) & 31) * 5 + ((b >> 5) & 31) * 3) / 8) << 5 | ((((a >> 10) & 31) * 5 + ((b >> 10) & 31) * 3) / 8) << 10;
                return r | (31u << 16);
            }
            break;
        default:
            if (mode == 0 || mode == 1) return 0;
            if (mode == 2) return PAL(3) | (31u << 16);
            {
                u32 a = PAL(0), b = PAL(1);
                u32 r = ((a & 31) * 3 + (b & 31) * 5) / 8 | ((((a >> 5) & 31) * 3 + ((b >> 5) & 31) * 5) / 8) << 5 | ((((a >> 10) & 31) * 3 + ((b >> 10) & 31) * 5) / 8) << 10;
                return r | (31u << 16);
            }
        }
        return 0;
    }
    case 6: {
        u8 *p = tex_ptr(base + t * w + s);
        if (!p) return 0;
        u32 idx = *p & 7, a = *p >> 3;
        u8 *q = pal_ptr(pb + idx * 2);
        if (!q || !a) return 0;
        return (*(u16 *)q & 0x7FFF) | (a << 16);
    }
    case 7: {
        u8 *p = tex_ptr(base + (t * w + s) * 2);
        if (!p) return 0;
        u16 c = *(u16 *)p;
        if (!(c & 0x8000)) return 0;
        return (c & 0x7FFF) | (31u << 16);
    }
    }
    return 0;
}

/* ------------------------------------------------------------------ rasterisation */
static float zbuf[256 * 192];
static u8 idbuf[256 * 192];

typedef struct { float sx, sy, iw, zw, rw, gw, bw, sw, tw; } SV;

u32 gx_stat[8]; /* essais : pixels ecrits, rejetes (z, texture, alpha), polygones par format */

static void draw_poly(const Poly *p, int disp3d, u32 alpha_ref) {
    SV sv[10];
    float vw = (float)(vp_x2 - vp_x1 + 1), vh = (float)(vp_y2 - vp_y1 + 1);
    for (int i = 0; i < p->n; i++) {
        const Vtx *v = &p->v[i];
        float iw = v->w > 1e-6f || v->w < -1e-6f ? 1.0f / v->w : 1e6f;
        sv[i].sx = vp_x1 + (v->x * iw + 1.0f) * 0.5f * vw;
        sv[i].sy = (191 - vp_y1) - (v->y * iw + 1.0f) * 0.5f * vh + 1.0f;
        sv[i].iw = iw;
        sv[i].zw = v->z * iw;
        sv[i].rw = v->r * iw;
        sv[i].gw = v->g * iw;
        sv[i].bw = v->b * iw;
        sv[i].sw = v->s * iw;
        sv[i].tw = v->t * iw;
    }
    float ymin = 1e9f, ymax = -1e9f;
    for (int i = 0; i < p->n; i++) {
        if (sv[i].sy < ymin) ymin = sv[i].sy;
        if (sv[i].sy > ymax) ymax = sv[i].sy;
    }
    int y0 = (int)ymin, y1 = (int)ymax;
    if (y0 < 0) y0 = 0;
    if (y1 > 191) y1 = 191;
    u32 attr = p->attr, texp = p->tex;
    u32 fmt = (texp >> 26) & 7;
    u32 mode = (attr >> 4) & 3;
    u32 palpha = (attr >> 16) & 31;
    int wire = palpha == 0;
    if (wire) palpha = 31;
    u32 pid = (attr >> 24) & 63;
    int blend_on = (disp3d >> 3) & 1;
    int tex_on = (disp3d & 1) && fmt != 0;
    int alpha_test = (disp3d >> 2) & 1;
    int update_depth = !p->translucent || ((attr >> 11) & 1);
    int depth_equal = (attr >> 14) & 1;
    for (int y = y0; y <= y1; y++) {
        float fy = y + 0.5f;
        float xl = 1e9f, xr = -1e9f;
        SV l, r;
        for (int i = 0; i < p->n; i++) {
            const SV *a = &sv[i], *b = &sv[(i + 1) % p->n];
            if ((a->sy <= fy) == (b->sy <= fy)) continue;
            float t = (fy - a->sy) / (b->sy - a->sy);
            float x = a->sx + (b->sx - a->sx) * t;
#define MIXV(o) o.sx = x; o.iw = a->iw + (b->iw - a->iw) * t; o.zw = a->zw + (b->zw - a->zw) * t; o.rw = a->rw + (b->rw - a->rw) * t; o.gw = a->gw + (b->gw - a->gw) * t; o.bw = a->bw + (b->bw - a->bw) * t; o.sw = a->sw + (b->sw - a->sw) * t; o.tw = a->tw + (b->tw - a->tw) * t;
            if (x < xl) { xl = x; MIXV(l) }
            if (x > xr) { xr = x; MIXV(r) }
        }
        if (xl > xr) continue;
        int x0 = (int)(xl + 0.5f), x1 = (int)(xr + 0.5f) - 1;
        if (x0 < 0) x0 = 0;
        if (x1 > 255) x1 = 255;
        float span = xr - xl;
        float di = span > 0 ? 1.0f / span : 0;
        for (int x = x0; x <= x1; x++) {
            float t = (x + 0.5f - xl) * di;
            if (t < 0) t = 0;
            if (t > 1) t = 1;
            float iw = l.iw + (r.iw - l.iw) * t;
            float w = iw != 0 ? 1.0f / iw : 0;
            /* z/w varie lineairement a l'ecran ; mode W de l'echange : la profondeur est w */
            float z = (swap_flags & 2) ? w : l.zw + (r.zw - l.zw) * t;
            int at = y * 256 + x;
            if (depth_equal ? (z > zbuf[at] + 0.002f || z < zbuf[at] - 0.002f) : z >= zbuf[at]) { gx_stat[1]++; continue; }
            if (wire && x != x0 && x != x1 && y != y0 && y != y1) continue;
            u32 cr = (u32)((l.rw + (r.rw - l.rw) * t) * w), cg = (u32)((l.gw + (r.gw - l.gw) * t) * w), cb = (u32)((l.bw + (r.bw - l.bw) * t) * w);
            if (cr > 31) cr = 31;
            if (cg > 31) cg = 31;
            if (cb > 31) cb = 31;
            u32 a = palpha;
            if (tex_on) {
                s32 s = (s32)((l.sw + (r.sw - l.sw) * t) * w), tt = (s32)((l.tw + (r.tw - l.tw) * t) * w);
                u32 tc = sample(texp, p->pal, s, tt);
                u32 ta = tc >> 16;
                if (!ta) { gx_stat[2]++; continue; }
                u32 tr = tc & 31, tg = (tc >> 5) & 31, tb = (tc >> 10) & 31;
                if (mode == 1) { /* decalque */
                    cr = (tr * ta + cr * (31 - ta)) / 31;
                    cg = (tg * ta + cg * (31 - ta)) / 31;
                    cb = (tb * ta + cb * (31 - ta)) / 31;
                } else {
                    cr = (tr + 1) * (cr + 1) / 32;
                    cg = (tg + 1) * (cg + 1) / 32;
                    cb = (tb + 1) * (cb + 1) / 32;
                    if (cr) cr--;
                    if (cg) cg--;
                    if (cb) cb--;
                    a = (ta + 1) * (palpha + 1) / 32;
                    if (a) a--;
                }
            }
            if (mode == 3) continue; /* ombres : non gerees */
            if (alpha_test && a <= alpha_ref) continue;
            if (!a) continue;
            if (blend_on && a < 31 && alpha3d[at]) {
                u16 d = frame3d[at];
                u32 inv = 31 - a;
                cr = (cr * (a + 1) + (d & 31) * inv) >> 5;
                cg = (cg * (a + 1) + ((d >> 5) & 31) * inv) >> 5;
                cb = (cb * (a + 1) + ((d >> 10) & 31) * inv) >> 5;
                if (a < alpha3d[at]) a = alpha3d[at];
            }
            gx_stat[0]++;
            frame3d[at] = (u16)(cr | (cg << 5) | (cb << 10));
            alpha3d[at] = (u8)a;
            if (update_depth) zbuf[at] = z;
            idbuf[at] = (u8)pid;
        }
    }
}

static void render_polys(int list) {
    u32 disp3d = REG16(IO + 0x60);
    u32 clear = REG32(IO + 0x350);
    u16 color = (u16)(clear & 0x7FFF);
    u8 alpha = (u8)((clear >> 16) & 31);
    u32 cdepth = REG16(IO + 0x354) & 0x7FFF;
    float zclear = cdepth / 16383.5f - 1.0f;
    if (cdepth == 0x7FFF) zclear = 1.0001f;
    if (swap_flags & 2) zclear = 1e30f;
    for (int i = 0; i < 256 * 192; i++) {
        frame3d[i] = color;
        alpha3d[i] = alpha;
        zbuf[i] = zclear;
    }
    u32 aref = REG8(IO + 0x340) & 31;
    Poly *ps = polys[list];
    int n = poly_count[list];
    for (int i = 0; i < n; i++) if (!ps[i].translucent) draw_poly(&ps[i], disp3d, aref);
    for (int i = 0; i < n; i++) if (ps[i].translucent) draw_poly(&ps[i], disp3d, aref);
    REG16(IO + 0x604) = (u16)n;
    gx_stat[3] = n;
    if (n) {
        const Poly *q = &ps[n / 2];
        gx_stat[4] = q->tex;
        gx_stat[5] = q->attr;
    }
}

/* appele a chaque VBlank : si le jeu a demande l'echange, on dessine ce qu'il a envoye */
void gx_begin_frame(void) {
    if (!swap_pending) return;
    swap_pending = 0;
    render_polys(build);
    build = !build;
    poly_count[build] = 0;
}
