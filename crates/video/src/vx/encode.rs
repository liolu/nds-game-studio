//! L'écriture d'une vidéo VX : chaque image relue par le décodeur d'à côté, avec ses propres prédictions.
//!
//! Le fichier garde l'en-tête de l'original (taille, cadence, quantificateur, tables du son), ce que le jeu attend. Chaque image reste
//! sous la taille maximale annoncée par l'original : le jeu a pu réserver sa mémoire d'après elle.

use super::*;

/// Trames de son gardées d'une vidéo : leur nombre par canal, et leurs octets.
struct Packet {
    aframes: u16,
    audio: Vec<u8>,
}

/// Écrit du poids fort au plus faible, puis en mots de 16 bits petit-boutistes (l'inverse de `Msb`).
#[derive(Default)]
pub(crate) struct BitWriter {
    bytes: Vec<u8>,
    current: u32,
    used: u32,
}

impl BitWriter {
    pub(crate) fn put(&mut self, count: u32, value: u32) {
        for shift in (0..count).rev() {
            self.current = (self.current << 1) | ((value >> shift) & 1);
            self.used += 1;
            if self.used == 16 {
                self.bytes.extend_from_slice(&(self.current as u16).to_le_bytes());
                self.current = 0;
                self.used = 0;
            }
        }
    }

    fn ue(&mut self, value: u32) {
        let code = value as u64 + 1;
        let zeros = 63 - code.leading_zeros();
        self.put(zeros, 0);
        self.put(1, 1);
        self.put(zeros, (code & ((1u64 << zeros) - 1)) as u32);
    }

    fn se(&mut self, value: i32) {
        self.ue(if value > 0 { value as u32 * 2 - 1 } else { value.unsigned_abs() * 2 });
    }

    pub(crate) fn finish(mut self) -> Vec<u8> {
        if self.used > 0 {
            self.put(16 - self.used, 0);
        }
        self.bytes
    }
}

fn ue_bits(value: u32) -> i32 {
    2 * (31 - (value + 1).leading_zeros()) as i32 + 1
}

fn se_bits(value: i32) -> i32 {
    ue_bits(if value > 0 { value as u32 * 2 - 1 } else { value.unsigned_abs() * 2 })
}

/// Les valeurs de H.264 pour la DCT directe, dans l'ordre de `VX_QUANT` : deux indices pairs, un impair, deux impairs.
const FORWARD: [[i64; 3]; 6] = [[13107, 8066, 5243], [11916, 7490, 4660], [10082, 6554, 4194], [9362, 5825, 3647], [8192, 5243, 3355], [7282, 4559, 2893]];

/// Plus le niveau monte, moins une image coûte : arrondi vers zéro, puis coefficients aigus coupés.
const LEVELS: [(i64, usize); 7] = [(3, 16), (4, 16), (6, 10), (8, 6), (10, 3), (12, 1), (12, 0)];

#[derive(Clone, Copy)]
enum Choice {
    Inter(Mv),
    /// Mode de luminance, mode de chrominance.
    Whole(u32, u32),
    MbPlane([i32; 3]),
    Tiles([i32; 16], u32),
}

/// Le décodeur du jeu range un vecteur sur 8 bits en x.
const MOTION_LIMIT: i32 = 100;

struct Settings {
    w: i32,
    h: i32,
    quantizer: i32,
    qtab: [u16; 3],
}

fn rgba_to_pic(rgba: &[u8], w: usize, h: usize) -> Pic {
    let mut pic: Pic = [vec![0; w * h], vec![0; w / 2 * (h / 2)], vec![0; w / 2 * (h / 2)]];
    let luma = |px: &[u8]| ((2 * px[0] as i32 + 4 * px[1] as i32 + px[2] as i32 + 3) / 7).clamp(0, 255);
    for (at, px) in rgba.chunks_exact(4).enumerate().take(w * h) {
        pic[0][at] = luma(px) as u8;
    }
    for y in 0..h / 2 {
        for x in 0..w / 2 {
            let (mut u, mut v) = (0, 0);
            for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                let px = &rgba[((y * 2 + dy) * w + x * 2 + dx) * 4..][..4];
                let l = luma(px);
                u += px[2] as i32 - l;
                v += px[0] as i32 - l;
            }
            // Quatre pixels, et un écart divisé par deux au décodage
            pic[1][y * (w / 2) + x] = (128 + (u + 4 * u.signum()) / 8).clamp(0, 255) as u8;
            pic[2][y * (w / 2) + x] = (128 + (v + 4 * v.signum()) / 8).clamp(0, 255) as u8;
        }
    }
    pic
}

/// La DCT entière de H.264, rangée comme `idct_add` : `4 * fréquence en x + fréquence en y`.
fn forward_dct(residue: &[i32; 16]) -> [i32; 16] {
    let pass = |a: i32, b: i32, c: i32, d: i32| [a + b + c + d, 2 * a + b - c - 2 * d, a - b - c + d, a - 2 * b + 2 * c - d];
    let mut mid = [0; 16];
    for x in 0..4 {
        let line = pass(residue[x * 4], residue[x * 4 + 1], residue[x * 4 + 2], residue[x * 4 + 3]);
        for (fy, value) in line.into_iter().enumerate() {
            mid[x * 4 + fy] = value;
        }
    }
    let mut out = [0; 16];
    for fy in 0..4 {
        let line = pass(mid[fy], mid[4 + fy], mid[8 + fy], mid[12 + fy]);
        for (fx, value) in line.into_iter().enumerate() {
            out[fx * 4 + fy] = value;
        }
    }
    out
}

fn quantize(settings: &Settings, residue: &[i32; 16], level: usize) -> [i32; 16] {
    let (divide, cut) = LEVELS[level];
    let dct = forward_dct(residue);
    let forward = &FORWARD[(settings.quantizer % 6) as usize];
    let shift = 15 + (settings.quantizer / 6) as u32;
    let round = (1i64 << shift) / divide;
    let mut out = [0; 16];
    for i in 0..cut {
        let at = VX_ZIGZAG[i] as usize;
        let value = (dct[at].unsigned_abs() as i64 * forward[(at & 1) + ((at >> 2) & 1)] + round) >> shift;
        // Au-delà, le code d'échappement du niveau ne suffit plus
        out[i] = (value.min(2000) as i32) * dct[at].signum();
    }
    // Un seul petit coefficient aigu coûte plus de bits qu'il ne rend d'image
    if level > 0 {
        let total = out.iter().filter(|v| **v != 0).count();
        if total == 1 && out[0] == 0 && out.iter().all(|v| v.abs() <= 1) {
            out = [0; 16];
        }
    }
    out
}

fn write_cavlc(out: &mut BitWriter, levels: &[i32; 16], nc: i32) {
    let table = match nc {
        0..=1 => 0,
        2..=3 => 1,
        4..=7 => 2,
        _ => 3,
    };
    // Du dernier coefficient au premier, comme la lecture
    let filled: Vec<usize> = (0..16).rev().filter(|&i| levels[i] != 0).collect();
    let total = filled.len();
    let trailing = filled.iter().take(3).take_while(|&&i| levels[i].abs() == 1).count();
    let token = total << 2 | trailing;
    out.put(VX_COEFF_TOKEN_LEN[table][token] as u32, VX_COEFF_TOKEN_BITS[table][token] as u32);
    if total == 0 {
        return;
    }
    let mut zeros = filled[0] + 1 - total;
    if total < 16 {
        out.put(VX_TOTAL_ZEROS_LEN[total - 1][zeros] as u32, VX_TOTAL_ZEROS_BITS[total - 1][zeros] as u32);
    }
    let limit = [0, 3, 6, 12, 24, 48, 0x8000];
    let mut suffix = 0u32;
    for (n, &at) in filled.iter().enumerate() {
        let level = levels[at];
        if n < trailing {
            out.put(1, u32::from(level < 0));
        } else {
            let size = level.unsigned_abs() - 1;
            let prefix = (size >> suffix).min(15);
            out.put(prefix, 0);
            out.put(1, 1);
            if prefix == 15 {
                out.put(11, size - (15 << suffix));
            } else {
                out.put(suffix, size & ((1 << suffix) - 1));
            }
            if (suffix as usize) + 1 < limit.len() && level.abs() > limit[suffix as usize + 1] {
                suffix += 1;
            }
            out.put(1, u32::from(level < 0));
        }
        if let Some(&next) = filled.get(n + 1) {
            if zeros != 0 {
                let run = at - next - 1;
                if zeros < 7 {
                    out.put(VX_RUN_LEN[zeros - 1][run] as u32, VX_RUN_BITS[zeros - 1][run] as u32);
                } else {
                    out.put(VX_RUN7_LEN[run] as u32, VX_RUN7_BITS[run] as u32);
                }
                zeros -= run;
            }
        }
    }
}

impl Frame<'_> {
    fn clear_block(&mut self, b: Block) {
        for plane in 0..3 {
            let step = if plane > 0 { 2 } else { 1 };
            for j in (0..b.h).step_by(step) {
                for i in (0..b.w).step_by(step) {
                    self.set(plane, b.x + i, b.y + j, 0);
                }
            }
        }
    }

    fn difference(&self, source: &Pic, plane: usize, b: Block) -> i32 {
        let step = if plane > 0 { 2 } else { 1 };
        let mut sum = 0;
        for j in (0..b.h).step_by(step) {
            for i in (0..b.w).step_by(step) {
                sum += (self.get(plane, b.x + i, b.y + j) - px_get(self.w, self.h, source, plane, b.x + i, b.y + j)).abs();
            }
        }
        sum
    }

    fn colors_difference(&self, source: &Pic, b: Block) -> i32 {
        self.difference(source, 1, b) + self.difference(source, 2, b)
    }

    fn apply_colors(&mut self, b: Block, mode: u32) {
        for plane in 1..3 {
            match mode {
                0 => self.predict_dc(b, plane),
                1 => self.predict_horizontal(b, plane),
                2 => self.predict_vertical(b, plane),
                _ => self.predict_plane(b, plane, 0),
            }
        }
    }

    fn apply_luma(&mut self, b: Block, mode: u32) {
        match mode {
            0 => self.predict_vertical(b, 0),
            1 => self.predict_horizontal(b, 0),
            2 => self.predict_dc(b, 0),
            _ => self.predict_plane(b, 0, 0),
        }
    }

    fn apply(&mut self, b: Block, choice: Choice) {
        self.clear_block(b);
        // Un essai ne doit rien laisser : seul un bloc qui garde son vecteur l'écrit, à la lecture aussi
        let at = ((b.y / 16 + 1) * self.vectors_stride + b.x / 16 + 1) as usize;
        self.vectors[at] = Mv::default();
        match choice {
            Choice::Inter(vec) => {
                let source = self.refs[0].expect("reference");
                self.copy_block(b, vec, source, [0; 3]);
                self.vectors[at] = vec;
            }
            Choice::Whole(luma, colors) => {
                self.apply_luma(b, luma);
                self.apply_colors(b, colors);
            }
            Choice::MbPlane(params) => {
                for (plane, param) in params.into_iter().enumerate() {
                    self.predict_plane(b, plane, param * 2);
                }
            }
            Choice::Tiles(modes, colors) => {
                for (at, &mode) in modes.iter().enumerate() {
                    let _ = self.pred4x4(mode, b.x + (at as i32 % 4) * 4, b.y + (at as i32 / 4) * 4);
                }
                self.apply_colors(b, colors);
            }
        }
    }

    fn choice_cost(&mut self, source: &Pic, b: Block, choice: Choice, bits: i32, lambda: i32) -> i32 {
        self.apply(b, choice);
        self.difference(source, 0, b) + self.colors_difference(source, b) + bits * lambda
    }

    /// Le vecteur qui colle le mieux à l'image précédente, en ne regardant que la luminance.
    fn search_motion(&mut self, source: &Pic, b: Block, starts: &[Mv], pred: Mv, lambda: i32) -> Mv {
        let reference = self.refs[0].expect("reference");
        let (w, h) = (self.w, self.h);
        let cost = |v: Mv| {
            let mut sum = (se_bits(v.x - pred.x) + se_bits(v.y - pred.y)) * lambda;
            for j in 0..b.h {
                for i in 0..b.w {
                    sum += (px_get(w, h, source, 0, b.x + i, b.y + j) - px_get(w, h, reference, 0, b.x + i + v.x, b.y + j + v.y)).abs();
                }
            }
            sum
        };
        let mut best = Mv::default();
        let mut best_cost = cost(best);
        let usable = |frame: &Self, v: Mv| v.x.abs() <= MOTION_LIMIT && v.y.abs() <= MOTION_LIMIT && frame.mv_in_bounds(b, v);
        for &start in starts {
            if usable(self, start) {
                let c = cost(start);
                if c < best_cost {
                    (best, best_cost) = (start, c);
                }
            }
        }
        let mut step = 8;
        while step > 0 {
            let mut moved = false;
            for (dx, dy) in [(step, 0), (-step, 0), (0, step), (0, -step)] {
                let v = Mv { x: best.x + dx, y: best.y + dy };
                if usable(self, v) {
                    let c = cost(v);
                    if c < best_cost {
                        (best, best_cost) = (v, c);
                        moved = true;
                    }
                }
            }
            if !moved {
                step /= 2;
            }
        }
        best
    }

    /// Le meilleur mode de chaque bloc 4x4, chacun prédit depuis la prédiction de ses voisins (le résidu vient après, dans ce format).
    fn best_tiles(&mut self, source: &Pic, b: Block, lambda: i32) -> ([i32; 16], i32) {
        self.clear_block(b);
        let mut cache = [[9i32; 5]; 5];
        let mut modes = [2i32; 16];
        let mut bits = 0;
        for y2 in 0..4usize {
            for x2 in 0..4usize {
                let likely = match cache[y2][1 + x2].min(cache[1 + y2][x2]) {
                    9 => 2,
                    mode => mode,
                };
                let tile = Block { x: b.x + x2 as i32 * 4, y: b.y + y2 as i32 * 4, w: 4, h: 4 };
                let mut best = (i32::MAX, likely);
                for mode in 0..9 {
                    if !self.can_tile(mode, tile, x2, y2) {
                        continue;
                    }
                    let _ = self.pred4x4(mode, tile.x, tile.y);
                    let cost = self.difference(source, 0, tile) + if mode == likely { lambda } else { 4 * lambda };
                    if cost < best.0 {
                        best = (cost, mode);
                    }
                }
                let _ = self.pred4x4(best.1, tile.x, tile.y);
                modes[y2 * 4 + x2] = best.1;
                cache[1 + y2][1 + x2] = best.1;
                bits += if best.1 == likely { 1 } else { 4 };
            }
        }
        (modes, bits)
    }

    /// Le code du jeu lit ce qui se trouve en mémoire hors de l'image ou dans un bloc pas encore décodé : l'ancienne image, pas nos zéros.
    /// Il ne faut donc jamais prédire depuis là. Les vidéos d'origine ne le font pas non plus.
    fn can_tile(&self, mode: i32, tile: Block, x2: usize, y2: usize) -> bool {
        let (top, left) = (tile.y > 0, tile.x > 0);
        match mode {
            0 => top,
            1 | 8 => left,
            2 => true,
            3 | 7 => top && (y2 == 0 || x2 < 3) && tile.x + 8 <= self.w,
            _ => top && left,
        }
    }

    fn plane_params(&self, source: &Pic, b: Block) -> [i32; 3] {
        std::array::from_fn(|plane| {
            let bl = self.get(plane, b.x - 1, b.y + b.h - 1);
            let tr = self.get(plane, b.x + b.w - 1, b.y - 1);
            let wanted = px_get(self.w, self.h, source, plane, b.x + b.w - 1, b.y + b.h - 1);
            (wanted - (bl + tr + 1) / 2) / 2
        })
    }

    fn choose(&mut self, source: &Pic, b: Block, pred: Mv, lambda: i32) -> Choice {
        let mut best: Option<(i32, Choice)> = None;
        let mut consider = |frame: &mut Self, choice: Choice, bits: i32| {
            let cost = frame.choice_cost(source, b, choice, bits, lambda);
            if best.is_none_or(|(c, _)| cost < c) {
                best = Some((cost, choice));
            }
            cost
        };
        let mut inter_cost = i32::MAX;
        if self.refs[0].is_some() {
            let at = |row: i32, col: i32| self.vectors[(row * self.vectors_stride + col) as usize];
            let (by, bx) = (b.y / 16, b.x / 16);
            let starts = [pred, at(by + 1, bx), at(by, bx + 1)];
            let vec = self.search_motion(source, b, &starts, pred, lambda);
            let bits = ue_bits(4) + se_bits(vec.x - pred.x) + se_bits(vec.y - pred.y);
            inter_cost = consider(self, Choice::Inter(vec), bits);
            if pred.x.abs() <= MOTION_LIMIT && pred.y.abs() <= MOTION_LIMIT && self.mv_in_bounds(b, pred) {
                inter_cost = inter_cost.min(consider(self, Choice::Inter(pred), ue_bits(1)));
            }
        }
        // Une image qui ne bouge pas : inutile de chercher mieux
        if inter_cost > b.w * b.h {
            let (top, left) = (b.y > 0, b.x > 0);
            let can_luma = |mode: u32| match mode {
                0 => top,
                1 => left,
                2 => true,
                _ => top && left,
            };
            let can_colors = |mode: u32| match mode {
                0 => true,
                1 => left,
                2 => top,
                _ => top && left,
            };
            let luma = (0..4u32).filter(|m| can_luma(*m)).min_by_key(|&mode| {
                self.clear_block(b);
                self.apply_luma(b, mode);
                self.difference(source, 0, b)
            });
            let colors = (0..4u32).filter(|m| can_colors(*m)).min_by_key(|&mode| {
                self.clear_block(b);
                self.apply_colors(b, mode);
                self.colors_difference(source, b)
            });
            let (luma, colors) = (luma.unwrap_or(2), colors.unwrap_or(0));
            consider(self, Choice::Whole(luma, colors), ue_bits(11) + ue_bits(luma) + ue_bits(colors));
            if top && left {
                self.clear_block(b);
                let params = self.plane_params(source, b);
                consider(self, Choice::MbPlane(params), ue_bits(7) + params.iter().map(|p| se_bits(*p)).sum::<i32>());
            }
            let (modes, bits) = self.best_tiles(source, b, lambda);
            consider(self, Choice::Tiles(modes, colors), ue_bits(15) + bits + ue_bits(colors));
        }
        best.map_or(Choice::Whole(2, 0), |(_, choice)| choice)
    }

    /// Le résidu de tout le macrobloc, sans rien écrire ni changer : `[[y0, y1, y2, y3, u, v] ; un par bloc 8x8]`.
    fn residue_levels(&self, settings: &Settings, source: &Pic, b: Block, level: usize) -> Vec<[[i32; 16]; 6]> {
        let block = |plane: usize, x: i32, y: i32| {
            let step = if plane > 0 { 2 } else { 1 };
            let mut residue = [0; 16];
            for i in 0..4 {
                for k in 0..4 {
                    let (px, py) = (x + i * step, y + k * step);
                    residue[(i * 4 + k) as usize] = px_get(self.w, self.h, source, plane, px, py) - self.get(plane, px, py);
                }
            }
            quantize(settings, &residue, level)
        };
        let mut out = Vec::new();
        for y in (b.y..b.y + b.h).step_by(8) {
            for x in (b.x..b.x + b.w).step_by(8) {
                out.push([block(0, x, y), block(0, x + 4, y), block(0, x, y + 4), block(0, x + 4, y + 4), block(1, x, y), block(2, x, y)]);
            }
        }
        out
    }

    fn write_residue(&mut self, out: &mut BitWriter, b: Block, levels: &[[[i32; 16]; 6]]) {
        let count = |levels: &[i32; 16]| levels.iter().filter(|v| **v != 0).count() as i32;
        let mut index = 0;
        for y in (b.y..b.y + b.h).step_by(8) {
            for x in (b.x..b.x + b.w).step_by(8) {
                let parts = &levels[index];
                index += 1;
                let mut mask = 0u8;
                for (bit, part) in parts[..4].iter().enumerate() {
                    if count(part) > 0 {
                        mask |= 1 << bit;
                    }
                }
                if count(&parts[4]) + count(&parts[5]) > 0 {
                    mask |= 16;
                }
                out.ue(VX_RESIDU_MASK.iter().position(|m| *m == mask).unwrap_or(0) as u32);
                for (bit, (dx, dy)) in [(0, 0), (4, 0), (0, 4), (4, 4)].into_iter().enumerate() {
                    let (px, py) = (x + dx, y + dy);
                    let mut total = 0;
                    if mask & (1 << bit) != 0 {
                        let nc = (self.coeff_y_get(px - 1, py) + self.coeff_y_get(px, py - 1) + 1) / 2;
                        write_cavlc(out, &parts[bit], nc);
                        self.idct_add(px, py, 0, &parts[bit]);
                        total = count(&parts[bit]);
                    }
                    self.coeff_y_set(px, py, total);
                }
                let mut total = 0;
                if mask & 16 != 0 {
                    let nc = (self.coeff_uv_get(x - 1, y) + self.coeff_uv_get(x, y - 1) + 1) / 2;
                    write_cavlc(out, &parts[4], nc);
                    write_cavlc(out, &parts[5], nc);
                    self.idct_add(x, y, 1, &parts[4]);
                    self.idct_add(x, y, 2, &parts[5]);
                    total = (count(&parts[4]) + count(&parts[5]) + 1) / 2;
                }
                self.coeff_uv_set(x, y, total);
            }
        }
    }

    fn write_macroblock(&mut self, out: &mut BitWriter, settings: &Settings, source: &Pic, b: Block, pred: Mv, level: usize, lambda: i32) {
        let choice = self.choose(source, b, pred, lambda);
        self.apply(b, choice);
        let levels = self.residue_levels(settings, source, b, level);
        let residue = levels.iter().flatten().flatten().any(|v| *v != 0);
        let mode = |plain: u32, with: u32| if residue { with } else { plain };
        match choice {
            Choice::Inter(vec) if vec.x == pred.x && vec.y == pred.y => out.ue(mode(1, 12)),
            Choice::Inter(vec) => {
                out.ue(mode(4, 16));
                out.se(vec.x - pred.x);
                out.se(vec.y - pred.y);
            }
            Choice::Whole(luma, colors) => {
                out.ue(mode(11, 22));
                out.ue(luma);
                out.ue(colors);
            }
            Choice::MbPlane(params) => {
                out.ue(mode(7, 23));
                params.iter().for_each(|p| out.se(*p));
            }
            Choice::Tiles(modes, colors) => {
                out.ue(mode(15, 19));
                let mut cache = [[9i32; 5]; 5];
                for y2 in 0..4usize {
                    for x2 in 0..4usize {
                        let likely = match cache[y2][1 + x2].min(cache[1 + y2][x2]) {
                            9 => 2,
                            value => value,
                        };
                        let value = modes[y2 * 4 + x2];
                        cache[1 + y2][1 + x2] = value;
                        if value == likely {
                            out.put(1, 1);
                        } else {
                            out.put(1, 0);
                            out.put(3, (value - i32::from(value > likely)) as u32);
                        }
                    }
                }
                out.ue(colors);
            }
        }
        if residue {
            self.write_residue(out, b, &levels);
        } else {
            self.clear_coeffs(b);
        }
    }
}

/// Une image : son résultat tel que le lira le jeu, et ses bits.
fn encode_picture(settings: &Settings, source: &Pic, previous: Option<&Pic>, level: usize) -> (Pic, Vec<u8>) {
    let (w, h) = (settings.w as usize, settings.h as usize);
    let mut dst: Pic = [vec![0; w * h], vec![0; w / 2 * (h / 2)], vec![0; w / 2 * (h / 2)]];
    let mut unused = Msb::from_words(&[]);
    let (coeff_y_stride, coeff_uv_stride, vectors_stride) = ((w / 4 + 1) as i32, (w / 8 + 1) as i32, (w / 16 + 2) as i32);
    let mut frame = Frame {
        gb: &mut unused,
        w: settings.w,
        h: settings.h,
        qtab: settings.qtab,
        dst: &mut dst,
        refs: [previous, None, None],
        coeff_y: vec![0; (h / 4 + 1) * coeff_y_stride as usize],
        coeff_y_stride,
        coeff_uv: vec![0; (h / 8 + 1) * coeff_uv_stride as usize],
        coeff_uv_stride,
        vectors: vec![Mv::default(); (h / 16 + 1) * vectors_stride as usize],
        vectors_stride,
    };
    let lambda = 1 << (settings.quantizer / 6).clamp(1, 5) >> 1;
    let mut out = BitWriter::default();
    for y in (0..settings.h).step_by(16) {
        for x in (0..settings.w).step_by(16) {
            let (by, bx) = (y / 16, x / 16);
            let vec = |f: &Frame, row: i32, col: i32| f.vectors[(row * f.vectors_stride + col) as usize];
            let (left, above, right) = (vec(&frame, by + 1, bx), vec(&frame, by, bx + 1), vec(&frame, by, bx + 2));
            let pred = Mv { x: median(left.x, above.x, right.x), y: median(left.y, above.y, right.y) };
            frame.write_macroblock(&mut out, settings, source, Block { x, y, w: 16, h: 16 }, pred, level, lambda);
        }
    }
    let bytes = out.finish();
    (dst, bytes)
}

// --------------------------------------------------------------------------------------------
//  Le son : LPC et impulsions régulières, sans la prédiction à long terme
// --------------------------------------------------------------------------------------------

/// L'inverse de `AudioChannel::decode`, canal par canal : le même décodeur garde l'état à jour.
struct SoundEncoder {
    channel: AudioChannel,
}

/// Les huit coefficients de réflexion (Q15) d'un morceau de son.
fn reflections(window: &[f32]) -> [i32; 8] {
    let n = window.len();
    let shaped: Vec<f32> = window.iter().enumerate().map(|(i, s)| s * (0.5 - 0.5 * (std::f32::consts::TAU * (i as f32 + 0.5) / n as f32).cos())).collect();
    let mut r = [0f32; 9];
    for (lag, value) in r.iter_mut().enumerate() {
        *value = shaped[lag..].iter().zip(&shaped).map(|(a, b)| a * b).sum();
    }
    let mut out = [0; 8];
    if r[0] < 1.0 {
        return out;
    }
    r[0] *= 1.0005;
    let mut a = [0f32; 9];
    a[0] = 1.0;
    let mut error = r[0];
    for i in 1..=8 {
        let sum: f32 = (0..i).map(|j| a[j] * r[i - j]).sum();
        let k = (-sum / error).clamp(-0.97, 0.97);
        let old = a;
        for j in 1..=i {
            a[j] = old[j] + k * old[i - j];
        }
        error *= 1.0 - k * k;
        out[i - 1] = (k * 32768.0) as i32;
    }
    out
}

impl SoundEncoder {
    /// `None` si le produit dépasse 32 bits : le code du jeu calcule sur 32 bits et son filtre partirait ailleurs.
    fn influence_of(filter: &[i32; 8]) -> Option<[i32; 8]> {
        let mut psi = [0i32; 8];
        for i in 0..8 {
            let old = psi;
            for j in 0..i {
                let product = i32::try_from(old[i - j - 1] as i64 * filter[i] as i64).ok()?;
                psi[j] = old[j].checked_add(product >> 15)?;
            }
            psi[i] = filter[i];
        }
        Some(std::array::from_fn(|i| -(psi[i] >> 1)))
    }

    /// Le filtre atteignable le plus proche : la somme de `start` et d'une entrée de chacun des trois livres.
    fn nearest_filter(&self, start: [i32; 8], wanted: &[i32; 8]) -> ([i32; 8], [usize; 3]) {
        let mut filter = start;
        let mut chosen = [0; 3];
        for book in 0..3 {
            let distance = |index: usize| -> i64 {
                (0..8).map(|k| {
                    let gap = (filter[k] + self.channel.codebooks[book * 64 + index][k] as i32 - wanted[k]) as i64;
                    gap * gap
                }).sum()
            };
            chosen[book] = (0..64).min_by_key(|&i| distance(i)).unwrap_or(0);
            for (k, value) in filter.iter_mut().enumerate() {
                *value += self.channel.codebooks[book * 64 + chosen[book]][k] as i32;
            }
        }
        (filter, chosen)
    }

    /// La suite d'impulsions pour cette voix : (erreur, valeurs de 0 à 7). Chaque impulsion est choisie sur les échantillons qu'elle
    /// gouverne seule, jusqu'à la suivante. Un filtre qui s'emballe rend `i64::MAX` : le décodeur refuserait ces valeurs.
    fn pulses_for(&self, target: &[i32; AFRAME], quarters: &[[i32; 8]; 4], reset: bool, scale: i32, start: usize) -> (i64, [u8; 42]) {
        let history = if reset { [0; 8] } else { self.channel.samples_prev };
        let mut samples = [0i32; AFRAME];
        let mut values = [0u8; 42];
        let mut error = 0i64;
        let predict = |samples: &[i32; AFRAME], i: usize| -> i64 {
            let weights = &quarters[i * 4 / AFRAME];
            let mut sum = 0i64;
            for (j, &weight) in weights.iter().enumerate() {
                let previous = if i > j { samples[i - 1 - j] } else { history[7 + i - j] };
                sum += previous as i64 * weight as i64;
            }
            sum >> 14
        };
        let mut i = 0;
        while i < AFRAME {
            let on_grid = i >= start && (i - start) % 3 == 0 && (i - start) / 3 < 42 && scale != 0;
            let mut value = 0;
            if on_grid {
                let mut best = i64::MAX;
                let end = (i + 3).min(AFRAME);
                for candidate in 0..8 {
                    let pulse = (candidate * 2 - 7) as i64 * scale as i64;
                    let mut gap_sum = 0i64;
                    for n in i..end {
                        let sample = predict(&samples, n) + if n == i { pulse } else { 0 };
                        samples[n] = sample.clamp(i32::MIN as i64 / 2, i32::MAX as i64 / 2) as i32;
                        let gap = target[n] as i64 - sample.clamp(-32768, 32767);
                        gap_sum += gap * gap;
                    }
                    if gap_sum < best {
                        best = gap_sum;
                        value = candidate;
                    }
                }
                values[(i - start) / 3] = value as u8;
            }
            let pulse = if on_grid { (value * 2 - 7) as i64 * scale as i64 } else { 0 };
            let sample = predict(&samples, i) + pulse;
            // Même remarque : sur 32 bits en Q14, un échantillon ne va pas au-delà de 2^17
            if sample.abs() >= 1 << 16 {
                return (i64::MAX, values);
            }
            samples[i] = sample as i32;
            let gap = target[i] as i64 - sample.clamp(-32768, 32767);
            error += gap * gap;
            i += 1;
        }
        (error, values)
    }

    /// Une trame de 128 échantillons. `around` : le son autour, pour l'analyse.
    fn frame(&mut self, target: &[i32; AFRAME], around: &[f32]) -> Result<Vec<u8>, &'static str> {
        let wanted = reflections(around);
        let mut best: Option<(i64, usize, usize, usize, [usize; 3], [u8; 42])> = None;
        for reset in [false, true] {
            if !reset && !self.channel.have_prev {
                continue;
            }
            let start_filter = if reset { self.channel.lpc_base } else { self.channel.filter_prev };
            let (filter, chosen) = self.nearest_filter(start_filter, &wanted);
            if filter.iter().any(|k| k.abs() >= 32700) {
                continue;
            }
            let Some(influence) = Self::influence_of(&filter) else { continue };
            let mut quarters = [influence; 4];
            if !reset {
                for j in 0..8 {
                    quarters[1][j] = (self.channel.influence_prev[j] + quarters[3][j]) >> 1;
                    quarters[0][j] = (self.channel.influence_prev[j] + quarters[1][j]) >> 1;
                    quarters[2][j] = (quarters[1][j] + quarters[3][j]) >> 1;
                }
            }
            let scale_before = if reset { self.channel.scale_initial as i32 } else { self.channel.scale_prev };
            for modifier in 0..8 {
                let scaled = (scale_before as i64 * self.channel.scale_modifiers[modifier] as i64) >> 13;
                // Le code du jeu multiplie sur 32 bits, et chaque impulsion vaut jusqu'à 7 fois l'échelle
                if scaled.abs() > 1 << 24 || (scale_before as i64 * self.channel.scale_modifiers[modifier] as i64).abs() >= 1 << 31 {
                    continue;
                }
                let scale = scaled as i32;
                for start in 0..3 {
                    let (mut error, values) = self.pulses_for(target, &quarters, reset, scale, start);
                    // Repartir de la base coupe les fondus entre trames : seulement si c'est nettement mieux
                    if reset && self.channel.have_prev {
                        error = error.saturating_add(error / 4);
                    }
                    if best.is_none_or(|b| error < b.0) {
                        best = Some((error, usize::from(reset), modifier, start, chosen, values));
                    }
                }
            }
        }
        // Rien de stable : le filtre le plus plat depuis la base, sans impulsion utile
        let best = best.unwrap_or_else(|| (0, 1, 0, 0, self.nearest_filter(self.channel.lpc_base, &[0; 8]).1, [3; 42]));
        let (_, reset, modifier, start, chosen, values) = best;
        let prev_offset: u32 = if reset == 1 { 0x7F } else { 0x7E };
        let word1 = prev_offset << 9 | (modifier as u32) << 6 | chosen[0] as u32;
        let word2 = (start as u32) << 14 | (chosen[1] as u32) << 6 | chosen[2] as u32;
        let mut out = BitWriter::default();
        out.put(16, word1);
        out.put(16, word2);
        for w in 0..8 {
            let mut word = 0u32;
            for v in 0..5 {
                word |= (values[w * 5 + v] as u32) << (13 - 3 * v);
            }
            let extra = |index: usize, shift: usize| ((values[index] as u32) >> shift) & 1;
            word |= match w {
                0 => extra(40, 2),
                1 => extra(40, 1),
                2 => extra(40, 0),
                3 => extra(41, 2),
                4 => extra(41, 1),
                5 => extra(41, 0),
                _ => 0,
            };
            out.put(16, word);
        }
        let bytes = out.finish();
        // Le vrai décodeur tient l'état : ce qu'il entend est ce que le jeu entendra
        let mut gb = Msb::from_words(&bytes);
        let mut heard = [0f32; AFRAME];
        self.channel.decode(&mut gb, &mut heard, 1)?;
        Ok(bytes)
    }
}

/// Le son nouveau, découpé selon les besoins des images. `samples` : entrelacé, à la fréquence et avec les voies de l'original.
pub(crate) struct SoundTrack<'a> {
    samples: &'a [i16],
    nch: usize,
    encoders: Vec<SoundEncoder>,
    at: usize,
}

impl<'a> SoundTrack<'a> {
    /// `tables` : les tables du son d'une vidéo, un bloc de `AUDIO_EXTRA` octets par voie.
    pub(crate) fn new(samples: &'a [i16], nch: usize, tables: &[u8]) -> SoundTrack<'a> {
        SoundTrack { samples, nch, encoders: (0..nch).map(|c| SoundEncoder { channel: AudioChannel::new(&tables[c * AUDIO_EXTRA..]) }).collect(), at: 0 }
    }

    fn sample(&self, channel: usize, index: isize) -> i32 {
        if index < 0 {
            return 0;
        }
        *self.samples.get(index as usize * self.nch + channel).unwrap_or(&0) as i32
    }

    pub(crate) fn packet(&mut self, aframes: usize) -> Result<Vec<u8>, &'static str> {
        let mut out = Vec::new();
        for _ in 0..aframes {
            for c in 0..self.nch {
                let target: [i32; AFRAME] = std::array::from_fn(|i| self.sample(c, (self.at + i) as isize));
                let around: Vec<f32> = (-64..AFRAME as isize + 64).map(|i| self.sample(c, self.at as isize + i) as f32).collect();
                out.extend(self.encoders[c].frame(&target, &around)?);
            }
            self.at += AFRAME;
        }
        Ok(out)
    }
}

// --------------------------------------------------------------------------------------------
//  Le fichier
// --------------------------------------------------------------------------------------------

/// Ce que l'écriture a donné, pour prévenir l'utilisateur.
#[derive(Clone, Debug, Default)]
pub struct Written {
    pub frames: u32,
    /// Le niveau de compression le plus fort employé, de 0 à 6. Haut : l'image a été abîmée pour tenir dans la taille de l'original.
    pub worst_level: usize,
    pub size: usize,
}

/// Le son déjà dans une vidéo, en -1..1 comme le rendent les autres lecteurs.
fn read_original(original: &[u8]) -> Result<(Vx, Vec<Packet>), String> {
    let bytes: std::sync::Arc<[u8]> = original.into();
    let (mut vx, _) = Vx::open(bytes.clone())?;
    let mut packets = Vec::new();
    let mut pcm = Vec::new();
    while let Some(result) = vx.next(&mut pcm) {
        pcm.clear();
        let (start, end) = vx.audio_span;
        let aframes = le16(&bytes, vx.packet_at + 2);
        // Une image illisible de l'original : son son sera écrit à neuf
        let audio = if result.is_ok() { bytes[start..end].to_vec() } else { Vec::new() };
        packets.push(Packet { aframes, audio });
    }
    Ok((vx, packets))
}

/// Écrit une vidéo VX à partir d'images RGBA à la taille de l'original, en reprenant ses réglages et le rythme de son son.
/// `sound` : le nouveau son, entrelacé, à la fréquence et avec les voies de l'original. `None` : le son de l'original reste, tel quel.
pub fn encode_like(original: &[u8], images: impl Iterator<Item = Result<Vec<u8>, String>>, sound: Option<&[i16]>) -> Result<(Vec<u8>, Written), String> {
    let (reader, packets) = read_original(original)?;
    let (w, h, nch, audio_at) = (reader.w, reader.h, reader.nch, reader.audio_at);
    let quantizer = le32(original, 0x14) as i32;
    let settings = Settings { w: w as i32, h: h as i32, quantizer, qtab: reader.qtab };
    let limit = le32(original, 0x20) as usize;
    let limit = if limit < 4 { 0xFFFE } else { limit.min(0xFFFE) };
    let old_stream: usize = packets.iter().map(|_| 0).sum::<usize>() + stream_size(original, audio_at);
    let per_frame = old_stream / packets.len().max(1);
    let seek_at = le32(original, 0x28) as usize;
    let seek_count = le32(original, 0x2C) as usize;
    let seeks: Vec<(u32, u32)> = (0..seek_count).filter_map(|i| Some((original.get(seek_at + i * 8..seek_at + i * 8 + 4)?, original.get(seek_at + i * 8 + 4..seek_at + i * 8 + 8)?))).map(|(a, b)| (u32::from_le_bytes(a.try_into().unwrap()), u32::from_le_bytes(b.try_into().unwrap()))).collect();

    let mut track = sound.map(|samples| SoundTrack::new(samples, nch, &original[audio_at..]));

    let mut out = original[..DATA_START].to_vec();
    let mut written = Written::default();
    let mut previous: Option<Pic> = None;
    let mut new_seeks = Vec::new();
    // La première image part de rien et coûte cher : elle a droit au maximum
    let mut bank = limit as isize;
    let mut level = 0usize;
    let mut biggest = 0usize;
    for (index, image) in images.enumerate() {
        let image = image?;
        if image.len() != w * h * 4 {
            return Err(format!("image de {} octets, {} attendus", image.len(), w * h * 4));
        }
        let source = rgba_to_pic(&image, w, h);
        let packet = packets.get(index);
        let schedule = packets.get(index % packets.len().max(1)).map_or(0, |p| p.aframes) as usize;
        let audio = match (&mut track, packet) {
            (Some(track), _) => track.packet(schedule).map_err(str::to_string)?,
            (None, Some(p)) if p.audio.len() == schedule * nch * 20 || !p.audio.is_empty() => p.audio.clone(),
            // L'image dure plus longtemps que l'original : du silence
            _ if nch > 0 => vec_silence(schedule, nch),
            _ => Vec::new(),
        };
        let keyframe = seeks.iter().any(|(frame, _)| *frame as usize == index);
        let reference = if keyframe { None } else { previous.as_ref() };
        let budget = (per_frame as isize + bank).clamp(limit as isize / 2, limit as isize) as usize;
        let room = |size: usize| 4 + size + audio.len();
        let mut try_level = level.saturating_sub(2);
        let (picture, bits) = loop {
            let (picture, bits) = encode_picture(&settings, &source, reference, try_level);
            if room(bits.len()) <= budget || try_level + 1 >= LEVELS.len() {
                break (picture, bits);
            }
            try_level += 1;
        };
        let size = room(bits.len()) - 2;
        if size + 2 > limit {
            return Err(format!("image {index} : {} octets, le jeu en attend {limit} au plus", size + 2));
        }
        level = try_level;
        written.worst_level = written.worst_level.max(level);
        biggest = biggest.max(size + 2);
        bank = (bank + per_frame as isize - (size + 2) as isize).clamp(-(limit as isize), limit as isize * 4);
        if keyframe {
            new_seeks.push((index as u32, out.len() as u32));
        }
        out.extend_from_slice(&(size as u16).to_le_bytes());
        out.extend_from_slice(&(if nch > 0 { schedule as u16 } else { 0 }).to_le_bytes());
        out.extend_from_slice(&bits);
        out.extend_from_slice(&audio);
        previous = Some(picture);
        written.frames += 1;
    }
    if written.frames == 0 {
        return Err("aucune image dans la video".into());
    }
    let new_audio_at = out.len();
    if nch > 0 {
        out.extend_from_slice(&original[audio_at..audio_at + nch * AUDIO_EXTRA]);
    }
    let new_seek_at = out.len();
    for (frame, at) in &new_seeks {
        out.extend_from_slice(&frame.to_le_bytes());
        out.extend_from_slice(&at.to_le_bytes());
    }
    let patch = |out: &mut Vec<u8>, at: usize, value: u32| out[at..at + 4].copy_from_slice(&value.to_le_bytes());
    patch(&mut out, 4, written.frames);
    patch(&mut out, 0x20, biggest.max(limit.min(0xFFFE)) as u32);
    patch(&mut out, 0x24, if nch > 0 { new_audio_at as u32 } else { le32(original, 0x24) });
    patch(&mut out, 0x28, new_seek_at as u32);
    patch(&mut out, 0x2C, new_seeks.len() as u32);
    written.size = out.len();
    Ok((out, written))
}

fn stream_size(original: &[u8], audio_at: usize) -> usize {
    let end = if audio_at != 0 { audio_at } else { le32(original, 0x28) as usize };
    end.saturating_sub(DATA_START).min(original.len())
}

/// Le silence le plus simple qui reste lisible : des impulsions à zéro d'échelle n'existent pas, alors les plus petites, avec filtre courant.
pub(crate) fn vec_silence(aframes: usize, nch: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(aframes * nch * 20);
    for _ in 0..aframes * nch {
        let mut bits = BitWriter::default();
        bits.put(16, 0x7E << 9);
        bits.put(16, 0);
        for _ in 0..8 {
            bits.put(16, 0b011_100_011_100_011_0);
        }
        out.extend(bits.finish());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic(frames: usize, w: usize, h: usize) -> Vec<Vec<u8>> {
        (0..frames)
            .map(|f| {
                let mut image = vec![255u8; w * h * 4];
                for y in 0..h {
                    for x in 0..w {
                        let px = &mut image[(y * w + x) * 4..][..3];
                        let edge = (x + f * 3) / 24 % 2 == (y / 24) % 2;
                        px[0] = if edge { 220 } else { 30 };
                        px[1] = (x * 255 / w) as u8;
                        px[2] = (y * 255 / h) as u8;
                    }
                }
                image
            })
            .collect()
    }

    /// Un fichier minimal, sans son : assez pour que l'écriture reprenne ses réglages.
    fn empty_vx(frames: u32, w: u32, h: u32, quantizer: u32, limit: u32) -> Vec<u8> {
        let mut out = b"VXDS".to_vec();
        for value in [frames, w, h, 15 << 16, quantizer, 0, 0, limit, 0, 0x30, 0] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out
    }

    fn psnr(a: &[u8], b: &[u8]) -> f32 {
        let total: u64 = a.iter().zip(b).map(|(x, y)| (*x as i64 - *y as i64).unsigned_abs().pow(2)).sum();
        10.0 * (255f32 * 255.0 / (total as f32 / a.len() as f32).max(0.0001)).log10()
    }

    #[test]
    fn the_transform_goes_back_to_where_it_came_from() {
        let settings = Settings { w: 16, h: 16, quantizer: 12, qtab: std::array::from_fn(|i| VX_QUANT[0][i] << 2) };
        let residue: [i32; 16] = std::array::from_fn(|i| ((i as i32 * 37) % 41) - 20);
        let levels = quantize(&settings, &residue, 0);
        let mut pic: Pic = [vec![128; 256], vec![128; 64], vec![128; 64]];
        let mut unused = Msb::from_words(&[]);
        let mut frame = Frame { gb: &mut unused, w: 16, h: 16, qtab: settings.qtab, dst: &mut pic, refs: [None; 3], coeff_y: vec![0; 25], coeff_y_stride: 5, coeff_uv: vec![0; 9], coeff_uv_stride: 3, vectors: vec![Mv::default(); 6], vectors_stride: 3 };
        frame.idct_add(0, 0, 0, &levels);
        for i in 0..4 {
            for k in 0..4 {
                let back = frame.get(0, i, k) - 128;
                assert!((back - residue[(i * 4 + k) as usize]).abs() <= 3, "{i},{k} : {back} au lieu de {}", residue[(i * 4 + k) as usize]);
            }
        }
    }

    #[test]
    fn a_written_video_reads_back_without_errors_and_looks_the_same() {
        let (w, h) = (64, 48);
        let images = synthetic(12, w, h);
        let original = empty_vx(1, w as u32, h as u32, 24, 0xFFFE);
        let (bytes, written) = encode_like(&original, images.iter().map(|i| Ok(i.clone())), None).unwrap();
        assert_eq!(written.frames, 12);
        let mut video = crate::Video::open(bytes).unwrap();
        let (mut pcm, mut frame, mut count) = (Vec::new(), Vec::new(), 0);
        while video.advance(&mut pcm) {
            video.rgba(&mut frame);
            let quality = psnr(&frame, &images[count]);
            assert!(quality > 25.0, "image {count} : {quality} dB");
            count += 1;
        }
        assert_eq!(count, 12);
        assert_eq!(video.errors(), (0, None));
    }
}
