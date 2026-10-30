//! Actimagine VX (`.vx`, VXDS) : le conteneur, l'image et le son.
//! Portage de libavcodec/vx.c, vx_audio.c et libavformat/vxdec.c de mobipeg (quatric, LGPL 2.1+).

use std::sync::OnceLock;

use crate::bits::{Msb, R, Vlc, clip8};
use crate::tables::*;
use crate::{Info, Stream, le16, le32};

/// Taille des données du son, par canal.
pub const AUDIO_EXTRA: usize = 3 * 64 * 8 * 2 + 8 * 2 + 8 * 4 + 4;
const AFRAME: usize = 128;

struct Tables {
    token: Vec<Vlc>,
    zeros: Vec<Vlc>,
    run: Vec<Vlc>,
    run7: Vlc,
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let of = |lens: &'static [u8], bits: &'static [u8]| Vlc::from_codes(lens.iter().zip(bits).enumerate().map(|(i, (&l, &b))| (l, b as u32, i as u16)));
        Tables {
            token: (0..4).map(|i| of(&VX_COEFF_TOKEN_LEN[i], &VX_COEFF_TOKEN_BITS[i])).collect(),
            zeros: (0..15).map(|i| of(&VX_TOTAL_ZEROS_LEN[i], &VX_TOTAL_ZEROS_BITS[i])).collect(),
            run: (0..6).map(|i| of(&VX_RUN_LEN[i], &VX_RUN_BITS[i])).collect(),
            run7: of(&VX_RUN7_LEN, &VX_RUN7_BITS),
        }
    })
}

#[derive(Clone, Copy)]
struct Block {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

impl Block {
    fn left(self) -> Block {
        Block { w: self.w / 2, ..self }
    }
    fn right(self) -> Block {
        Block { x: self.x + self.w / 2, w: self.w / 2, ..self }
    }
    fn up(self) -> Block {
        Block { h: self.h / 2, ..self }
    }
    fn down(self) -> Block {
        Block { y: self.y + self.h / 2, h: self.h / 2, ..self }
    }
}

#[derive(Clone, Copy, Default)]
struct Mv {
    x: i32,
    y: i32,
}

type Pic = [Vec<u8>; 3];

/// Une image à la fois ; les trois précédentes servent de références.
struct Frame<'a> {
    gb: &'a mut Msb,
    w: i32,
    h: i32,
    qtab: [u16; 3],
    dst: &'a mut Pic,
    refs: [Option<&'a Pic>; 3],
    coeff_y: Vec<u8>,
    coeff_y_stride: i32,
    coeff_uv: Vec<u8>,
    coeff_uv_stride: i32,
    vectors: Vec<Mv>,
    vectors_stride: i32,
}

fn median(a: i32, b: i32, c: i32) -> i32 {
    a.min(b).max(a.max(b).min(c))
}

#[inline]
fn px_get(w: i32, h: i32, pic: &Pic, plane: usize, x: i32, y: i32) -> i32 {
    let step = if plane > 0 { 2 } else { 1 };
    let (cw, ch) = (w / step, h / step);
    let cx = if x < 0 { cw - 1 } else { x / step };
    let cy = if y < 0 { ch - 1 } else { y / step };
    *pic[plane].get((cy * cw + cx) as usize).unwrap_or(&0) as i32
}

impl Frame<'_> {
    #[inline]
    fn get(&self, plane: usize, x: i32, y: i32) -> i32 {
        px_get(self.w, self.h, self.dst, plane, x, y)
    }

    #[inline]
    fn set(&mut self, plane: usize, x: i32, y: i32, value: i32) {
        let step = if plane > 0 { 2 } else { 1 };
        let at = (y / step) * (self.w / step) + x / step;
        if let Some(p) = self.dst[plane].get_mut(at as usize) {
            *p = value as u8;
        }
    }

    fn coeff_y_get(&self, x: i32, y: i32) -> i32 {
        let cx = if x < 0 { -1 } else { x / 4 };
        let cy = if y < 0 { -1 } else { y / 4 };
        *self.coeff_y.get(((cy + 1) * self.coeff_y_stride + cx + 1) as usize).unwrap_or(&0) as i32
    }

    fn coeff_y_set(&mut self, x: i32, y: i32, value: i32) {
        let at = (y / 4 + 1) * self.coeff_y_stride + x / 4 + 1;
        if let Some(c) = self.coeff_y.get_mut(at as usize) {
            *c = value as u8;
        }
    }

    fn coeff_uv_get(&self, x: i32, y: i32) -> i32 {
        let cx = if x < 0 { -1 } else { x / 8 };
        let cy = if y < 0 { -1 } else { y / 8 };
        *self.coeff_uv.get(((cy + 1) * self.coeff_uv_stride + cx + 1) as usize).unwrap_or(&0) as i32
    }

    fn coeff_uv_set(&mut self, x: i32, y: i32, value: i32) {
        let at = (y / 8 + 1) * self.coeff_uv_stride + x / 8 + 1;
        if let Some(c) = self.coeff_uv.get_mut(at as usize) {
            *c = value as u8;
        }
    }

    fn fill4(&mut self, x: i32, y: i32, f: impl Fn(i32, i32) -> i32) {
        for j in 0..4 {
            for i in 0..4 {
                self.set(0, x + i, y + j, f(i, j));
            }
        }
    }

    fn top<const N: usize>(&self, x: i32, y: i32) -> [i32; N] {
        std::array::from_fn(|i| self.get(0, x + i as i32, y - 1))
    }

    fn left_col<const N: usize>(&self, x: i32, y: i32) -> [i32; N] {
        std::array::from_fn(|j| self.get(0, x - 1, y + j as i32))
    }

    /// Prédiction intra des blocs 4x4 (les neuf modes de H.264).
    fn pred4x4(&mut self, mode: i32, x: i32, y: i32) -> R {
        let f3 = |a: i32, b: i32, c: i32| (a + 2 * b + c + 2) >> 2;
        let f2 = |a: i32, b: i32| (a + b + 1) >> 1;
        match mode {
            0 => {
                let t: [i32; 4] = self.top(x, y);
                self.fill4(x, y, |i, _| t[i as usize]);
            }
            1 => {
                let l: [i32; 4] = self.left_col(x, y);
                self.fill4(x, y, |_, j| l[j as usize]);
            }
            2 => {
                let dc = if x != 0 && y != 0 {
                    let t: [i32; 4] = self.top(x, y);
                    let l: [i32; 4] = self.left_col(x, y);
                    (4 + t.iter().sum::<i32>() + l.iter().sum::<i32>()) / 8
                } else if x != 0 {
                    let l: [i32; 4] = self.left_col(x, y);
                    (2 + l.iter().sum::<i32>()) / 4
                } else if y != 0 {
                    let t: [i32; 4] = self.top(x, y);
                    (2 + t.iter().sum::<i32>()) / 4
                } else {
                    128
                };
                self.fill4(x, y, |_, _| dc);
            }
            3 => {
                let t: [i32; 8] = self.top(x, y);
                let p = [f3(t[0], t[1], t[2]), f3(t[1], t[2], t[3]), f3(t[2], t[3], t[4]), f3(t[3], t[4], t[5]), f3(t[4], t[5], t[6]), f3(t[5], t[6], t[7]), (t[6] + 3 * t[7] + 2) >> 2];
                self.fill4(x, y, |i, j| p[(i + j) as usize]);
            }
            4 => {
                let lt = self.get(0, x - 1, y - 1);
                let t: [i32; 4] = self.top(x, y);
                let l: [i32; 4] = self.left_col(x, y);
                let p = [f3(l[3], l[2], l[1]), f3(l[2], l[1], l[0]), f3(l[1], l[0], lt), f3(l[0], lt, t[0]), f3(lt, t[0], t[1]), f3(t[0], t[1], t[2]), f3(t[1], t[2], t[3])];
                self.fill4(x, y, |i, j| p[(3 + i - j) as usize]);
            }
            5 => {
                let lt = self.get(0, x - 1, y - 1);
                let t: [i32; 4] = self.top(x, y);
                let l: [i32; 3] = self.left_col(x, y);
                let p = [f3(l[0], l[1], l[2]), f3(lt, l[0], l[1]), f3(l[0], lt, t[0]), f2(lt, t[0]), f3(lt, t[0], t[1]), f2(t[0], t[1]), f3(t[0], t[1], t[2]), f2(t[1], t[2]), f3(t[1], t[2], t[3]), f2(t[2], t[3])];
                self.fill4(x, y, |i, j| p[(3 + 2 * i - j) as usize]);
            }
            6 => {
                let lt = self.get(0, x - 1, y - 1);
                let t: [i32; 3] = self.top(x, y);
                let l: [i32; 4] = self.left_col(x, y);
                let p = [f3(t[0], t[1], t[2]), f3(lt, t[0], t[1]), f3(l[0], lt, t[0]), f2(lt, l[0]), f3(lt, l[0], l[1]), f2(l[0], l[1]), f3(l[0], l[1], l[2]), f2(l[1], l[2]), f3(l[1], l[2], l[3]), f2(l[2], l[3])];
                self.fill4(x, y, |i, j| p[(3 - i + 2 * j) as usize]);
            }
            7 => {
                let t: [i32; 7] = self.top(x, y);
                let p = [f2(t[0], t[1]), f3(t[0], t[1], t[2]), f2(t[1], t[2]), f3(t[1], t[2], t[3]), f2(t[2], t[3]), f3(t[2], t[3], t[4]), f2(t[3], t[4]), f3(t[3], t[4], t[5]), f2(t[4], t[5]), f3(t[4], t[5], t[6])];
                self.fill4(x, y, |i, j| p[(2 * i + j) as usize]);
            }
            8 => {
                let l: [i32; 4] = self.left_col(x, y);
                let p = [f2(l[0], l[1]), f3(l[0], l[1], l[2]), f2(l[1], l[2]), f3(l[1], l[2], l[3]), f2(l[2], l[3]), f3(l[2], l[3], l[3]), l[3]];
                self.fill4(x, y, |i, j| p[(i + 2 * j).min(6) as usize]);
            }
            _ => return Err("mode intra 4x4"),
        }
        Ok(())
    }

    fn predict4(&mut self, b: Block) -> R {
        let mut cache = [[9i32; 5]; 5];
        for y2 in 0..(b.h / 4) as usize {
            for x2 in 0..(b.w / 4) as usize {
                let mut mode = cache[y2][1 + x2].min(cache[1 + y2][x2]);
                if mode == 9 {
                    mode = 2;
                }
                if !self.gb.bit() {
                    let value = self.gb.get(3) as i32;
                    mode = value + i32::from(value >= mode);
                }
                cache[1 + y2][1 + x2] = mode;
                self.pred4x4(mode, b.x + x2 as i32 * 4, b.y + y2 as i32 * 4)?;
            }
        }
        self.predict_notile_uv(b)
    }

    fn predict_dc(&mut self, b: Block, plane: usize) {
        let mut dc = 128;
        let sum_top = |f: &Self| (0..b.w).map(|i| f.get(plane, b.x + i, b.y - 1)).sum::<i32>() + b.w / 2;
        let sum_left = |f: &Self| (0..b.h).map(|i| f.get(plane, b.x - 1, b.y + i)).sum::<i32>() + b.h / 2;
        if b.x != 0 && b.y != 0 {
            dc = (sum_top(self) / b.w + sum_left(self) / b.h + 1) / 2;
        } else if b.x == 0 && b.y != 0 {
            dc = sum_top(self) / b.w;
        } else if b.x != 0 && b.y == 0 {
            dc = sum_left(self) / b.h;
        }
        let step = if plane > 0 { 2 } else { 1 };
        for j in (0..b.h).step_by(step) {
            for i in (0..b.w).step_by(step) {
                self.set(plane, b.x + i, b.y + j, dc);
            }
        }
    }

    fn predict_horizontal(&mut self, b: Block, plane: usize) {
        let step = if plane > 0 { 2 } else { 1 };
        for j in (0..b.h).step_by(step) {
            let px = self.get(plane, b.x - 1, b.y + j);
            for i in (0..b.w).step_by(step) {
                self.set(plane, b.x + i, b.y + j, px);
            }
        }
    }

    fn predict_vertical(&mut self, b: Block, plane: usize) {
        let step = if plane > 0 { 2 } else { 1 };
        for i in (0..b.w).step_by(step) {
            let px = self.get(plane, b.x + i, b.y - 1);
            for j in (0..b.h).step_by(step) {
                self.set(plane, b.x + i, b.y + j, px);
            }
        }
    }

    fn predict_plane_intern(&mut self, b: Block, plane: usize) {
        let step = if plane > 0 { 2 } else { 1 };
        if b.w == step && b.h == step {
            return;
        }
        if b.w == step && b.h > step {
            let top = self.get(plane, b.x, b.y - 1);
            let bottom = self.get(plane, b.x, b.y + b.h - 1);
            self.set(plane, b.x, b.y + b.h / 2 - 1, (top + bottom) / 2);
            self.predict_plane_intern(b.up(), plane);
            self.predict_plane_intern(b.down(), plane);
        } else if b.w > step && b.h == step {
            let left = self.get(plane, b.x - 1, b.y);
            let right = self.get(plane, b.x + b.w - 1, b.y);
            self.set(plane, b.x + b.w / 2 - 1, b.y, (left + right) / 2);
            self.predict_plane_intern(b.left(), plane);
            self.predict_plane_intern(b.right(), plane);
        } else {
            let bl = self.get(plane, b.x - 1, b.y + b.h - 1);
            let tr = self.get(plane, b.x + b.w - 1, b.y - 1);
            let br = self.get(plane, b.x + b.w - 1, b.y + b.h - 1);
            let bc = (bl + br) / 2;
            let cr = (tr + br) / 2;
            self.set(plane, b.x + b.w / 2 - 1, b.y + b.h - 1, bc);
            self.set(plane, b.x + b.w - 1, b.y + b.h / 2 - 1, cr);
            let wide = b.w == 4 * step || b.w == 16 * step;
            let high = b.h == 4 * step || b.h == 16 * step;
            let center = if wide != high {
                (self.get(plane, b.x - 1, b.y + b.h / 2 - 1) + cr) / 2
            } else {
                (self.get(plane, b.x + b.w / 2 - 1, b.y - 1) + bc) / 2
            };
            self.set(plane, b.x + b.w / 2 - 1, b.y + b.h / 2 - 1, center);
            self.predict_plane_intern(b.up().left(), plane);
            self.predict_plane_intern(b.up().right(), plane);
            self.predict_plane_intern(b.down().left(), plane);
            self.predict_plane_intern(b.down().right(), plane);
        }
    }

    fn predict_plane(&mut self, b: Block, plane: usize, param: i32) {
        let bl = self.get(plane, b.x - 1, b.y + b.h - 1);
        let tr = self.get(plane, b.x + b.w - 1, b.y - 1);
        self.set(plane, b.x + b.w - 1, b.y + b.h - 1, (bl + tr + 1) / 2 + param);
        self.predict_plane_intern(b, plane);
    }

    fn bounded_se(&mut self) -> R<i32> {
        let value = self.gb.se();
        if !(-(1 << 16)..(1 << 16)).contains(&value) {
            return Err("valeur hors limites");
        }
        Ok(value)
    }

    fn predict_mb_plane(&mut self, b: Block) -> R {
        for plane in 0..3 {
            let param = self.bounded_se()?;
            self.predict_plane(b, plane, param * 2);
        }
        Ok(())
    }

    fn predict_notile_uv(&mut self, b: Block) -> R {
        match self.gb.ue() {
            0 => {
                self.predict_dc(b, 1);
                self.predict_dc(b, 2);
            }
            1 => {
                self.predict_horizontal(b, 1);
                self.predict_horizontal(b, 2);
            }
            2 => {
                self.predict_vertical(b, 1);
                self.predict_vertical(b, 2);
            }
            3 => {
                self.predict_plane(b, 1, 0);
                self.predict_plane(b, 2, 0);
            }
            _ => return Err("mode de chrominance"),
        }
        Ok(())
    }

    fn predict_notile(&mut self, b: Block) -> R {
        match self.gb.ue() {
            0 => self.predict_vertical(b, 0),
            1 => self.predict_horizontal(b, 0),
            2 => self.predict_dc(b, 0),
            3 => self.predict_plane(b, 0, 0),
            _ => return Err("mode de luminance"),
        }
        self.predict_notile_uv(b)
    }

    fn mv_in_bounds(&self, b: Block, v: Mv) -> bool {
        !(b.x + v.x < 0 || b.x + v.x + b.w > self.w || b.y + v.y < 0 || b.y + v.y + b.h > self.h)
    }

    fn predict_inter(&mut self, b: Block, pred: Mv, delta: bool, reference: usize) -> R {
        let Some(source) = self.refs[reference] else { return Err("reference absente") };
        let mut vec = pred;
        if delta {
            vec.x = vec.x.wrapping_add(self.gb.se());
            vec.y = vec.y.wrapping_add(self.gb.se());
        }
        if !self.mv_in_bounds(b, vec) {
            return Err("vecteur hors de l'image");
        }
        let at = (b.y / 16 + 1) * self.vectors_stride + b.x / 16 + 1;
        if let Some(slot) = self.vectors.get_mut(at as usize) {
            *slot = vec;
        }
        self.copy_block(b, vec, source, [0; 3]);
        Ok(())
    }

    fn copy_block(&mut self, b: Block, vec: Mv, source: &Pic, add: [i32; 3]) {
        for (plane, &dc) in add.iter().enumerate() {
            let step = if plane > 0 { 2 } else { 1 };
            for j in (0..b.h).step_by(step) {
                for i in (0..b.w).step_by(step) {
                    let value = px_get(self.w, self.h, source, plane, b.x + i + vec.x, b.y + j + vec.y);
                    self.set(plane, b.x + i, b.y + j, if dc == 0 { value } else { clip8(value + dc) as i32 });
                }
            }
        }
    }

    /// Ce vecteur-là n'entre pas dans la prédiction des suivants.
    fn predict_inter_dc(&mut self, b: Block) -> R {
        let vec = Mv { x: self.gb.se(), y: self.gb.se() };
        let Some(source) = self.refs[0] else { return Err("reference absente") };
        if !self.mv_in_bounds(b, vec) {
            return Err("vecteur hors de l'image");
        }
        let add = [self.bounded_se()? * 2, self.bounded_se()? * 2, self.bounded_se()? * 2];
        self.copy_block(b, vec, source, add);
        Ok(())
    }

    fn idct_add(&mut self, x: i32, y: i32, plane: usize, levels: &[i32; 16]) {
        let step = if plane > 0 { 2 } else { 1 };
        let mut dct = [0i32; 16];
        for i in 0..16 {
            let at = VX_ZIGZAG[i] as usize;
            dct[at] = levels[i].wrapping_mul(self.qtab[(at & 1) + ((at >> 2) & 1)] as i32);
        }
        dct[0] = dct[0].wrapping_add(1 << 5);
        for i in 0..4 {
            let z0 = dct[i].wrapping_add(dct[i + 8]);
            let z1 = dct[i].wrapping_sub(dct[i + 8]);
            let z2 = (dct[i + 4] >> 1).wrapping_sub(dct[i + 12]);
            let z3 = dct[i + 4].wrapping_add(dct[i + 12] >> 1);
            dct[i] = z0.wrapping_add(z3);
            dct[i + 4] = z1.wrapping_add(z2);
            dct[i + 8] = z1.wrapping_sub(z2);
            dct[i + 12] = z0.wrapping_sub(z3);
        }
        for i in 0..4 {
            let at = 4 * i;
            let z0 = dct[at].wrapping_add(dct[at + 2]);
            let z1 = dct[at].wrapping_sub(dct[at + 2]);
            let z2 = (dct[at + 1] >> 1).wrapping_sub(dct[at + 3]);
            let z3 = dct[at + 1].wrapping_add(dct[at + 3] >> 1);
            let column = [z0.wrapping_add(z3), z1.wrapping_add(z2), z1.wrapping_sub(z2), z0.wrapping_sub(z3)];
            for (row, value) in column.into_iter().enumerate() {
                let (px, py) = (x + step * i as i32, y + step * row as i32);
                let current = self.get(plane, px, py);
                self.set(plane, px, py, clip8(current + (value >> 6)) as i32);
            }
        }
    }

    /// CAVLC de H.264. Rend le nombre de coefficients.
    fn residu_cavlc(&mut self, x: i32, y: i32, nc: i32, plane: usize) -> R<i32> {
        let t = tables();
        let table = match nc {
            0..=1 => 0,
            2..=3 => 1,
            4..=7 => 2,
            8..=16 => 3,
            _ => return Err("nc"),
        };
        let token = t.token[table].read(self.gb)? as i32;
        let trailing = token & 3;
        let total = token >> 2;
        if total == 0 {
            return Ok(0);
        }
        if total > 16 {
            return Err("trop de coefficients");
        }
        let mut level = [0i32; 16];
        let mut wpos = 15;
        let mut zeros = 0;
        if total < 16 {
            zeros = t.zeros[total as usize - 1].read(self.gb)? as i32;
            wpos -= 16 - (total + zeros);
        }
        let limit = [0, 3, 6, 12, 24, 48, 0x8000];
        let mut suffix = 0usize;
        let mut left = total;
        let mut ones = trailing;
        loop {
            let value;
            if ones > 0 {
                ones -= 1;
                value = if self.gb.bit() { -1 } else { 1 };
            } else {
                let mut prefix = 0;
                while !self.gb.bit() {
                    prefix += 1;
                    if prefix > 32 {
                        return Err("niveau");
                    }
                }
                let level_suffix = if prefix == 15 { self.gb.get(11) as i32 } else { self.gb.get(suffix as u32) as i32 };
                let mut code = (prefix << suffix) + level_suffix + 1;
                if suffix + 1 < limit.len() && code > limit[suffix + 1] {
                    suffix += 1;
                }
                if self.gb.bit() {
                    code = -code;
                }
                value = code;
            }
            if wpos < 0 {
                return Err("position");
            }
            level[wpos as usize] = value;
            wpos -= 1;
            left -= 1;
            if left == 0 {
                break;
            }
            if zeros != 0 {
                let run = if zeros < 7 { t.run[zeros as usize - 1].read(self.gb)? } else { t.run7.read(self.gb)? } as i32;
                zeros -= run;
                wpos -= run;
            }
        }
        self.idct_add(x, y, plane, &level);
        Ok(total)
    }

    fn luma_residu(&mut self, x: i32, y: i32, coded: bool) -> R {
        let total = if coded {
            let nc = (self.coeff_y_get(x - 1, y) + self.coeff_y_get(x, y - 1) + 1) / 2;
            self.residu_cavlc(x, y, nc, 0)?
        } else {
            0
        };
        self.coeff_y_set(x, y, total);
        Ok(())
    }

    fn residu_blocks(&mut self, b: Block) -> R {
        for y in (b.y..b.y + b.h).step_by(8) {
            for x in (b.x..b.x + b.w).step_by(8) {
                let index = self.gb.ue();
                let Some(&mask) = VX_RESIDU_MASK.get(index as usize) else { return Err("masque") };
                self.luma_residu(x, y, mask & 1 != 0)?;
                self.luma_residu(x + 4, y, mask & 2 != 0)?;
                self.luma_residu(x, y + 4, mask & 4 != 0)?;
                self.luma_residu(x + 4, y + 4, mask & 8 != 0)?;
                let total = if mask & 16 != 0 {
                    let nc = (self.coeff_uv_get(x - 1, y) + self.coeff_uv_get(x, y - 1) + 1) / 2;
                    let u = self.residu_cavlc(x, y, nc, 1)?;
                    let v = self.residu_cavlc(x, y, nc, 2)?;
                    (u + v + 1) / 2
                } else {
                    0
                };
                self.coeff_uv_set(x, y, total);
            }
        }
        Ok(())
    }

    fn clear_coeffs(&mut self, b: Block) {
        for y in (b.y..b.y + b.h).step_by(8) {
            for x in (b.x..b.x + b.w).step_by(8) {
                self.coeff_y_set(x, y, 0);
                self.coeff_y_set(x, y + 4, 0);
                self.coeff_y_set(x + 4, y, 0);
                self.coeff_y_set(x + 4, y + 4, 0);
                self.coeff_uv_set(x, y, 0);
            }
        }
    }

    fn macroblock(&mut self, b: Block, pred: Mv) -> R {
        let mode = self.gb.ue();
        // Avec résidu, sinon les compteurs de coefficients sont remis à zéro.
        let residu = matches!(mode, 8 | 10 | 12 | 13 | 16..=23);
        match mode {
            0 | 8 => {
                if b.w == 2 {
                    return Err("division verticale");
                }
                self.macroblock(b.left(), pred)?;
                self.macroblock(b.right(), pred)?;
                if !residu && b.w == 8 && b.h >= 8 {
                    self.clear_coeffs(b);
                }
            }
            2 | 13 => {
                if b.h == 2 {
                    return Err("division horizontale");
                }
                self.macroblock(b.up(), pred)?;
                self.macroblock(b.down(), pred)?;
                if !residu && b.w >= 8 && b.h == 8 {
                    self.clear_coeffs(b);
                }
            }
            _ => {
                match mode {
                    1 | 12 => self.predict_inter(b, pred, false, 0)?,
                    9 | 20 => self.predict_inter(b, pred, false, 1)?,
                    14 | 21 => self.predict_inter(b, pred, false, 2)?,
                    4 | 16 => self.predict_inter(b, pred, true, 0)?,
                    5 | 17 => self.predict_inter(b, pred, true, 1)?,
                    6 | 18 => self.predict_inter(b, pred, true, 2)?,
                    3 | 10 => self.predict_inter_dc(b)?,
                    7 | 23 => self.predict_mb_plane(b)?,
                    11 | 22 => self.predict_notile(b)?,
                    15 | 19 => self.predict4(b)?,
                    _ => return Err("mode de macrobloc"),
                }
                if !residu && b.w >= 8 && b.h >= 8 {
                    self.clear_coeffs(b);
                }
            }
        }
        if residu {
            self.residu_blocks(b)?;
        }
        Ok(())
    }

    fn decode(&mut self) -> R {
        for y in (0..self.h).step_by(16) {
            for x in (0..self.w).step_by(16) {
                let (by, bx) = (y / 16, x / 16);
                let vec = |f: &Self, row: i32, col: i32| f.vectors[(row * f.vectors_stride + col) as usize];
                let (left, above, right) = (vec(self, by + 1, bx), vec(self, by, bx + 1), vec(self, by, bx + 2));
                let pred = Mv { x: median(left.x, above.x, right.x), y: median(left.y, above.y, right.y) };
                if self.gb.left() < 0 {
                    return Err("fin des donnees");
                }
                self.macroblock(Block { x, y, w: 16, h: 16 }, pred)?;
            }
        }
        self.gb.align16();
        Ok(())
    }
}

/// Le son d'un canal : LPC + impulsions, 128 échantillons par trame.
pub struct AudioChannel {
    codebooks: Vec<[i16; 8]>,
    scale_modifiers: [u16; 8],
    lpc_base: [i32; 8],
    scale_initial: u32,
    have_prev: bool,
    have_prev2: bool,
    pulses_prev: [i32; AFRAME],
    pulses_prev2: [i32; AFRAME],
    samples_prev: [i32; 8],
    scale_prev: i32,
    filter_prev: [i32; 8],
    influence_prev: [i32; 8],
}

const PULSE_WORDS: [usize; 4] = [8, 5, 4, 3];
const PULSE_DISTANCE: [usize; 4] = [3, 3, 4, 5];

impl AudioChannel {
    pub fn new(extra: &[u8]) -> AudioChannel {
        let i16_at = |at: usize| i16::from_le_bytes([extra[at], extra[at + 1]]);
        let codebooks = (0..3 * 64).map(|i| std::array::from_fn(|k| i16_at((i * 8 + k) * 2))).collect();
        let base = 3 * 64 * 8 * 2;
        AudioChannel {
            codebooks,
            scale_modifiers: std::array::from_fn(|i| i16_at(base + i * 2) as u16),
            lpc_base: std::array::from_fn(|i| le32(extra, base + 16 + i * 4) as i32),
            scale_initial: le32(extra, base + 16 + 32),
            have_prev: false,
            have_prev2: false,
            pulses_prev: [0; AFRAME],
            pulses_prev2: [0; AFRAME],
            samples_prev: [0; 8],
            scale_prev: 0,
            filter_prev: [0; 8],
            influence_prev: [0; 8],
        }
    }

    /// Une trame : écrit ses 128 échantillons, un sur `stride` dans `out`.
    pub fn decode(&mut self, gb: &mut Msb, out: &mut [f32], stride: usize) -> R {
        if gb.left() < 32 {
            return Err("son : fin des donnees");
        }
        let word1 = gb.get(16) as usize;
        let word2 = gb.get(16) as usize;
        let prev_offset = (word1 >> 9) & 0x7F;
        let modifier = (word1 >> 6) & 7;
        let start = (word2 >> 14) & 3;
        let packing = (word2 >> 12) & 3;
        let lpc = [word1 & 0x3F, (word2 >> 6) & 0x3F, word2 & 0x3F];
        if gb.left() < 16 * PULSE_WORDS[packing] as isize {
            return Err("son : fin des donnees");
        }
        let words: Vec<u32> = (0..PULSE_WORDS[packing]).map(|_| gb.get(16)).collect();
        let mut values = Vec::with_capacity(42);
        if packing == 0 {
            for word in &words {
                for shift in (0..=13).rev().step_by(3) {
                    values.push(((word >> shift) & 7) as i32);
                }
            }
            values.push((((words[0] & 1) << 2 | (words[1] & 1) << 1 | (words[2] & 1)) & 7) as i32);
            values.push((((words[3] & 1) << 2 | (words[4] & 1) << 1 | (words[5] & 1)) & 7) as i32);
            values.iter_mut().for_each(|v| *v = *v * 2 - 7);
        } else {
            for word in &words {
                for shift in (0..=14).rev().step_by(2) {
                    values.push(((word >> shift) & 3) as i32);
                }
            }
            values.iter_mut().for_each(|v| *v = *v * 2 - 3);
        }
        if prev_offset != 0x7F && !self.have_prev {
            return Err("son : pas de trame precedente");
        }
        let mut scale = if self.have_prev && prev_offset != 0x7F { self.scale_prev } else { self.scale_initial as i32 };
        let scaled = (scale as i64 * self.scale_modifiers[modifier] as i64) >> 13;
        scale = scaled.clamp(-(i32::MAX as i64 / 16), i32::MAX as i64 / 16) as i32;

        let mut filter = if prev_offset == 0x7F { self.lpc_base } else { self.filter_prev };
        for (k, value) in filter.iter_mut().enumerate() {
            let mut sum = *value as i64;
            for (book, &index) in lpc.iter().enumerate() {
                sum += self.codebooks[book * 64 + index][k] as i64;
            }
            *value = i32::try_from(sum).map_err(|_| "son : filtre")?;
        }

        let mut pulses = [0i32; AFRAME];
        if prev_offset < 0x7E {
            let at = |i: usize| {
                let k = i + 0x7F - prev_offset;
                if k < AFRAME {
                    if self.have_prev2 { self.pulses_prev2[k] } else { 0 }
                } else {
                    self.pulses_prev.get(k - AFRAME).copied().unwrap_or(0)
                }
            };
            for (i, pulse) in pulses.iter_mut().enumerate() {
                let volume = 8.min(i + 1).min(AFRAME - i) as i64;
                *pulse = ((at(i) as i64 * volume) >> 4) as i32;
            }
        }
        let distance = PULSE_DISTANCE[packing];
        for (idx, value) in values.iter().enumerate() {
            let at = start + idx * distance;
            if at < AFRAME {
                pulses[at] = pulses[at].wrapping_add(value.wrapping_mul(scale));
            }
        }

        let mut psi = [0i32; 8];
        for i in 0..8 {
            let coeff = filter[i];
            let old = psi;
            for j in 0..i {
                let v = old[j] as i64 + ((old[i - j - 1] as i64 * coeff as i64) >> 15);
                psi[j] = i32::try_from(v).map_err(|_| "son : treillis")?;
            }
            psi[i] = coeff;
        }
        let influence: [i32; 8] = std::array::from_fn(|i| -(psi[i] >> 1));

        let mut quarters = [influence; 4];
        if prev_offset != 0x7F {
            for j in 0..8 {
                quarters[1][j] = (self.influence_prev[j] + quarters[3][j]) >> 1;
                quarters[0][j] = (self.influence_prev[j] + quarters[1][j]) >> 1;
                quarters[2][j] = (quarters[1][j] + quarters[3][j]) >> 1;
            }
        }
        let mut samples = [0i32; AFRAME];
        for i in 0..AFRAME {
            let weights = &quarters[i * 4 / AFRAME];
            let mut sample = pulses[i] as i64 * 0x4000;
            for (j, &weight) in weights.iter().enumerate() {
                let previous = if i > j { samples[i - 1 - j] } else { self.samples_prev[7 + i - j] };
                sample += previous as i64 * weight as i64;
            }
            samples[i] = i32::try_from(sample >> 14).map_err(|_| "son : echantillon")?;
        }
        for (i, &sample) in samples.iter().enumerate() {
            out[i * stride] = sample.clamp(-32768, 32767) as f32 / 32768.0;
        }
        self.pulses_prev2 = self.pulses_prev;
        self.have_prev2 = self.have_prev;
        self.pulses_prev = pulses;
        self.samples_prev.copy_from_slice(&samples[AFRAME - 8..]);
        self.scale_prev = scale;
        self.filter_prev = filter;
        self.influence_prev = influence;
        self.have_prev = true;
        Ok(())
    }
}

/// `periods` trames par canal, entrelacées (ch0, ch1...), ajoutées à `pcm`.
pub fn read_aframes(gb: &mut Msb, channels: &mut [AudioChannel], periods: usize, pcm: &mut Vec<f32>) -> R {
    let nch = channels.len().max(1);
    let start = pcm.len();
    pcm.resize(start + periods * AFRAME * nch, 0.0);
    for period in 0..periods {
        for (c, channel) in channels.iter_mut().enumerate() {
            let at = start + period * AFRAME * nch + c;
            channel.decode(gb, &mut pcm[at..], nch)?;
        }
    }
    Ok(())
}

pub struct Vx {
    bytes: std::sync::Arc<[u8]>,
    frames: usize,
    w: usize,
    h: usize,
    qtab: [u16; 3],
    nch: usize,
    audio_at: usize,
    audio: Vec<AudioChannel>,
    pics: [Pic; 4],
    cur: usize,
    pos: usize,
    index: usize,
}

const DATA_START: usize = 0x30;

impl Vx {
    pub fn open(bytes: std::sync::Arc<[u8]>) -> Result<(Vx, Info), String> {
        if bytes.len() < DATA_START {
            return Err("VX : en-tete trop court".into());
        }
        let u = |at: usize| le32(&bytes, at);
        let (frames, w, h, fps, quantizer, rate, nch, audio_at) = (u(4) as usize, u(8) as usize, u(0xC) as usize, u(0x10), u(0x14) as i32, u(0x18), u(0x1C) as usize, u(0x24) as usize);
        if w == 0 || h == 0 || w % 16 != 0 || h % 16 != 0 || w > 2048 || h > 2048 || nch > 2 {
            return Err(format!("VX : taille {w}x{h}, {nch} canaux"));
        }
        if !(12..=161).contains(&quantizer) {
            return Err(format!("VX : quantificateur {quantizer}"));
        }
        let (qx, qy) = ((quantizer % 6) as usize, quantizer / 6);
        let qtab = std::array::from_fn(|i| VX_QUANT[qx][i] << qy);
        let channels = if rate > 0 && bytes.len() >= audio_at + nch * AUDIO_EXTRA { nch } else { 0 };
        let info = Info { format: "VX", width: w as u32, height: h as u32, frames: frames as u32, fps: if fps > 0 { fps as f32 / 65536.0 } else { 30.0 }, sample_rate: if channels > 0 { rate } else { 0 }, channels: channels as u16 };
        let mut vx = Vx { bytes, frames, w, h, qtab, nch: channels, audio_at, audio: Vec::new(), pics: Default::default(), cur: 0, pos: DATA_START, index: 0 };
        vx.rewind();
        Ok((vx, info))
    }
}

impl Stream for Vx {
    fn yuv(&self, out: &mut Vec<u8>) {
        out.clear();
        self.pics[(self.cur + 3) % 4].iter().for_each(|plane| out.extend_from_slice(plane));
    }

    fn rewind(&mut self) {
        self.audio = (0..self.nch).map(|c| AudioChannel::new(&self.bytes[self.audio_at + c * AUDIO_EXTRA..])).collect();
        self.pics = Default::default();
        self.cur = 0;
        self.pos = DATA_START;
        self.index = 0;
    }

    fn next(&mut self, pcm: &mut Vec<f32>) -> Option<R> {
        if self.index >= self.frames || self.pos + 4 > self.bytes.len() {
            return None;
        }
        self.index += 1;
        let size = le16(&self.bytes, self.pos) as usize;
        let aframes = le16(&self.bytes, self.pos + 2) as usize;
        if size < 2 || self.pos + 2 + size > self.bytes.len() {
            self.index = self.frames;
            return Some(Err("taille d'image"));
        }
        let mut gb = Msb::from_words(&self.bytes[self.pos + 4..self.pos + 2 + size]);
        self.pos += 2 + size;

        let (w, h) = (self.w, self.h);
        let mut dst = std::mem::take(&mut self.pics[self.cur]);
        for (plane, data) in dst.iter_mut().enumerate() {
            let len = if plane == 0 { w * h } else { w / 2 * (h / 2) };
            data.clear();
            data.resize(len, 0);
        }
        let refs = std::array::from_fn(|i| {
            let pic = &self.pics[(self.cur + 3 - i) % 4];
            (!pic[0].is_empty()).then_some(pic)
        });
        let (coeff_y_stride, coeff_uv_stride, vectors_stride) = ((w / 4 + 1) as i32, (w / 8 + 1) as i32, (w / 16 + 2) as i32);
        let mut frame = Frame {
            gb: &mut gb,
            w: w as i32,
            h: h as i32,
            qtab: self.qtab,
            dst: &mut dst,
            refs,
            coeff_y: vec![0; (h / 4 + 1) * coeff_y_stride as usize],
            coeff_y_stride,
            coeff_uv: vec![0; (h / 8 + 1) * coeff_uv_stride as usize],
            coeff_uv_stride,
            vectors: vec![Mv::default(); (h / 16 + 1) * vectors_stride as usize],
            vectors_stride,
        };
        let result = frame.decode();
        self.pics[self.cur] = dst;
        if result.is_err() {
            // Sans la fin de l'image, on ne sait pas où commence le son : du silence à la place
            pcm.resize(pcm.len() + aframes * AFRAME * self.nch, 0.0);
            return Some(result);
        }
        self.cur = (self.cur + 1) % 4;
        if aframes > 0 && self.nch > 0 {
            let start = pcm.len();
            if let Err(e) = read_aframes(&mut gb, &mut self.audio, aframes, pcm) {
                pcm.truncate(start);
                pcm.resize(start + aframes * AFRAME * self.nch, 0.0);
                return Some(Err(e));
            }
        }
        Some(Ok(()))
    }

    fn rgba(&self, out: &mut Vec<u8>) {
        let pic = &self.pics[(self.cur + 3) % 4];
        out.clear();
        out.resize(self.w * self.h * 4, 255);
        if pic[0].is_empty() {
            return;
        }
        for y in 0..self.h {
            for x in 0..self.w {
                let luma = pic[0][y * self.w + x] as i32;
                let at = (y / 2) * (self.w / 2) + x / 2;
                let (cu, cv) = (pic[1][at] as i32 - 128, pic[2][at] as i32 - 128);
                let px = &mut out[(y * self.w + x) * 4..][..3];
                px[0] = clip8(luma + 2 * cv);
                px[1] = clip8(luma - (cu >> 1) - cv);
                px[2] = clip8(luma + 2 * cu);
            }
        }
    }
}
