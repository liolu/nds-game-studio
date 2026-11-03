//! L'écriture d'une vidéo MODS : chaque bloc passe par les calculs du décodeur d'à côté, le résultat est ce que le jeu affichera.
//!
//! Le fichier garde l'en-tête de l'original et sa taille maximale de paquet : le jeu réserve sa mémoire d'après elle. Tout paquet fait
//! un multiple de 4 octets : le jeu lit le mot du paquet suivant d'un seul coup, à la fin de celui-ci. On ne prédit jamais depuis l'extérieur
//! de l'image ni depuis un bloc pas encore décodé : le code du jeu y lit ce qui traîne dans ses tampons, pas ce que lirait le décodeur Rust.

use super::*;
use crate::vx::{BitWriter, SoundTrack, Written, vec_silence};

/// Une suite de codes, qu'on compte avant de les écrire : l'en-tête d'un macrobloc dépend de ce qui vient après.
#[derive(Clone, Default)]
struct Tokens(Vec<(u32, u32)>);

impl Tokens {
    fn put(&mut self, count: u32, value: u32) {
        self.0.push((count, value));
    }

    fn ue(&mut self, value: u32) {
        let code = value as u64 + 1;
        let zeros = 63 - code.leading_zeros();
        self.put(zeros + 1, 1);
        self.put(zeros, (code & ((1u64 << zeros) - 1)) as u32);
    }

    fn se(&mut self, value: i32) {
        self.ue(if value > 0 { value as u32 * 2 - 1 } else { value.unsigned_abs() * 2 });
    }

    fn bits(&self) -> i32 {
        self.0.iter().map(|t| t.0 as i32).sum()
    }

    fn extend(&mut self, other: &Tokens) {
        self.0.extend_from_slice(&other.0);
    }

    fn write(&self, out: &mut BitWriter) {
        for &(count, value) in &self.0 {
            out.put(count, value);
        }
    }
}

/// Le code de chaque symbole d'une table décrite par ses longueurs, attribués dans l'ordre comme `Vlc::from_lengths`.
fn codes_of(lens: &[u8]) -> Vec<(u32, u32)> {
    let mut code = 0u64;
    lens.iter()
        .map(|&len| {
            if len == 0 {
                return (0, 0);
            }
            let out = (len as u32, (code >> (32 - len as u32)) as u32);
            code += 1u64 << (32 - len as u32);
            out
        })
        .collect()
}

struct Writers {
    /// `[last][run][level]` -> (longueur, code), `dct_tab` 0.
    run_codes: Vec<(u32, u32)>,
    escape: (u32, u32),
    mv: [(u32, u32); 10],
    basis4: [[f32; 4]; 4],
    norm4: [f32; 4],
    basis8: [[f32; 8]; 8],
    norm8: [f32; 8],
}

fn run_index(last: bool, run: i32, level: i32) -> usize {
    usize::from(last) * 64 * 32 + run as usize * 32 + level as usize
}

/// Ce que rend `idct` pour un seul coefficient : la colonne de sa matrice, à l'échelle de 1.
fn response<const N: usize>(k: usize) -> [f32; N] {
    const SCALE: i32 = 1 << 14;
    let mut line = [0i32; 8];
    line[k] = SCALE;
    idct(&mut line[..N], N);
    std::array::from_fn(|i| line[i] as f32 / SCALE as f32)
}

fn writers() -> &'static Writers {
    static WRITERS: OnceLock<Writers> = OnceLock::new();
    WRITERS.get_or_init(|| {
        let codes = codes_of(&MC_RL_LEN);
        let mut run_codes = vec![(0, 0); 2 * 64 * 32];
        let mut escape = (0, 0);
        for (i, &code) in codes.iter().enumerate() {
            let symbol = MC_RL_SYMS0[i] as i32;
            let (last, run, level) = ((symbol >> 11) == 1, (symbol >> 5) & 0x3F, symbol & 0x1F);
            if code.0 == 0 {
                continue;
            }
            if level == 0 {
                escape = code;
            } else {
                run_codes[run_index(last, run, level)] = code;
            }
        }
        let mv_codes = codes_of(&MC_MV_LEN[0][0][..MC_MV_COUNT[0] as usize]);
        let mut mv = [(0, 0); 10];
        for (i, &code) in mv_codes.iter().enumerate() {
            mv[MC_MV_SYMS[0][0][i] as usize] = code;
        }
        let basis4: [[f32; 4]; 4] = std::array::from_fn(response::<4>);
        let basis8: [[f32; 8]; 8] = std::array::from_fn(response::<8>);
        Writers { run_codes, escape, mv, basis4, norm4: std::array::from_fn(|k| basis4[k].iter().map(|v| v * v).sum()), basis8, norm8: std::array::from_fn(|k| basis8[k].iter().map(|v| v * v).sum()) }
    })
}

/// La transformée directe qui répond à `add_matrix` : coefficients déjà multipliés, dans l'ordre du zigzag.
fn forward(residue: &[f32], size: usize) -> [f32; 64] {
    let tables = writers();
    let mut out = [0f32; 64];
    let mut mid = [0f32; 64];
    // `add_matrix` : mat[r * n + c] donne base(c) en y et base(r) en x
    for y in 0..size {
        for r in 0..size {
            mid[y * size + r] = (0..size).map(|x| residue[y * size + x] * if size == 8 { tables.basis8[r][x] } else { tables.basis4[r][x] }).sum::<f32>() / if size == 8 { tables.norm8[r] } else { tables.norm4[r] };
        }
    }
    for r in 0..size {
        for c in 0..size {
            let value = (0..size).map(|y| mid[y * size + r] * if size == 8 { tables.basis8[c][y] } else { tables.basis4[c][y] }).sum::<f32>() / if size == 8 { tables.norm8[c] } else { tables.norm4[c] };
            out[r * size + c] = value * 64.0;
        }
    }
    out
}

/// Les valeurs de la transformée inverse restent loin de 16 bits, à chacune de ses deux passes, et le pixel reste près de 0..255 : le jeu
/// le borne par une table, qu'il lirait au-delà de sa fin. `predicted` : le bloc avant ces coefficients.
fn fits_the_game(mut mat: [i32; 64], n: usize, predicted: &[i32]) -> bool {
    const LIMIT: i32 = 24000;
    const CLIP_MARGIN: i32 = 64;
    let small = |values: &[i32]| values.iter().all(|v| v.abs() < LIMIT);
    mat[0] += 32;
    if !small(&mat[..n * n]) {
        return false;
    }
    for y in 0..n {
        idct(&mut mat[y * n..(y + 1) * n], n);
    }
    if !small(&mat[..n * n]) {
        return false;
    }
    for y in 0..n {
        for x in y + 1..n {
            mat.swap(x * n + y, y * n + x);
        }
        idct(&mut mat[y * n..(y + 1) * n], n);
    }
    small(&mat[..n * n]) && (0..n * n).all(|i| (-CLIP_MARGIN..=255 + CLIP_MARGIN).contains(&(predicted[i] + (mat[i] >> 6))))
}

fn write_run(out: &mut Tokens, last: bool, run: i32, level: i32) {
    let tables = writers();
    let size = level.abs();
    let code = |last: bool, run: i32, size: i32| -> Option<(u32, u32)> {
        if !(0..64).contains(&run) || !(1..32).contains(&size) {
            return None;
        }
        let code = tables.run_codes[run_index(last, run, size)];
        (code.0 != 0).then_some(code)
    };
    let sign = u32::from(level < 0);
    if let Some((len, value)) = code(last, run, size) {
        out.put(len, value);
        out.put(1, sign);
        return;
    }
    out.put(tables.escape.0, tables.escape.1);
    let residue = &MC_RUN_RESIDUE[0];
    let shift = if last { 64 } else { 0 };
    let smaller = size - residue[shift + run as usize] as i32;
    if let Some((len, value)) = code(last, run, smaller) {
        out.put(1, 0);
        out.put(len, value);
        out.put(1, sign);
        return;
    }
    if size < 64 {
        let shorter = run - residue[128 + shift + size as usize] as i32;
        if let Some((len, value)) = code(last, shorter, size) {
            out.put(2, 0b10);
            out.put(len, value);
            out.put(1, sign);
            return;
        }
    }
    out.put(2, 0b11);
    out.put(1, u32::from(last));
    out.put(6, run as u32);
    out.put(12, (level.clamp(-2047, 2047) & 0xFFF) as u32);
}

/// Les codes d'un bloc de coefficients : `levels` dans l'ordre du zigzag, avec au moins un non nul.
fn write_block(out: &mut Tokens, levels: &[i32]) {
    let end = levels.iter().rposition(|l| *l != 0).expect("un coefficient");
    let mut run = 0;
    for (pos, &level) in levels.iter().enumerate().take(end + 1) {
        if level == 0 {
            run += 1;
        } else {
            write_run(out, pos == end, run, level);
            run = 0;
        }
    }
}

const STARTS: [(i32, i32); 4] = [(0, 0), (8, 0), (0, 8), (8, 8)];

/// Ce que coûte un essai sur un macrobloc.
#[derive(Clone)]
struct Trial {
    tokens: Tokens,
    distortion: i64,
}

impl Trial {
    fn cost(&self, lambda: f32) -> f32 {
        self.distortion as f32 + self.tokens.bits() as f32 * lambda
    }
}

struct Encoder {
    codec: Mobiclip,
    source: Pic,
    dst: Pic,
    lambda: f32,
    deadzone: f32,
}

impl Encoder {
    fn size_of(&self, plane: usize) -> (i32, i32, usize) {
        if plane == 0 { (self.codec.w, self.codec.h, self.codec.w as usize) } else { (self.codec.w >> 1, self.codec.h >> 1, (self.codec.w >> 1) as usize) }
    }

    /// La somme des carrés des écarts avec l'image voulue.
    fn distortion(&self, plane: usize, x: i32, y: i32, size: i32) -> i64 {
        let (_, _, stride) = self.size_of(plane);
        let mut sum = 0i64;
        for j in 0..size as usize {
            let at = (y as usize + j) * stride + x as usize;
            for (a, b) in self.dst[plane][at..at + size as usize].iter().zip(&self.source[plane][at..at + size as usize]) {
                sum += (*a as i64 - *b as i64).pow(2);
            }
        }
        sum
    }

    fn save_block(&self, plane: usize, x: i32, y: i32, size: i32) -> Vec<u8> {
        let (_, _, stride) = self.size_of(plane);
        (0..size as usize).flat_map(|j| self.dst[plane][(y as usize + j) * stride + x as usize..][..size as usize].iter().copied()).collect()
    }

    fn restore_block(&mut self, saved: &[u8], plane: usize, x: i32, y: i32, size: i32) {
        let (_, _, stride) = self.size_of(plane);
        for j in 0..size as usize {
            self.dst[plane][(y as usize + j) * stride + x as usize..][..size as usize].copy_from_slice(&saved[j * size as usize..][..size as usize]);
        }
    }

    /// Le résidu d'un bloc déjà prédit : l'ajoute à l'image, et rend ses coefficients dans l'ordre du zigzag s'il en reste.
    fn residue(&mut self, plane: usize, x: i32, y: i32, size: i32) -> Option<[i32; 64]> {
        let (_, _, stride) = self.size_of(plane);
        let n = size as usize;
        let mut wanted = [0f32; 64];
        for j in 0..n {
            for i in 0..n {
                let at = (y as usize + j) * stride + x as usize + i;
                wanted[j * n + i] = self.source[plane][at] as f32 - self.dst[plane][at] as f32;
            }
        }
        let dct = forward(&wanted[..n * n], n);
        let qtab = &self.codec.qtab[usize::from(size == 8)];
        let mut levels = [0i32; 64];
        for pos in 0..n * n {
            let at = if size == 8 { ZIGZAG8[pos] } else { MC_ZIGZAG4[pos] } as usize;
            let level = (dct[at].abs() / qtab[pos] as f32 + self.deadzone) as i32;
            levels[pos] = level.min(2047) * if dct[at] < 0.0 { -1 } else { 1 };
        }
        // Le code du jeu ne fait pas ce calcul sur 32 bits : un fort quantificateur le ferait déborder, et le pixel partirait de l'autre côté
        loop {
            let mut mat = [0i32; 64];
            for pos in 0..n * n {
                let at = if size == 8 { ZIGZAG8[pos] } else { MC_ZIGZAG4[pos] } as usize;
                mat[at] = qtab[pos].wrapping_mul(levels[pos]);
            }
            let predicted: Vec<i32> = (0..n * n).map(|i| self.dst[plane][(y as usize + i / n) * stride + x as usize + i % n] as i32).collect();
            if fits_the_game(mat, n, &predicted) {
                if levels.iter().all(|l| *l == 0) {
                    return None;
                }
                self.codec.add_matrix(mat, &mut self.dst, x, y, size, plane);
                return Some(levels);
            }
            for level in &mut levels {
                *level = *level * 3 / 4;
            }
        }
    }

    /// Prédit un bloc comme la lecture. Le plan se règle sur le coin bas-droite voulu, et rend son réglage.
    fn predict(&mut self, plane: usize, x: i32, y: i32, size: i32, mode: i32) -> Option<i32> {
        if mode == 2 {
            let (_, _, stride) = self.size_of(plane);
            let corner = |p: &Pic, cx: i32, cy: i32| p[plane][cy as usize * stride + cx as usize] as i32;
            let bottommost = corner(&self.dst, x - 1, y + size - 1);
            let rightmost = corner(&self.dst, x + size - 1, y - 1);
            let wanted = corner(&self.source, x + size - 1, y + size - 1);
            let delta = (wanted - (bottommost + rightmost + 1) / 2) / 2;
            self.codec.predict_plane(&mut self.dst, x, y, size, plane, delta);
            return Some(delta);
        }
        let mut nothing = Msb::from_words(&[]);
        let codec = std::mem::replace(&mut self.codec, Mobiclip::new(0, 0));
        let _ = codec.predict_intra(&mut nothing, &mut self.dst, x, y, mode, false, size, plane);
        self.codec = codec;
        None
    }

    /// Ces modes ne lisent que des pixels déjà décodés, dans l'image.
    fn allowed(mode: i32, x: i32, y: i32) -> bool {
        match mode {
            0 => y > 0,
            1 | 4 => x > 0,
            3 => true,
            5..=7 => x > 0 && y > 0,
            _ => false,
        }
    }

    fn sad_after(&mut self, plane: usize, x: i32, y: i32, size: i32, mode: i32) -> i64 {
        self.predict(plane, x, y, size, mode);
        self.distortion(plane, x, y, size)
    }

    /// Lit le mode le plus probable sans rien écrire, comme `get_prediction`.
    fn likely_mode(&self, x: i32, y: i32) -> (usize, u8) {
        let index = ((y & 0xC) | (x / 4 % 4)) as usize;
        let value = self.codec.pre[index].min(if index % 4 == 0 { 9 } else { self.codec.pre[index + 3] });
        (index, if value == 9 { 3 } else { value })
    }

    fn write_mode(&mut self, out: &mut Tokens, x: i32, y: i32, size: i32, mode: i32) {
        let (index, likely) = self.likely_mode(x, y);
        let mode = mode as u8;
        if mode == likely {
            out.put(1, 1);
        } else {
            out.put(1, 0);
            out.put(3, (mode - u8::from(mode > likely)) as u32);
        }
        self.codec.pre[index + 4] = mode;
        if size == 8 {
            self.codec.pre[index + 5] = mode;
            self.codec.pre[index + 8] = mode;
            self.codec.pre[index + 9] = mode;
        }
    }

    fn best_block_mode(&mut self, plane: usize, x: i32, y: i32, size: i32) -> i32 {
        let saved = self.save_block(plane, x, y, size);
        let mut best = (i64::MAX, 3);
        for mode in [3, 0, 1, 4, 5, 6, 7] {
            if Self::allowed(mode, x, y) {
                let cost = self.sad_after(plane, x, y, size, mode);
                if cost < best.0 {
                    best = (cost, mode);
                }
                self.restore_block(&saved, plane, x, y, size);
            }
        }
        best.1
    }

    /// Un bloc 8x8 (ou 4x4 quatre fois). `mode` : celui du macrobloc, ou `None` s'il se choisit ici et s'écrit. Rend (codes, avec des coefficients).
    fn intra_block(&mut self, plane: usize, x: i32, y: i32, mode: Option<i32>) -> (Tokens, bool) {
        let before = self.save_block(plane, x, y, 8);
        let pre = self.codec.pre;

        // En un seul bloc de 8x8
        let mut whole = Tokens::default();
        let chosen = mode.unwrap_or_else(|| self.best_block_mode(plane, x, y, 8));
        if mode.is_none() {
            self.write_mode(&mut whole, x, y, 8, chosen);
        }
        if chosen != 9 {
            self.predict(plane, x, y, 8, chosen);
        }
        let levels = self.residue(plane, x, y, 8);
        let mut body = Tokens::default();
        if levels.is_some() {
            body.ue(0);
        }
        body.extend(&whole);
        if let Some(levels) = &levels {
            write_block(&mut body, &levels[..64]);
        }
        let whole_trial = Trial { tokens: body, distortion: self.distortion(plane, x, y, 8) };
        let whole_pixels = self.save_block(plane, x, y, 8);
        let whole_pre = self.codec.pre;
        let whole_flag = levels.is_some();

        // En quatre blocs de 4x4 : seulement si le bloc entier ne suffit pas
        self.restore_block(&before, plane, x, y, 8);
        self.codec.pre = pre;
        let mut flags = 0u8;
        let mut parts = Tokens::default();
        for (bit, (dx, dy)) in [(0, 0), (4, 0), (0, 4), (4, 4)].into_iter().enumerate() {
            let (bx, by) = (x + dx, y + dy);
            let chosen = mode.unwrap_or_else(|| self.best_block_mode(plane, bx, by, 4));
            if mode.is_none() {
                self.write_mode(&mut parts, bx, by, 4, chosen);
            }
            if chosen != 9 {
                self.predict(plane, bx, by, 4, chosen);
            }
            if let Some(levels) = self.residue(plane, bx, by, 4) {
                flags |= 1 << bit;
                write_block(&mut parts, &levels[..16]);
            }
        }
        let mut split = Tokens::default();
        split.ue(MC_BLOCK4.iter().position(|f| *f == flags).expect("les 16 motifs") as u32 + 1);
        split.extend(&parts);
        let split_trial = Trial { tokens: split, distortion: self.distortion(plane, x, y, 8) };
        if split_trial.cost(self.lambda) < whole_trial.cost(self.lambda) {
            return (split_trial.tokens, true);
        }
        self.restore_block(&whole_pixels, plane, x, y, 8);
        self.codec.pre = whole_pre;
        (whole_trial.tokens, whole_flag)
    }

    /// La luminance et la couleur d'un macrobloc intra, comme `decode_macroblock`.
    fn intra_macroblock(&mut self, x: i32, y: i32, luma_mode: Option<i32>, colors: i32) -> Trial {
        let mut head = Tokens::default();
        let mut body = Tokens::default();
        let mut flags = 0u8;
        let mut pmode = luma_mode;
        if let Some(mode) = luma_mode {
            head.put(3, mode as u32);
            if mode == 2 {
                let delta = self.predict(0, x, y, 16, 2).unwrap_or(0);
                head.se(delta);
                pmode = Some(9);
            }
        }
        for (bit, (dx, dy)) in STARTS.into_iter().enumerate() {
            let (tokens, coded) = self.intra_block(0, x + dx, y + dy, pmode);
            if coded {
                flags |= 1 << bit;
            }
            body.extend(&tokens);
        }
        let (cx, cy) = (x >> 1, y >> 1);
        let mut pmode_uv = colors;
        let mut color_head = Tokens::default();
        color_head.put(3, colors as u32);
        if colors == 2 {
            // Le premier plan lu est le plan 2
            for plane in [2, 1] {
                let delta = self.predict(plane, cx, cy, 8, 2).unwrap_or(0);
                color_head.se(delta);
            }
            pmode_uv = 9;
        }
        body.extend(&color_head);
        for (bit, plane) in [(4, 2), (5, 1)] {
            let (tokens, coded) = self.intra_block(plane, cx, cy, Some(pmode_uv));
            if coded {
                flags |= 1 << bit;
            }
            body.extend(&tokens);
        }
        let mut tokens = Tokens::default();
        tokens.ue(MC_BLOCK8.iter().position(|f| *f == flags).expect("les 64 motifs") as u32);
        tokens.extend(&head);
        tokens.extend(&body);
        Trial { tokens, distortion: self.distortion(0, x, y, 16) + self.distortion(1, cx, cy, 8) + self.distortion(2, cx, cy, 8) }
    }

    fn save_macroblock(&self, x: i32, y: i32) -> [Vec<u8>; 3] {
        [self.save_block(0, x, y, 16), self.save_block(1, x >> 1, y >> 1, 8), self.save_block(2, x >> 1, y >> 1, 8)]
    }

    fn restore_macroblock(&mut self, saved: &[Vec<u8>; 3], x: i32, y: i32) {
        self.restore_block(&saved[0], 0, x, y, 16);
        self.restore_block(&saved[1], 1, x >> 1, y >> 1, 8);
        self.restore_block(&saved[2], 2, x >> 1, y >> 1, 8);
    }

    /// Le meilleur essai intra de ce macrobloc, écrit dans l'image : ses codes, et le signal `predict` de son appelant.
    fn best_intra(&mut self, x: i32, y: i32) -> (Trial, bool) {
        let before = self.save_macroblock(x, y);
        let pre = self.codec.pre;
        let colors = {
            let (top, left) = (y > 0, x > 0);
            let (cx, cy) = (x >> 1, y >> 1);
            let mut best = (i64::MAX, 3);
            for mode in [3, 0, 1, 2] {
                let usable = match mode {
                    0 => top,
                    1 => left,
                    2 => top && left,
                    _ => true,
                };
                if usable {
                    let saved = [self.save_block(1, cx, cy, 8), self.save_block(2, cx, cy, 8)];
                    self.predict(1, cx, cy, 8, mode);
                    self.predict(2, cx, cy, 8, mode);
                    let cost = self.distortion(1, cx, cy, 8) + self.distortion(2, cx, cy, 8);
                    self.restore_block(&saved[0], 1, cx, cy, 8);
                    self.restore_block(&saved[1], 2, cx, cy, 8);
                    if cost < best.0 {
                        best = (cost, mode);
                    }
                }
            }
            best.1
        };
        let mut best: Option<(Trial, bool, [Vec<u8>; 3], [u8; 32])> = None;
        let mut consider = |this: &mut Self, luma: Option<i32>| {
            this.restore_macroblock(&before, x, y);
            this.codec.pre = pre;
            let trial = this.intra_macroblock(x, y, luma, colors);
            if best.as_ref().is_none_or(|b| trial.cost(this.lambda) < b.0.cost(this.lambda)) {
                best = Some((trial, luma.is_none(), this.save_macroblock(x, y), this.codec.pre));
            }
        };
        consider(self, Some(3));
        if y > 0 {
            consider(self, Some(0));
        }
        if x > 0 {
            consider(self, Some(1));
        }
        if x > 0 && y > 0 {
            consider(self, Some(2));
        }
        consider(self, None);
        let (trial, predict, pixels, pre) = best.expect("au moins le mode uni");
        self.restore_macroblock(&pixels, x, y);
        self.codec.pre = pre;
        (trial, predict)
    }

    fn motion_usable(&self, x: i32, y: i32, mx: i32, my: i32) -> bool {
        let (w, h) = (self.codec.w, self.codec.h);
        // Le code du jeu range les vecteurs sur peu de bits : on reste loin de ses limites, comme pour VX
        mx.abs() <= 120 && my.abs() <= 120 && x + (mx >> 1) >= 0 && y + (my >> 1) >= 0 && x + 16 + ((mx + 1) >> 1) <= w && y + 16 + ((my + 1) >> 1) <= h
    }

    fn motion_cost(&mut self, source: usize, x: i32, y: i32, mx: i32, my: i32) -> i64 {
        let codec = std::mem::replace(&mut self.codec, Mobiclip::new(0, 0));
        let result = codec.motion_copy(source, &mut self.dst, 16, 16, mx, my, x, y);
        self.codec = codec;
        if result.is_err() {
            return i64::MAX;
        }
        self.distortion(0, x, y, 16)
    }

    /// Le vecteur le plus proche de l'image voulue, en demi-pixels.
    fn search_motion(&mut self, source: usize, x: i32, y: i32, starts: &[(i32, i32)]) -> (i32, i32) {
        let mut best = (0, 0);
        let mut best_cost = i64::MAX;
        let try_one = |this: &mut Self, v: (i32, i32), best: &mut (i32, i32), best_cost: &mut i64| -> bool {
            if !this.motion_usable(x, y, v.0, v.1) {
                return false;
            }
            let cost = this.motion_cost(source, x, y, v.0, v.1);
            if cost < *best_cost {
                *best_cost = cost;
                *best = v;
                true
            } else {
                false
            }
        };
        for &start in starts {
            try_one(self, start, &mut best, &mut best_cost);
        }
        let mut step = 16;
        while step > 0 {
            let center = best;
            let mut moved = false;
            for (dx, dy) in [(step, 0), (-step, 0), (0, step), (0, -step)] {
                moved |= try_one(self, (center.0 + dx, center.1 + dy), &mut best, &mut best_cost);
            }
            if !moved {
                step /= 2;
            }
        }
        best
    }

    /// Le résidu d'un bloc 8x8 après mouvement : (codes, avec des coefficients).
    fn inter_block(&mut self, plane: usize, x: i32, y: i32) -> (Tokens, bool) {
        let before = self.save_block(plane, x, y, 8);
        let plain = self.distortion(plane, x, y, 8);
        let mut whole = Tokens::default();
        let levels = self.residue(plane, x, y, 8);
        let whole_coded = levels.is_some();
        if let Some(levels) = &levels {
            whole.ue(0);
            write_block(&mut whole, &levels[..64]);
        }
        let whole_trial = Trial { tokens: whole, distortion: self.distortion(plane, x, y, 8) };
        let whole_pixels = self.save_block(plane, x, y, 8);
        self.restore_block(&before, plane, x, y, 8);
        let mut flags = 0u8;
        let mut parts = Tokens::default();
        for (bit, (dx, dy)) in [(0, 0), (4, 0), (0, 4), (4, 4)].into_iter().enumerate() {
            if let Some(levels) = self.residue(plane, x + dx, y + dy, 4) {
                flags |= 1 << bit;
                write_block(&mut parts, &levels[..16]);
            }
        }
        let mut split = Trial { tokens: Tokens::default(), distortion: self.distortion(plane, x, y, 8) };
        if flags != 0 {
            split.tokens.ue(MC_PBLOCK4.iter().position(|f| *f == flags).expect("les 16 motifs") as u32);
            split.tokens.extend(&parts);
        }
        let lambda = self.lambda;
        let nothing = Trial { tokens: Tokens::default(), distortion: plain };
        if nothing.cost(lambda) <= split.cost(lambda) && nothing.cost(lambda) <= whole_trial.cost(lambda) {
            self.restore_block(&before, plane, x, y, 8);
            return (nothing.tokens, false);
        }
        if flags != 0 && split.cost(lambda) < whole_trial.cost(lambda) {
            return (split.tokens, true);
        }
        self.restore_block(&whole_pixels, plane, x, y, 8);
        (whole_trial.tokens, whole_coded)
    }

    /// Un macrobloc qui part de l'image précédente, comme la fin de la boucle de `decode_picture`.
    fn inter_macroblock(&mut self, source: usize, x: i32, y: i32, index: u32, delta: (i32, i32), vector: (i32, i32)) -> Trial {
        let tables = writers();
        let mut tokens = Tokens::default();
        tokens.put(tables.mv[index as usize].0, tables.mv[index as usize].1);
        if index > 0 {
            tokens.se(delta.0);
            tokens.se(delta.1);
        }
        let codec = std::mem::replace(&mut self.codec, Mobiclip::new(0, 0));
        codec.motion_copy(source, &mut self.dst, 16, 16, vector.0, vector.1, x, y).expect("vecteur verifie");
        self.codec = codec;
        let mut flags = 0u8;
        let mut body = Tokens::default();
        for (bit, (dx, dy)) in STARTS.into_iter().enumerate() {
            let (coded, any) = self.inter_block(0, x + dx, y + dy);
            if any {
                flags |= 1 << bit;
                body.extend(&coded);
            }
        }
        let (cx, cy) = (x >> 1, y >> 1);
        for (bit, plane) in [(4, 2), (5, 1)] {
            let (coded, any) = self.inter_block(plane, cx, cy);
            if any {
                flags |= 1 << bit;
                body.extend(&coded);
            }
        }
        tokens.ue(MC_PBLOCK8.iter().position(|f| *f == flags).expect("les 64 motifs") as u32);
        tokens.extend(&body);
        Trial { tokens, distortion: self.distortion(0, x, y, 16) + self.distortion(1, cx, cy, 8) + self.distortion(2, cx, cy, 8) }
    }
}

fn rgba_to_pic(rgba: &[u8], w: usize, h: usize) -> Pic {
    let mut pic: Pic = [vec![0; w * h], vec![0; w / 2 * (h / 2)], vec![0; w / 2 * (h / 2)]];
    let luma = |px: &[u8]| (px[0] as i32 + 2 * px[1] as i32 + px[2] as i32 + 2) / 4;
    for (at, px) in rgba.chunks_exact(4).enumerate().take(w * h) {
        pic[0][at] = luma(px) as u8;
    }
    for y in 0..h / 2 {
        for x in 0..w / 2 {
            let (mut cg, mut co) = (0, 0);
            for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                let px = &rgba[((y * 2 + dy) * w + x * 2 + dx) * 4..][..4];
                cg += 2 * px[1] as i32 - px[0] as i32 - px[2] as i32;
                co += px[0] as i32 - px[2] as i32;
            }
            // Quatre pixels ; `cg` et `co` se lisent à l'échelle 1/4 et 1/2 de la somme
            pic[1][y * (w / 2) + x] = (128 + (cg + 8 * cg.signum()) / 16).clamp(0, 255) as u8;
            pic[2][y * (w / 2) + x] = (128 + (co + 4 * co.signum()) / 8).clamp(0, 255) as u8;
        }
    }
    pic
}

/// Une image entière : ses bits, et l'image telle que le jeu la verra. `key` : sans référence.
fn encode_picture(codec: Mobiclip, source: Pic, key: bool, quantizer: i32) -> (Mobiclip, Vec<u8>) {
    let (w, h) = (codec.w as usize, codec.h as usize);
    let mut encoder = Encoder { dst: [vec![0; w * h], vec![0; w / 2 * (h / 2)], vec![0; w / 2 * (h / 2)]], source, codec, lambda: 0.0, deadzone: if key { 0.33 } else { 0.17 } };
    let mut out = BitWriter::default();
    let previous = encoder.codec.quantizer;
    encoder.codec.setup_qtables(quantizer as i64).expect("quantificateur borne");
    encoder.lambda = 0.85 * 2f32.powf((quantizer - 12) as f32 / 3.0);
    if key {
        out.put(1, 1);
        out.put(1, 0);
        out.put(1, 0);
        out.put(6, quantizer as u32);
        encoder.codec.dct_tab = 0;
        for y in (0..h as i32).step_by(16) {
            for x in (0..w as i32).step_by(16) {
                let (trial, predict) = encoder.best_intra(x, y);
                out.put(1, u32::from(predict));
                trial.tokens.write(&mut out);
            }
        }
    } else {
        encoder.codec.dct_tab = 0;
        encoder.codec.motion.fill((0, 0));
        out.put(1, 0);
        let mut delta = Tokens::default();
        delta.se(quantizer - previous);
        delta.write(&mut out);
        let source_index = (encoder.codec.current + 5) % 6;
        let tables = writers();
        for y in (0..h as i32).step_by(16) {
            for x in (0..w as i32).step_by(16) {
                let col = (x / 16) as usize;
                let (a, b, c) = (encoder.codec.motion[col + 1], encoder.codec.motion[col + 2], encoder.codec.motion[col + 3]);
                let median = |a: i32, b: i32, c: i32| a.min(b).max(a.max(b).min(c));
                let pred = (median(a.0, b.0, c.0), median(a.1, b.1, c.1));
                encoder.codec.motion[col + 2] = (0, 0);

                let before = encoder.save_macroblock(x, y);
                let pre = encoder.codec.pre;
                type Best = Option<(Trial, [Vec<u8>; 3], [u8; 32], (i32, i32), Option<bool>)>;
                let mut best: Best = None;
                let lambda = encoder.lambda;
                let consider = |encoder: &mut Encoder, best: &mut Best, trial: Trial, vector: (i32, i32), intra: Option<bool>| {
                    if best.as_ref().is_none_or(|b| trial.cost(lambda) < b.0.cost(lambda)) {
                        *best = Some((trial, encoder.save_macroblock(x, y), encoder.codec.pre, vector, intra));
                    }
                    encoder.restore_macroblock(&before, x, y);
                    encoder.codec.pre = pre;
                };
                if encoder.motion_usable(x, y, pred.0, pred.1) {
                    let trial = encoder.inter_macroblock(source_index, x, y, 0, (0, 0), pred);
                    consider(&mut encoder, &mut best, trial, pred, None);
                }
                let vector = encoder.search_motion(source_index, x, y, &[(0, 0), pred, a, b, c]);
                encoder.restore_macroblock(&before, x, y);
                if encoder.motion_usable(x, y, vector.0, vector.1) {
                    let trial = encoder.inter_macroblock(source_index, x, y, 1, (vector.0 - pred.0, vector.1 - pred.1), vector);
                    consider(&mut encoder, &mut best, trial, vector, None);
                }
                // L'intra coûte cher à essayer : seulement si le mouvement laisse un écart visible
                if best.as_ref().is_none_or(|b| b.0.cost(lambda) > 16.0 * 16.0 * 16.0 + lambda * 40.0) {
                    let (trial, predict) = encoder.best_intra(x, y);
                    consider(&mut encoder, &mut best, trial, (0, 0), Some(predict));
                }
                let (trial, pixels, pre, vector, intra) = best.expect("au moins l'intra");
                encoder.restore_macroblock(&pixels, x, y);
                encoder.codec.pre = pre;
                match intra {
                    Some(predict) => {
                        let index = if predict { 7 } else { 6 };
                        out.put(tables.mv[index].0, tables.mv[index].1);
                    }
                    None => encoder.codec.motion[col + 2] = vector,
                }
                trial.tokens.write(&mut out);
            }
        }
    }
    let mut codec = encoder.codec;
    codec.pics[codec.current] = encoder.dst;
    codec.current = (codec.current + 1) % 6;
    (codec, out.finish())
}

/// Au-delà, le code du jeu ne rend plus la même image que le décodeur Rust.
const HIGHEST_QUANTIZER: i32 = 51;

/// Le son déjà dans une vidéo : pour chaque image, le nombre de blocs et leurs octets.
fn read_original(original: &[u8]) -> Result<Vec<(usize, Vec<u8>)>, String> {
    let bytes: Arc<[u8]> = original.into();
    let (mut mods, _) = Mods::open(bytes.clone())?;
    let per_block = match mods.codec {
        1 => 20,
        2 => 40,
        4 => 512,
        _ => 0,
    } * mods.nch;
    let mut packets = Vec::new();
    let mut pos = mods.first;
    for _ in 0..mods.frames {
        if pos + 4 > mods.end {
            break;
        }
        let word = le32(&bytes, pos);
        let (size, blocks) = ((word >> 14) as usize, (word & 0x3FFF) as usize);
        let start = pos + 4;
        if start + size > bytes.len() {
            break;
        }
        let chunk = &bytes[start..start + size];
        let consumed = mods.decoder.decode(chunk).map_err(str::to_string)?;
        let at = consumed + if chunk[1] & 0x80 != 0 { 4 } else { 0 };
        let len = blocks * per_block;
        packets.push((blocks, chunk.get(at..at + len).map_or(Vec::new(), <[u8]>::to_vec)));
        pos = start + size;
    }
    Ok(packets)
}

/// Écrit une vidéo MODS à partir d'images RGBA à la taille de l'original, en reprenant ses réglages et le rythme de son son.
/// `sound` : le nouveau son, entrelacé, à la fréquence et avec les voies de l'original. `None` : le son de l'original reste, tel quel.
pub fn encode_like(original: &[u8], images: impl Iterator<Item = Result<Vec<u8>, String>>, sound: Option<&[i16]>) -> Result<(Vec<u8>, Written), String> {
    let bytes: Arc<[u8]> = original.into();
    let (reader, info) = Mods::open(bytes)?;
    let (w, h, nch, first, end, codec_id) = (info.width as usize, info.height as usize, reader.nch, reader.first, reader.end, reader.codec);
    let info_at = le32(original, 0x24) as usize;
    let table_at = le32(original, 0x28) as usize;
    let table_count = le32(original, 0x2C) as usize;
    let limit = le32(original, 0x20) as usize;
    let limit = if limit < 64 { 0x20000 } else { limit };
    let seeks: Vec<u32> = (0..table_count).map(|i| le32(original, table_at + i * 8)).collect();
    let schedule = read_original(original)?;
    if codec_id == 2 && sound.is_some() {
        return Err("le son FastAudio ne sait pas encore s'ecrire".into());
    }
    let mut track = match (sound, codec_id) {
        (Some(samples), 1) => Some(SoundTrack::new(samples, nch, &original[info_at..])),
        _ => None,
    };
    let mut pcm_at = 0usize;
    let stream_size: usize = end.min(table_at).saturating_sub(first);
    let per_frame = stream_size / schedule.len().max(1);

    let mut out = original[..first].to_vec();
    let mut written = Written::default();
    let mut new_seeks = Vec::new();
    let mut bank = limit as isize;
    let mut quantizer = 30i32;
    let mut codec = Mobiclip::new(w as i32, h as i32);
    for (index, image) in images.enumerate() {
        let image = image?;
        if image.len() != w * h * 4 {
            return Err(format!("image de {} octets, {} attendus", image.len(), w * h * 4));
        }
        let source = rgba_to_pic(&image, w, h);
        let blocks = schedule.get(index % schedule.len().max(1)).map_or(0, |s| s.0);
        let audio = match (sound, codec_id) {
            (_, 0) => Vec::new(),
            (Some(_), 1) => track.as_mut().expect("piste").packet(blocks).map_err(str::to_string)?,
            (Some(samples), 4) => {
                // Bloc de 256 échantillons par voie, voie après voie
                let mut pcm = vec![0u8; blocks * nch * 512];
                for b in 0..blocks {
                    for c in 0..nch {
                        for s in 0..256 {
                            let value = samples.get((pcm_at + b * 256 + s) * nch + c).copied().unwrap_or(0);
                            pcm[(b * nch + c) * 512 + s * 2..][..2].copy_from_slice(&value.to_le_bytes());
                        }
                    }
                }
                pcm_at += blocks * 256;
                pcm
            }
            (None, _) => match schedule.get(index) {
                Some(p) if !p.1.is_empty() || blocks == 0 => p.1.clone(),
                _ if codec_id == 1 => vec_silence(blocks, nch),
                _ if codec_id == 4 => vec![0; blocks * nch * 512],
                // FastAudio : le silence n'est pas une suite de zéros, on répète plutôt rien
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        let blocks = if audio.is_empty() { 0 } else { blocks };
        let keyframe = index == 0 || seeks.iter().any(|f| *f as usize == index);
        let budget = (per_frame as isize + bank).clamp(limit as isize / 4, limit as isize) as usize;
        // Aligné sur 4 octets, en comptant le mot de tête du paquet suivant ; une image clé garde 4 octets que le jeu saute
        let room = |bits: usize| (bits + if keyframe { 4 } else { 0 } + audio.len()).div_ceil(4) * 4 + 4;
        let mut chosen = quantizer;
        let (picture, bits) = loop {
            let (picture, bits) = encode_picture(clone_codec(&codec), clone_pic(&source), keyframe, chosen);
            if room(bits.len()) <= budget || chosen >= HIGHEST_QUANTIZER {
                break (picture, bits);
            }
            chosen = (chosen + 2).min(HIGHEST_QUANTIZER);
        };
        let size = room(bits.len()) - 4;
        // Le tampon du jeu vaut le maximum de l'en-tête plus 0x400 : la marge ne sert qu'aux images impossibles à faire tenir autrement
        if size + 4 > limit + 0x3F0 {
            return Err(format!("image {index} : {} octets, le jeu en attend {limit} au plus", size + 4));
        }
        codec = picture;
        bank = (bank + per_frame as isize - (size + 4) as isize).clamp(-(limit as isize), limit as isize * 4);
        // Le quantificateur suit ce qu'il reste à dépenser, pas ce que coûte cette image-ci (une image clé paie pour toutes)
        quantizer = if keyframe {
            chosen
        } else if size + 4 > per_frame + per_frame / 4 && bank < 0 {
            (chosen + 1).min(HIGHEST_QUANTIZER)
        } else if size + 4 < per_frame - per_frame / 4 && bank > 0 {
            (chosen - 1).max(18)
        } else {
            chosen
        };
        written.worst_level = written.worst_level.max(if chosen >= 48 { 6 } else if chosen >= 42 { 5 } else { 0 });
        if keyframe {
            new_seeks.push((index as u32, out.len() as u32));
        }
        out.extend_from_slice(&(((size as u32) << 14) | blocks as u32).to_le_bytes());
        let start = out.len();
        out.extend_from_slice(&bits);
        if keyframe {
            out.extend_from_slice(&[0; 4]);
        }
        out.extend_from_slice(&audio);
        out.resize(start + size, 0);
        written.frames += 1;
    }
    if written.frames == 0 {
        return Err("aucune image dans la video".into());
    }
    let new_info = out.len();
    if info_at >= first && codec_id == 1 {
        out.extend_from_slice(&original[info_at..info_at + nch * AUDIO_EXTRA]);
    }
    let new_table = out.len();
    for (frame, at) in &new_seeks {
        out.extend_from_slice(&frame.to_le_bytes());
        out.extend_from_slice(&at.to_le_bytes());
    }
    let patch = |out: &mut Vec<u8>, at: usize, value: u32| out[at..at + 4].copy_from_slice(&value.to_le_bytes());
    patch(&mut out, 8, written.frames);
    if info_at >= first && codec_id == 1 {
        patch(&mut out, 0x24, new_info as u32);
    }
    patch(&mut out, 0x28, new_table as u32);
    patch(&mut out, 0x2C, new_seeks.len() as u32);
    written.size = out.len();
    Ok((out, written))
}

fn clone_pic(pic: &Pic) -> Pic {
    [pic[0].clone(), pic[1].clone(), pic[2].clone()]
}

fn clone_codec(codec: &Mobiclip) -> Mobiclip {
    Mobiclip { w: codec.w, h: codec.h, pics: codec.pics.clone(), current: codec.current, moflex: codec.moflex, dct_tab: codec.dct_tab, quantizer: codec.quantizer, qtab: codec.qtab, pre: codec.pre, motion: codec.motion.clone() }
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

    /// Un fichier minimal, sans son : un paquet vide pour que l'écriture reprenne ses réglages.
    fn empty_mods(frames: u32, w: u32, h: u32, limit: u32) -> Vec<u8> {
        let mut out = b"MODSN3\n\0".to_vec();
        for value in [frames, w, h, 15 << 24, 0, 0, limit, 0, 0x34, 1] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(b"HE\0\0");
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0x34u32.to_le_bytes());
        out
    }

    fn psnr(a: &[u8], b: &[u8]) -> f32 {
        let total: u64 = a.iter().zip(b).map(|(x, y)| (*x as i64 - *y as i64).unsigned_abs().pow(2)).sum();
        10.0 * (255f32 * 255.0 / (total as f32 / a.len() as f32).max(0.0001)).log10()
    }

    #[test]
    fn a_written_video_reads_back_without_errors_and_looks_the_same() {
        let (w, h) = (64, 48);
        let images = synthetic(10, w, h);
        let original = empty_mods(0, w as u32, h as u32, 0x8000);
        let (bytes, written) = encode_like(&original, images.iter().map(|i| Ok(i.clone())), None).unwrap();
        assert_eq!(written.frames, 10);
        let mut video = crate::Video::open(bytes).unwrap();
        let (mut pcm, mut frame, mut count) = (Vec::new(), Vec::new(), 0);
        while video.advance(&mut pcm) {
            video.rgba(&mut frame);
            let quality = psnr(&frame, &images[count]);
            assert!(quality > 25.0, "image {count} : {quality} dB");
            count += 1;
        }
        assert_eq!(count, 10);
        assert_eq!(video.errors(), (0, None));
    }
}
