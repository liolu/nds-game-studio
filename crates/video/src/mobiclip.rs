//! Mobiclip (`.mods`) : le conteneur MODS, l'image et le son.
//! Portage de libavcodec/mobiclip.c, fastaudio.c et libavformat/mods.c
//! (Florian Nouwt, Adib Surani, Paul B Mahol, quatric ; LGPL 2.1+).

use std::sync::{Arc, OnceLock};

use crate::bits::{Msb, R, Vlc, clip8};
use crate::tables::*;
use crate::vx::{AUDIO_EXTRA, AudioChannel, read_aframes};
use crate::{Info, Stream, le16, le32};

const ZIGZAG8: [u8; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

struct Tables {
    rl: [Vlc; 2],
    mv: Vec<Vlc>,
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| Tables {
        rl: [Vlc::from_lengths(&MC_RL_LEN, |i| MC_RL_SYMS0[i]), Vlc::from_lengths(&MC_RL_LEN, |i| MC_RL_SYMS1[i])],
        mv: (0..2).flat_map(|m| (0..16).map(move |t| Vlc::from_lengths(&MC_MV_LEN[m][t][..MC_MV_COUNT[t] as usize], |i| MC_MV_SYMS[m][t][i] as u16))).collect(),
    })
}

type Pic = [Vec<u8>; 3];

struct Px<'a> {
    w: i32,
    h: i32,
    ax: i32,
    ay: i32,
    x: i32,
    y: i32,
    size: i32,
    block: &'a [u8],
    stride: i32,
}

impl Px<'_> {
    fn at(&self, x: i32, y: i32) -> i32 {
        Px { x, y, ..*self }.pget()
    }

    fn pget(&self) -> i32 {
        let (mut x, mut y) = (self.x, self.y);
        if x == -1 && y >= self.size {
            y = self.size - 1;
        } else if x >= -1 && y >= -1 {
        } else if x == -1 && y == -2 {
            x = 0;
            y = -1;
        } else if x == -2 && y == -1 {
            x = -1;
            y = 0;
        }
        let row = (self.ay + y).clamp(0, self.h - 1);
        let col = (self.ax + x).clamp(0, self.w - 1);
        self.block[(row * self.stride + col) as usize] as i32
    }
}

fn half(a: i32, b: i32) -> i32 {
    (a + b + 1) / 2
}

fn half3(a: i32, b: i32, c: i32) -> i32 {
    ((a + b + b + c) * 2 / 4 + 1) / 2
}

fn half_horz(p: &Px, x: i32, y: i32) -> i32 {
    half3(p.at(x - 1, y), p.at(x, y), p.at(x + 1, y))
}

fn half_vert(p: &Px, x: i32, y: i32) -> i32 {
    half3(p.at(x, y - 1), p.at(x, y), p.at(x, y + 1))
}

fn pick_4(p: &Px) -> i32 {
    if p.x % 2 == 0 { half(p.at(-1, p.y + p.x / 2), p.at(-1, p.y + p.x / 2 + 1)) } else { half_vert(p, -1, p.y + p.x / 2 + 1) }
}

fn pick_5(p: &Px) -> i32 {
    if p.x == 0 {
        half(p.at(-1, p.y - 1), p.at(-1, p.y))
    } else if p.y == 0 {
        half_horz(p, p.x - 2, p.y - 1)
    } else if p.x == 1 {
        half_vert(p, p.x - 2, p.y - 1)
    } else {
        p.at(p.x - 2, p.y - 1)
    }
}

fn pick_6(p: &Px) -> i32 {
    if p.y == 0 {
        half(p.at(p.x - 1, -1), p.at(p.x, -1))
    } else if p.x == 0 {
        half_vert(p, p.x - 1, p.y - 2)
    } else if p.y == 1 {
        half_horz(p, p.x - 1, p.y - 2)
    } else {
        p.at(p.x - 1, p.y - 2)
    }
}

fn pick_7(p: &Px) -> i32 {
    let color = p.at(p.x - 1, p.y - 1);
    if p.x != 0 && p.y != 0 {
        return color;
    }
    let first = if p.x == 0 { p.at(-1, p.y) } else { p.at(p.x - 2, -1) };
    let second = if p.y == 0 { p.at(p.x, -1) } else { p.at(-1, p.y - 2) };
    half3(first, color, second)
}

fn pick_8(p: &Px) -> i32 {
    if p.y == 0 {
        half(p.at(p.x, -1), p.at(p.x + 1, -1))
    } else if p.y == 1 {
        half_horz(p, p.x + 1, p.y - 2)
    } else if p.x < p.size - 1 {
        p.at(p.x + 1, p.y - 2)
    } else if p.y % 2 == 0 {
        half(p.at(p.y / 2 + p.size - 1, -1), p.at(p.y / 2 + p.size, -1))
    } else {
        half_horz(p, p.y / 2 + p.size, -1)
    }
}

fn inverse4(rs: &mut [i32]) {
    let a = rs[0].wrapping_add(rs[2]);
    let b = rs[0].wrapping_sub(rs[2]);
    let c = rs[1].wrapping_add(rs[3] >> 1);
    let d = (rs[1] >> 1).wrapping_sub(rs[3]);
    rs[0] = a.wrapping_add(c);
    rs[1] = b.wrapping_add(d);
    rs[2] = b.wrapping_sub(d);
    rs[3] = a.wrapping_sub(c);
}

fn idct(arr: &mut [i32], size: usize) {
    if size == 4 {
        inverse4(arr);
        return;
    }
    let mut tmp = [arr[0], arr[2], arr[4], arr[6]];
    inverse4(&mut tmp);
    let e = arr[7].wrapping_add(arr[1]).wrapping_sub(arr[3]).wrapping_sub(arr[3] >> 1);
    let f = arr[7].wrapping_sub(arr[1]).wrapping_add(arr[5]).wrapping_add(arr[5] >> 1);
    let g = arr[5].wrapping_sub(arr[3]).wrapping_sub(arr[7]).wrapping_sub(arr[7] >> 1);
    let h = arr[5].wrapping_add(arr[3]).wrapping_add(arr[1]).wrapping_add(arr[1] >> 1);
    let x3 = g.wrapping_add(h >> 2);
    let x2 = e.wrapping_add(f >> 2);
    let x1 = (e >> 2).wrapping_sub(f);
    let x0 = h.wrapping_sub(g >> 2);
    arr[0] = tmp[0].wrapping_add(x0);
    arr[1] = tmp[1].wrapping_add(x1);
    arr[2] = tmp[2].wrapping_add(x2);
    arr[3] = tmp[3].wrapping_add(x3);
    arr[4] = tmp[3].wrapping_sub(x3);
    arr[5] = tmp[2].wrapping_sub(x2);
    arr[6] = tmp[1].wrapping_sub(x1);
    arr[7] = tmp[0].wrapping_sub(x0);
}

fn adjust(x: i32, size: i32) -> i32 {
    if size == 16 { (x + 1) >> 1 } else { x }
}

fn index_of(x: i32) -> usize {
    match x {
        16 => 0,
        8 => 1,
        4 => 2,
        2 => 3,
        _ => 0,
    }
}

/// Le décodeur d'images, sans le conteneur.
pub struct Mobiclip {
    w: i32,
    h: i32,
    pics: Vec<Pic>,
    current: usize,
    moflex: usize,
    dct_tab: usize,
    quantizer: i32,
    qtab: [[i32; 64]; 2],
    pre: [u8; 32],
    motion: Vec<(i32, i32)>,
}

impl Mobiclip {
    pub fn new(w: i32, h: i32) -> Mobiclip {
        Mobiclip { w, h, pics: vec![Pic::default(); 6], current: 0, moflex: 0, dct_tab: 0, quantizer: 0, qtab: [[0; 64]; 2], pre: [9; 32], motion: vec![(0, 0); (w / 16 + 3) as usize] }
    }

    fn stride(&self, plane: usize) -> i32 {
        if plane == 0 { self.w } else { self.w >> 1 }
    }

    fn setup_qtables(&mut self, quantizer: i64) -> R {
        if !(12..=161).contains(&quantizer) {
            return Err("quantificateur");
        }
        let quantizer = quantizer as i32;
        self.quantizer = quantizer;
        let (qx, qy) = ((quantizer % 6) as usize, quantizer / 6);
        for i in 0..16 {
            self.qtab[0][i] = (MC_QUANT4[qx][i] as i32) << qy;
        }
        for i in 0..64 {
            self.qtab[1][i] = (MC_QUANT8[qx][i] as i32) << (qy - 2);
        }
        self.pre[..20].fill(9);
        Ok(())
    }

    fn read_run(&self, gb: &mut Msb) -> R<(bool, i32, i32)> {
        let n = tables().rl[self.dct_tab].read(gb)? as i32;
        Ok(((n >> 11) == 1, (n >> 5) & 0x3F, n & 0x1F))
    }

    fn add_coefficients(&self, gb: &mut Msb, dst: &mut Pic, bx: i32, by: i32, size: i32, plane: usize) -> R {
        let mut mat = [0i32; 64];
        let total = (size * size) as usize;
        let qtab = &self.qtab[usize::from(size == 8)];
        let mut pos = 0usize;
        while gb.left() > 0 {
            let (mut last, mut run, mut level) = self.read_run(gb)?;
            let residue = &MC_RUN_RESIDUE[self.dct_tab];
            if level != 0 {
                if gb.bit() {
                    level = -level;
                }
            } else if !gb.bit() {
                (last, run, level) = self.read_run(gb)?;
                level += residue[(if last { 64 } else { 0 }) + run as usize] as i32;
                if gb.bit() {
                    level = -level;
                }
            } else if !gb.bit() {
                (last, run, level) = self.read_run(gb)?;
                run += residue[128 + (if last { 64 } else { 0 }) + level as usize] as i32;
                if gb.bit() {
                    level = -level;
                }
            } else {
                last = gb.bit();
                run = gb.get(6) as i32;
                level = gb.signed(12);
            }
            pos += run as usize;
            if pos >= total {
                return Err("coefficient hors du bloc");
            }
            let at = if size == 8 { ZIGZAG8[pos] } else { MC_ZIGZAG4[pos] } as usize;
            mat[at] = qtab[pos].wrapping_mul(level);
            if last {
                break;
            }
            pos += 1;
        }
        mat[0] = mat[0].wrapping_add(32);
        let n = size as usize;
        for y in 0..n {
            idct(&mut mat[y * n..(y + 1) * n], n);
        }
        let stride = self.stride(plane);
        for y in 0..n {
            for x in y + 1..n {
                mat.swap(x * n + y, y * n + x);
            }
            idct(&mut mat[y * n..(y + 1) * n], n);
            let row = ((by + y as i32) * stride + bx) as usize;
            for x in 0..n {
                let p = &mut dst[plane][row + x];
                *p = clip8(*p as i32 + (mat[y * n + x] >> 6));
            }
        }
        Ok(())
    }

    fn add_pframe_coefficients(&self, gb: &mut Msb, dst: &mut Pic, bx: i32, by: i32, size: i32, plane: usize) -> R {
        let index = gb.ue() as usize;
        if index == 0 {
            return self.add_coefficients(gb, dst, bx, by, size, plane);
        }
        let Some(&table) = MC_PBLOCK4.get(index) else { return Err("bloc") };
        let mut flags = table;
        for y in (by..by + 8).step_by(4) {
            for x in (bx..bx + 8).step_by(4) {
                if flags & 1 != 0 {
                    self.add_coefficients(gb, dst, x, y, 4, plane)?;
                }
                flags >>= 1;
            }
        }
        Ok(())
    }

    fn fill(&self, dst: &mut Pic, plane: usize, size: i32, ax: i32, ay: i32, pick: fn(&Px) -> i32) {
        let (w, h, stride) = (self.w >> i32::from(plane > 0), self.h >> i32::from(plane > 0), self.stride(plane));
        for y in 0..size {
            for x in 0..size {
                let value = pick(&Px { w, h, ax, ay, x, y, size, block: &dst[plane], stride });
                dst[plane][((ay + y) * stride + ax + x) as usize] = value as u8;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn predict_intra(&self, gb: &mut Msb, dst: &mut Pic, ax: i32, ay: i32, pmode: i32, add: bool, size: i32, plane: usize) -> R {
        let stride = self.stride(plane);
        let at = |x: i32, y: i32| (y * stride + x) as usize;
        match pmode {
            0 => self.fill(dst, plane, size, ax, ay, |p| p.at(p.x, p.y - 1)),
            1 => self.fill(dst, plane, size, ax, ay, |p| p.at(p.x - 1, p.y)),
            2 => {
                // `top` et `left` peuvent être le bloc lui-même au bord de l'image : on relit à chaque fois
                let top = at(ax, (ay - 1).max(0));
                let left = at((ax - 1).max(0), ay);
                let bottommost = dst[plane][at((ax - 1).max(0), ay + size - 1)] as i32;
                let rightmost = dst[plane][at(ax + size - 1, (ay - 1).max(0))] as i32;
                let avg = (bottommost + rightmost + 1) / 2 + 2 * gb.se().clamp(-(1 << 16), 1 << 16);
                let r6 = adjust(avg - bottommost, size);
                let r9 = adjust(avg - rightmost, size);
                let shift = if adjust(size, size) == 8 { 3 } else { 2 };
                let mut arr1 = [0i32; 16];
                let mut arr2 = [0i32; 16];
                for x in 0..size {
                    let value = dst[plane][top + x as usize] as i32;
                    arr1[x as usize] = adjust(((bottommost - value) * (1 << shift)) + r6 * (x + 1), size);
                }
                for y in 0..size {
                    let value = dst[plane][left + (y * stride) as usize] as i32;
                    arr2[y as usize] = adjust(((rightmost - value) * (1 << shift)) + r9 * (y + 1), size);
                }
                for y in 0..size {
                    for x in 0..size {
                        let t = dst[plane][top + x as usize] as i32;
                        let l = dst[plane][left + (y * stride) as usize] as i32;
                        let v = (t + l + ((arr1[x as usize] * (y + 1) + arr2[y as usize] * (x + 1)) >> (2 * shift)) + 1) / 2;
                        dst[plane][at(ax + x, ay + y)] = v as u8;
                    }
                }
            }
            3 => {
                let sum_left = |d: &Pic| (0..size).map(|y| d[plane][at(ax - 1, ay + y)] as i32).sum::<i32>();
                let sum_top = |d: &Pic| (0..size).map(|x| d[plane][at(ax + x, ay - 1)] as i32).sum::<i32>();
                let fill = if ax == 0 && ay == 0 {
                    0x80
                } else if ax >= 1 && ay >= 1 {
                    ((sum_left(dst) + sum_top(dst)) * 2 / (2 * size) + 1) / 2
                } else if ax >= 1 {
                    (sum_left(dst) * 2 / size + 1) / 2
                } else {
                    (sum_top(dst) * 2 / size + 1) / 2
                };
                for y in 0..size {
                    for x in 0..size {
                        dst[plane][at(ax + x, ay + y)] = fill as u8;
                    }
                }
            }
            4 => self.fill(dst, plane, size, ax, ay, pick_4),
            5 => self.fill(dst, plane, size, ax, ay, pick_5),
            6 => self.fill(dst, plane, size, ax, ay, pick_6),
            7 => self.fill(dst, plane, size, ax, ay, pick_7),
            8 => self.fill(dst, plane, size, ax, ay, pick_8),
            _ => {}
        }
        if add {
            self.add_coefficients(gb, dst, ax, ay, size, plane)?;
        }
        Ok(())
    }

    fn get_prediction(&mut self, gb: &mut Msb, x: i32, y: i32, size: i32) -> i32 {
        let index = ((y & 0xC) | (x / 4 % 4)) as usize;
        let mut value = self.pre[index].min(if index % 4 == 0 { 9 } else { self.pre[index + 3] });
        if value == 9 {
            value = 3;
        }
        if !gb.bit() {
            let read = gb.get(3) as u8;
            value = read + u8::from(read >= value);
        }
        self.pre[index + 4] = value;
        if size == 8 {
            self.pre[index + 5] = value;
            self.pre[index + 8] = value;
            self.pre[index + 9] = value;
        }
        value as i32
    }

    #[allow(clippy::too_many_arguments)]
    fn process_block(&mut self, gb: &mut Msb, dst: &mut Pic, x: i32, y: i32, pmode: i32, has_coeffs: bool, plane: usize) -> R {
        if !has_coeffs {
            let mode = if pmode < 0 { self.get_prediction(gb, x, y, 8) } else { pmode };
            return self.predict_intra(gb, dst, x, y, mode, false, 8, plane);
        }
        let tmp = gb.ue() as usize;
        if tmp > MC_BLOCK4.len() {
            return Err("bloc");
        }
        if tmp == 0 {
            let mode = if pmode < 0 { self.get_prediction(gb, x, y, 8) } else { pmode };
            return self.predict_intra(gb, dst, x, y, mode, true, 8, plane);
        }
        let mut flags = MC_BLOCK4[tmp - 1];
        for by in (y..y + 8).step_by(4) {
            for bx in (x..x + 8).step_by(4) {
                let mode = if pmode < 0 { self.get_prediction(gb, bx, by, 4) } else { pmode };
                self.predict_intra(gb, dst, bx, by, mode, flags & 1 != 0, 4, plane)?;
                flags >>= 1;
            }
        }
        Ok(())
    }

    fn decode_macroblock(&mut self, gb: &mut Msb, dst: &mut Pic, x: i32, y: i32, predict: bool) -> R {
        let index = gb.ue() as usize;
        let Some(&table) = MC_BLOCK8.get(index) else { return Err("bloc 8x8") };
        let mut flags = table;
        let mut pmode = -1;
        if !predict {
            pmode = gb.get(3) as i32;
            if pmode == 2 {
                self.predict_intra(gb, dst, x, y, pmode, false, 16, 0)?;
                pmode = 9;
            }
        }
        for (bx, by) in [(x, y), (x + 8, y), (x, y + 8), (x + 8, y + 8)] {
            self.process_block(gb, dst, bx, by, pmode, flags & 1 != 0, 0)?;
            flags >>= 1;
        }
        let mut pmode_uv = gb.get(3) as i32;
        let (first, second) = (1 + usize::from(self.moflex == 0), 2 - usize::from(self.moflex == 0));
        if pmode_uv == 2 {
            self.predict_intra(gb, dst, x >> 1, y >> 1, pmode_uv, false, 8, first)?;
            self.predict_intra(gb, dst, x >> 1, y >> 1, pmode_uv, false, 8, second)?;
            pmode_uv = 9;
        }
        self.process_block(gb, dst, x >> 1, y >> 1, pmode_uv, flags & 1 != 0, first)?;
        flags >>= 1;
        self.process_block(gb, dst, x >> 1, y >> 1, pmode_uv, flags & 1 != 0, second)
    }

    #[allow(clippy::too_many_arguments)]
    fn predict_motion(&mut self, gb: &mut Msb, dst: &mut Pic, mut width: i32, mut height: i32, index: i32, offsetm: usize, mut offsetx: i32, mut offsety: i32) -> R {
        if index > 5 {
            let adjx = if index == 8 { 0 } else { width / 2 };
            let adjy = if index == 8 { height / 2 } else { 0 };
            width -= adjx;
            height -= adjy;
            let tidx = index_of(height) * 4 + index_of(width);
            for i in 0..2 {
                let next = tables().mv[self.moflex * 16 + tidx].read(gb)? as i32;
                self.predict_motion(gb, dst, width, height, next, offsetm, offsetx + i * adjx, offsety + i * adjy)?;
            }
            return Ok(());
        }
        let back = index.max(1) as usize;
        let mut source = (self.current + 6 - back) % 6;
        // Une référence qui n'existe pas encore : la plus proche
        if self.pics[source][0].is_empty() {
            for d in 1..=5 {
                let candidate = (self.current + 6 - d) % 6;
                if !self.pics[candidate][0].is_empty() {
                    source = candidate;
                    break;
                }
            }
        }
        let (mut mx, mut my) = self.motion[0];
        if index > 0 {
            mx = mx.wrapping_add(gb.se());
            my = my.wrapping_add(gb.se());
        }
        if mx == i32::MAX || my == i32::MAX {
            return Err("vecteur");
        }
        self.motion[offsetm] = (mx, my);
        let (mut fwidth, mut fheight) = (self.w, self.h);
        for plane in 0..3 {
            if plane == 1 {
                offsetx >>= 1;
                offsety >>= 1;
                mx >>= 1;
                my >>= 1;
                width >>= 1;
                height >>= 1;
                fwidth >>= 1;
                fheight >>= 1;
            }
            let src = &self.pics[source][plane];
            if src.is_empty() {
                return Err("reference absente");
            }
            if offsetx + (mx >> 1) < 0 || offsety + (my >> 1) < 0 || offsetx + width + ((mx + 1) >> 1) > fwidth || offsety + height + ((my + 1) >> 1) > fheight {
                return Err("vecteur hors de l'image");
            }
            let stride = self.stride(plane);
            let method = (mx & 1) | ((my & 1) << 1);
            let first = ((offsety + (my >> 1)) * stride + offsetx + (mx >> 1)) as usize;
            let s = stride as usize;
            for y in 0..height as usize {
                let row = first + y * s;
                let out = (offsety as usize + y) * s + offsetx as usize;
                for x in 0..width as usize {
                    let p = |at: usize| src.get(at).copied().unwrap_or(0);
                    dst[plane][out + x] = match method {
                        0 => p(row + x),
                        1 => (p(row + x) >> 1) + (p(row + x + 1) >> 1),
                        2 => (p(row + x) >> 1) + (p(row + x + s) >> 1),
                        _ => ((p(row + x) >> 1) + (p(row + x + 1) >> 1)) / 2 + ((p(row + x + s) >> 1) + (p(row + x + 1 + s) >> 1)) / 2,
                    };
                }
            }
        }
        Ok(())
    }

    /// Une image ; rend le nombre d'octets lus.
    pub fn decode(&mut self, packet: &[u8]) -> R<usize> {
        if (self.h / 16 * (self.w / 16) * 2) as usize > 8 * packet.len().div_ceil(2) * 2 {
            return Err("paquet trop court");
        }
        let mut gb = Msb::from_words(packet);
        let mut dst = std::mem::take(&mut self.pics[self.current]);
        if dst[0].is_empty() {
            let (w, h) = (self.w as usize, self.h as usize);
            dst = [vec![0; w * h], vec![0; (w >> 1) * (h >> 1)], vec![0; (w >> 1) * (h >> 1)]];
        }
        let result = self.decode_picture(&mut gb, &mut dst);
        self.pics[self.current] = dst;
        result?;
        self.current = (self.current + 1) % 6;
        Ok(gb.pos().div_ceil(16) * 2)
    }

    fn decode_picture(&mut self, gb: &mut Msb, dst: &mut Pic) -> R {
        if gb.bit() {
            self.moflex = gb.get(1) as usize;
            self.dct_tab = gb.get(1) as usize;
            self.setup_qtables(gb.get(6) as i64)?;
            for y in (0..self.h).step_by(16) {
                for x in (0..self.w).step_by(16) {
                    let predict = gb.bit();
                    self.decode_macroblock(gb, dst, x, y, predict)?;
                }
            }
            return Ok(());
        }
        self.motion.fill((0, 0));
        self.dct_tab = 0;
        let delta = gb.se() as i64;
        self.setup_qtables(self.quantizer as i64 + delta)?;
        for y in (0..self.h).step_by(16) {
            for x in (0..self.w).step_by(16) {
                let col = (x / 16) as usize;
                let (a, b, c) = (self.motion[col + 1], self.motion[col + 2], self.motion[col + 3]);
                let median = |a: i32, b: i32, c: i32| a.min(b).max(a.max(b).min(c));
                self.motion[0] = (median(a.0, b.0, c.0), median(a.1, b.1, c.1));
                self.motion[col + 2] = (0, 0);
                let index = tables().mv[self.moflex * 16].read(gb)? as i32;
                if index == 6 || index == 7 {
                    self.decode_macroblock(gb, dst, x, y, index == 7)?;
                    continue;
                }
                self.predict_motion(gb, dst, 16, 16, index, col + 2, x, y)?;
                let Some(&table) = MC_PBLOCK8.get(gb.ue() as usize) else { return Err("bloc 8x8") };
                let mut flags = table;
                for sy in (y..y + 16).step_by(8) {
                    for sx in (x..x + 16).step_by(8) {
                        if flags & 1 != 0 {
                            let _ = self.add_pframe_coefficients(gb, dst, sx, sy, 8, 0);
                        }
                        flags >>= 1;
                    }
                }
                let (first, second) = (1 + usize::from(self.moflex == 0), 2 - usize::from(self.moflex == 0));
                if flags & 1 != 0 {
                    let _ = self.add_pframe_coefficients(gb, dst, x >> 1, y >> 1, 8, first);
                }
                flags >>= 1;
                if flags & 1 != 0 {
                    let _ = self.add_pframe_coefficients(gb, dst, x >> 1, y >> 1, 8, second);
                }
            }
        }
        Ok(())
    }

    fn last(&self) -> &Pic {
        &self.pics[(self.current + 5) % 6]
    }
}

/// FastAudio : 256 échantillons pour 40 octets par canal.
struct FastAudio {
    table: [[f32; 64]; 8],
    channels: Vec<([f32; 8], f32)>,
}

const FAST_BITS: [u32; 8] = [6, 6, 5, 5, 4, 0, 3, 3];

impl FastAudio {
    fn new(channels: usize) -> FastAudio {
        let mut t = [[0f32; 64]; 8];
        for i in 0..8 {
            t[0][i] = (i as f32 - 159.5) / 160.0;
        }
        for i in 0..11 {
            t[0][i + 8] = (i as f32 - 37.5) / 40.0;
        }
        for i in 0..27 {
            t[0][i + 19] = (i as f32 - 13.0) / 20.0;
        }
        for i in 0..11 {
            t[0][i + 46] = (i as f32 + 27.5) / 40.0;
        }
        for i in 0..7 {
            t[0][i + 57] = (i as f32 + 152.5) / 160.0;
        }
        t[1] = t[0];
        for i in 0..7 {
            t[2][i] = (i as f32 - 33.5) / 40.0;
        }
        for i in 0..25 {
            t[2][i + 7] = (i as f32 - 13.0) / 20.0;
        }
        for i in 0..32 {
            t[3][i] = -t[2][31 - i];
        }
        for i in 0..16 {
            t[4][i] = i as f32 * 0.22 / 3.0 - 0.6;
            t[5][i] = i as f32 * 0.20 / 3.0 - 0.3;
        }
        for i in 0..8 {
            t[6][i] = i as f32 * 0.36 / 3.0 - 0.4;
            t[7][i] = i as f32 * 0.34 / 3.0 - 0.2;
        }
        FastAudio { table: t, channels: vec![([0.0; 8], 0.0); channels] }
    }

    fn decode(&mut self, data: &[u8], pcm: &mut Vec<f32>) {
        let nch = self.channels.len();
        for block in data.chunks_exact(40 * nch) {
            let start = pcm.len();
            pcm.resize(start + 256 * nch, 0.0);
            for (c, words) in block.chunks_exact(40).enumerate() {
                let src: [u32; 10] = std::array::from_fn(|i| le32(words, i * 4));
                let mut pos = 0i32;
                let mut read = |bits: u32| -> i32 {
                    if bits == 0 {
                        return 0;
                    }
                    pos += bits as i32;
                    let value = src[((pos - 1) / 32) as usize] >> ((-pos) & 31);
                    (value & ((1u32 << bits) - 1)) as i32
                };
                let mut m = [0f32; 8];
                for i in 0..8 {
                    m[7 - i] = self.table[i][read(FAST_BITS[i]) as usize];
                }
                let mut inds = [0i32; 4];
                let mut pads = [0i32; 4];
                for i in 0..4 {
                    inds[3 - i] = read(6);
                }
                for i in 0..4 {
                    pads[3 - i] = read(2);
                }
                let mut result = [0f32; 256];
                let mut index5 = 0i32;
                for i in 0..4 {
                    // av_int2float((n + 1) << 20) * 2^116
                    let value = f32::from_bits(((inds[i] + 1) as u32) << 20) * 2f32.powi(116);
                    let mut tmp = 0;
                    for j in 0..21 {
                        let out = if j == 20 { tmp as f32 - 7.0 } else { 2.0 * read(3) as f32 - 7.0 };
                        result[i * 64 + pads[i] as usize + j * 3] = value * out;
                        if j % 10 == 9 {
                            tmp = 4 * tmp + read(2);
                        }
                        if j == 20 {
                            index5 = (2 * index5 + tmp % 2).min(63);
                        }
                    }
                    m[2] = self.table[5][index5 as usize];
                }
                let (f, last) = &mut self.channels[c];
                for (i, &sample) in result.iter().enumerate() {
                    let mut x = sample;
                    for j in 0..8 {
                        x -= m[j] * f[j];
                        f[j] += m[j] * x;
                    }
                    f.copy_within(1.., 0);
                    f[7] = x;
                    *last = x + *last * 0.86;
                    pcm[start + i * nch + c] = *last * 2.0;
                }
            }
        }
    }
}

enum Audio {
    None,
    Sx(Vec<AudioChannel>),
    Fast(FastAudio),
    Pcm,
}

pub struct Mods {
    bytes: Arc<[u8]>,
    frames: usize,
    first: usize,
    end: usize,
    codec: u16,
    nch: usize,
    sx_at: usize,
    decoder: Mobiclip,
    audio: Audio,
    pos: usize,
    index: usize,
}

impl Mods {
    pub fn open(bytes: Arc<[u8]>) -> Result<(Mods, Info), String> {
        if bytes.len() < 0x30 || &bytes[..4] != b"MODS" {
            return Err("MODS : en-tete".into());
        }
        let u = |at: usize| le32(&bytes, at);
        let (frames, w, h, fps) = (u(8) as usize, u(0xC) as i32, u(0x10) as i32, u(0x14));
        if w <= 0 || h <= 0 || w % 16 != 0 || h % 16 != 0 || w > 2048 || h > 2048 {
            return Err(format!("MODS : taille {w}x{h}"));
        }
        let (codec, nch, rate) = (le16(&bytes, 0x18), le16(&bytes, 0x1A) as usize, u(0x1C));
        let (info_at, table_at, table_count) = (u(0x24) as usize, u(0x28) as usize, u(0x2C));
        let first = if table_count > 0 && table_at + 8 <= bytes.len() { le32(&bytes, table_at + 4) as usize } else { 0x30 };
        let end = if info_at > table_at { info_at.min(bytes.len()) } else { bytes.len() };
        // 3 (IMA) n'existe dans aucun jeu connu : l'image seule
        let sound = match codec {
            1 => bytes.len() >= info_at + nch * AUDIO_EXTRA,
            2 | 4 => true,
            _ => false,
        } && (1..=2).contains(&nch)
            && rate > 0;
        let info = Info { format: "MODS", width: w as u32, height: h as u32, frames: frames as u32, fps: fps as f32 / 16_777_216.0, sample_rate: if sound { rate } else { 0 }, channels: if sound { nch as u16 } else { 0 } };
        let mut mods = Mods { bytes, frames, first, end, codec: if sound { codec } else { 0 }, nch, sx_at: info_at, decoder: Mobiclip::new(w, h), audio: Audio::None, pos: first, index: 0 };
        mods.rewind();
        Ok((mods, info))
    }

    fn audio_from(&mut self, chunk_start: usize, size: usize, consumed: usize, blocks: usize, pcm: &mut Vec<f32>) -> R {
        let chunk = &self.bytes[chunk_start..chunk_start + size];
        let key = size >= 2 && chunk[1] & 0x80 != 0;
        match &mut self.audio {
            Audio::None => Ok(()),
            Audio::Sx(channels) => {
                let start = consumed + if key { 4 } else { 0 };
                if blocks == 0 || start > size {
                    return Ok(());
                }
                let mut gb = Msb::from_words(&chunk[start..]);
                read_aframes(&mut gb, channels, blocks, pcm)
            }
            Audio::Fast(fast) => {
                let start = consumed + if key && blocks > 0 { 4 } else { 0 };
                let len = blocks * self.nch * 40;
                if len > 0 && start + len <= size {
                    fast.decode(&chunk[start..start + len], pcm);
                    Ok(())
                } else if len > 0 {
                    Err("son : hors du paquet")
                } else {
                    Ok(())
                }
            }
            Audio::Pcm => {
                let len = blocks * self.nch * 512;
                if len > size {
                    return Err("son : hors du paquet");
                }
                for block in chunk[size - len..].chunks_exact(self.nch * 512) {
                    let start = pcm.len();
                    pcm.resize(start + 256 * self.nch, 0.0);
                    for c in 0..self.nch {
                        for s in 0..256 {
                            let at = (c * 256 + s) * 2;
                            pcm[start + s * self.nch + c] = i16::from_le_bytes([block[at], block[at + 1]]) as f32 / 32768.0;
                        }
                    }
                }
                Ok(())
            }
        }
    }

    /// Le silence qui remplace le son d'une image illisible.
    fn silence(&self, blocks: usize, pcm: &mut Vec<f32>) {
        let samples = match self.audio {
            Audio::None => 0,
            Audio::Sx(_) => 128,
            Audio::Fast(_) | Audio::Pcm => 256,
        };
        pcm.resize(pcm.len() + blocks * samples * self.nch, 0.0);
    }
}

impl Stream for Mods {
    fn yuv(&self, out: &mut Vec<u8>) {
        out.clear();
        self.decoder.last().iter().for_each(|plane| out.extend_from_slice(plane));
    }

    fn rewind(&mut self) {
        self.audio = match self.codec {
            1 => Audio::Sx((0..self.nch).map(|c| AudioChannel::new(&self.bytes[self.sx_at + c * AUDIO_EXTRA..])).collect()),
            2 => Audio::Fast(FastAudio::new(self.nch)),
            4 => Audio::Pcm,
            _ => Audio::None,
        };
        self.decoder = Mobiclip::new(self.decoder.w, self.decoder.h);
        self.pos = self.first;
        self.index = 0;
    }

    fn next(&mut self, pcm: &mut Vec<f32>) -> Option<R> {
        if self.index >= self.frames || self.pos + 4 > self.end {
            return None;
        }
        self.index += 1;
        let word = le32(&self.bytes, self.pos);
        let (size, blocks) = ((word >> 14) as usize, (word & 0x3FFF) as usize);
        let start = self.pos + 4;
        if start + size > self.bytes.len() {
            self.index = self.frames;
            return Some(Err("taille d'image"));
        }
        self.pos = start + size;
        let bytes = self.bytes.clone();
        match self.decoder.decode(&bytes[start..start + size]) {
            Ok(consumed) => {
                let before = pcm.len();
                let result = self.audio_from(start, size, consumed, blocks, pcm);
                if result.is_err() {
                    pcm.truncate(before);
                    self.silence(blocks, pcm);
                }
                Some(result)
            }
            Err(e) => {
                self.silence(blocks, pcm);
                Some(Err(e))
            }
        }
    }

    fn rgba(&self, out: &mut Vec<u8>) {
        let (w, h) = (self.decoder.w as usize, self.decoder.h as usize);
        let pic = self.decoder.last();
        out.clear();
        out.resize(w * h * 4, 255);
        if pic[0].is_empty() {
            return;
        }
        for y in 0..h {
            for x in 0..w {
                let luma = pic[0][y * w + x] as i32;
                let at = (y / 2) * (w / 2) + x / 2;
                let (cg, co) = (pic[1][at] as i32 - 128, pic[2][at] as i32 - 128);
                let t = luma - cg;
                let px = &mut out[(y * w + x) * 4..][..3];
                px[0] = clip8(t + co);
                px[1] = clip8(luma + cg);
                px[2] = clip8(t - co);
            }
        }
    }
}
