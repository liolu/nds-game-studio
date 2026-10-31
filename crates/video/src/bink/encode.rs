//! L'écriture d'une vidéo Bink 1 (révision `i`) : chaque bloc passe par les calculs du décodeur d'à côté.
//!
//! Le fichier garde l'en-tête de l'original et sa plus grande image : le jeu réserve sa mémoire d'après elle. Un bloc de mouvement ne lit
//! jamais hors de l'image, et une image clé ne lit jamais l'image précédente : le code du jeu y trouverait autre chose que Rust.

use super::*;
use crate::vx::Written;

/// Du bit de poids faible au plus fort (l'inverse de `Lsb`).
#[derive(Default, Clone)]
struct LsbWriter {
    bytes: Vec<u8>,
    current: u64,
    used: u32,
}

impl LsbWriter {
    fn put(&mut self, count: u32, value: u32) {
        if count == 0 {
            return;
        }
        self.current |= ((value as u64) & ((1u64 << count) - 1)) << self.used;
        self.used += count;
        while self.used >= 8 {
            self.bytes.push(self.current as u8);
            self.current >>= 8;
            self.used -= 8;
        }
    }

    fn bits(&self) -> usize {
        self.bytes.len() * 8 + self.used as usize
    }

    fn align32(&mut self) {
        let over = self.bits() % 32;
        if over != 0 {
            let mut left = 32 - over as u32;
            while left > 0 {
                let step = left.min(16);
                self.put(step, 0);
                left -= step;
            }
        }
    }

    /// Ajoute des bits déjà écrits (non alignés, derrière ceux-ci).
    fn extend(&mut self, other: &LsbWriter) {
        for byte in &other.bytes {
            self.put(8, *byte as u32);
        }
        if other.used > 0 {
            self.put(other.used, other.current as u32);
        }
    }
}

/// La table de 16 symboles d'un paquet : celle qui coûte le moins pour ces fréquences.
#[derive(Clone, Copy)]
struct WriteTree {
    vlc: usize,
    syms: [u8; 16],
    /// Combien de symboles sont écrits ; le reste suit dans l'ordre croissant.
    explicit: usize,
}

impl WriteTree {
    fn flat() -> WriteTree {
        WriteTree { vlc: 0, syms: std::array::from_fn(|i| i as u8), explicit: 0 }
    }

    fn write_head(&self, out: &mut LsbWriter) {
        out.put(4, self.vlc as u32);
        if self.vlc != 0 {
            out.put(1, 1);
            out.put(3, self.explicit as u32 - 1);
            for sym in &self.syms[..self.explicit] {
                out.put(4, *sym as u32);
            }
        }
    }

    fn write(&self, out: &mut LsbWriter, sym: u8) {
        let at = self.syms.iter().position(|s| *s == sym).expect("les 16 symboles");
        out.put(BINK_TREE_LENS[self.vlc][at] as u32, BINK_TREE_BITS[self.vlc][at] as u32);
    }

    fn choose(freq: &[u32; 16]) -> WriteTree {
        let total: u32 = freq.iter().sum();
        let mut best = (4 + 4 * total as u64, WriteTree::flat());
        let mut order: Vec<u8> = (0..16).collect();
        order.sort_by_key(|s| std::cmp::Reverse(freq[*s as usize]));
        let used = order.iter().filter(|s| freq[**s as usize] > 0).count();
        let explicit = used.clamp(1, 8);
        for vlc in 1..16 {
            let mut syms = [0u8; 16];
            let mut seen = [false; 16];
            for (i, sym) in order[..explicit].iter().enumerate() {
                syms[i] = *sym;
                seen[*sym as usize] = true;
            }
            let mut at = explicit;
            for sym in 0..16u8 {
                if !seen[sym as usize] {
                    syms[at] = sym;
                    at += 1;
                }
            }
            let mut cost = 8 + 4 * explicit as u64;
            for (i, sym) in syms.iter().enumerate() {
                cost += BINK_TREE_LENS[vlc][i] as u64 * freq[*sym as usize] as u64;
            }
            if cost < best.0 {
                best = (cost, WriteTree { vlc, syms, explicit });
            }
        }
        best.1
    }
}

/// Le contraire de `read_dct_coeffs` : `levels` rangés dans l'ordre du balayage (0 : le continu, à part).
fn write_dct(out: &mut LsbWriter, levels: &[i32; 64], quant: usize) {
    let biggest = levels[1..].iter().map(|l| l.unsigned_abs()).max().unwrap_or(0);
    if biggest == 0 {
        out.put(4, 0);
        out.put(4, quant as u32);
        return;
    }
    let mut bits = 31 - biggest.leading_zeros() as i32;
    out.put(4, bits as u32 + 1);
    let mut coef = [0i32; 128];
    let mut mode = [0i32; 128];
    let (mut start, mut end) = (64usize, 64usize);
    for (c, m) in [(4, 0), (24, 0), (44, 0), (1, 3), (2, 3), (3, 3)] {
        coef[end] = c;
        mode[end] = m;
        end += 1;
    }
    let significant = |c: i32, count: i32, bits: i32| (c..c + count).any(|i| levels[i as usize].unsigned_abs() >= 1 << bits);
    let write_value = |out: &mut LsbWriter, value: i32, bits: i32| {
        let size = value.unsigned_abs();
        if bits > 0 {
            out.put(bits as u32, size & ((1 << bits) - 1));
        }
        out.put(1, u32::from(value < 0));
    };
    while bits >= 0 {
        let mut pos = start;
        while pos < end {
            let (c, m) = (coef[pos], mode[pos]);
            if (m | c) == 0 {
                pos += 1;
                continue;
            }
            let group = match m {
                0 => 20,
                1 => 16,
                2 => 4,
                _ => 1,
            };
            if !significant(c, group, bits) {
                out.put(1, 0);
                pos += 1;
                continue;
            }
            out.put(1, 1);
            let mut ccoef = c;
            match m {
                0 | 2 => {
                    if m == 0 {
                        coef[pos] = ccoef + 4;
                        mode[pos] = 1;
                    } else {
                        coef[pos] = 0;
                        mode[pos] = 0;
                        pos += 1;
                    }
                    for _ in 0..4 {
                        if levels[ccoef as usize].unsigned_abs() < 1 << bits {
                            out.put(1, 1);
                            start -= 1;
                            coef[start] = ccoef;
                            mode[start] = 3;
                        } else {
                            out.put(1, 0);
                            write_value(out, levels[ccoef as usize], bits);
                        }
                        ccoef += 1;
                    }
                }
                1 => {
                    mode[pos] = 2;
                    for _ in 0..3 {
                        ccoef += 4;
                        coef[end] = ccoef;
                        mode[end] = 2;
                        end += 1;
                    }
                }
                _ => {
                    write_value(out, levels[ccoef as usize], bits);
                    coef[pos] = 0;
                    mode[pos] = 0;
                    pos += 1;
                }
            }
        }
        bits -= 1;
    }
    out.put(4, quant as u32);
}

/// Ce que rend une colonne de `idct_transform` pour un seul coefficient, à l'échelle de 1.
fn basis() -> &'static ([[f32; 8]; 8], [f32; 8]) {
    static BASIS: OnceLock<([[f32; 8]; 8], [f32; 8])> = OnceLock::new();
    BASIS.get_or_init(|| {
        const SCALE: i32 = 1 << 16;
        let table: [[f32; 8]; 8] = std::array::from_fn(|k| {
            let mut line = [0; 8];
            line[k] = SCALE;
            let out = idct_transform(line);
            std::array::from_fn(|i| out[i] as f32 / SCALE as f32)
        });
        let norm = std::array::from_fn(|k| table[k].iter().map(|v| v * v).sum());
        (table, norm)
    })
}

/// La transformée directe qui répond à `idct` : `pixels[ligne * 8 + colonne]`, résultat rangé comme le bloc du décodeur.
fn forward(pixels: &[f32; 64]) -> [f32; 64] {
    let (table, norm) = basis();
    let mut mid = [0f32; 64];
    for y in 0..8 {
        for u in 0..8 {
            mid[y * 8 + u] = (0..8).map(|x| pixels[y * 8 + x] * table[u][x]).sum::<f32>() / norm[u];
        }
    }
    let mut out = [0f32; 64];
    for u in 0..8 {
        for v in 0..8 {
            out[u + 8 * v] = 256.0 * (0..8).map(|y| mid[y * 8 + u] * table[v][y]).sum::<f32>() / norm[v];
        }
    }
    out
}

#[derive(Clone, Copy)]
enum Choice {
    Skip,
    Fill(u8),
    Motion(i8, i8),
    /// Le continu, et les codes des autres coefficients.
    Intra(i32),
    Inter(i8, i8, i32),
}

struct Coded {
    choice: Choice,
    dct: LsbWriter,
    pixels: [u8; 64],
}

const TYPE_SKIP: u8 = 0;
const TYPE_MOTION: u8 = 2;
const TYPE_INTRA: u8 = 5;
const TYPE_FILL: u8 = 6;
const TYPE_INTER: u8 = 7;

struct Plane<'a> {
    source: &'a [u8],
    previous: &'a [u8],
    stride: usize,
    /// En blocs de 8x8.
    bw: usize,
    bh: usize,
}

impl Plane<'_> {
    fn distance(&self, at: usize, reference: usize) -> i64 {
        let mut sum = 0;
        for j in 0..8 {
            for i in 0..8 {
                sum += (self.source[at + j * self.stride + i] as i64 - self.previous[reference + j * self.stride + i] as i64).abs();
            }
        }
        sum
    }

    fn search(&self, bx: usize, by: usize) -> (i8, i8) {
        let at = |x: i32, y: i32| y as usize * self.stride + x as usize;
        let (x0, y0) = (bx as i32 * 8, by as i32 * 8);
        let (mx_limit, my_limit) = (self.bw as i32 * 8 - 8, self.bh as i32 * 8 - 8);
        let here = at(x0, y0);
        let usable = |dx: i32, dy: i32| (-15..=15).contains(&dx) && (-15..=15).contains(&dy) && x0 + dx >= 0 && y0 + dy >= 0 && x0 + dx <= mx_limit && y0 + dy <= my_limit;
        let mut best = (0, 0);
        let mut best_cost = self.distance(here, here);
        let mut step = 8;
        while step > 0 {
            let center = best;
            let mut moved = false;
            for (dx, dy) in [(step, 0), (-step, 0), (0, step), (0, -step)] {
                let (nx, ny) = (center.0 + dx, center.1 + dy);
                if usable(nx, ny) {
                    let cost = self.distance(here, at(x0 + nx, y0 + ny));
                    if cost < best_cost {
                        best_cost = cost;
                        best = (nx, ny);
                        moved = true;
                    }
                }
            }
            if !moved {
                step /= 2;
            }
        }
        (best.0 as i8, best.1 as i8)
    }

    fn block(&self, at: usize) -> [f32; 64] {
        std::array::from_fn(|i| self.source[at + (i / 8) * self.stride + i % 8] as f32)
    }

    fn cost_of(&self, at: usize, pixels: &[u8; 64], bits: usize, lambda: f32) -> f32 {
        let mut sum = 0i64;
        for (i, &value) in pixels.iter().enumerate() {
            sum += (self.source[at + (i / 8) * self.stride + i % 8] as i64 - value as i64).pow(2);
        }
        sum as f32 + bits as f32 * lambda
    }
}

/// Les coefficients d'un bloc pour ce quantificateur : (continu, autres dans l'ordre du balayage, pixels ajoutés).
fn quantized(values: &[f32; 64], table: &[i32; 64], deadzone: f32, dc_limit: i32) -> (i32, [i32; 64], [i32; 64]) {
    let dc = ((values[0] * 2048.0 / table[0] as f32).round() as i32).clamp(-dc_limit, dc_limit);
    let mut levels = [0i32; 64];
    let mut block = [0i32; 64];
    block[0] = dc.wrapping_mul(table[0]) >> 11;
    for i in 1..64 {
        let at = BINK_SCAN[i] as usize;
        let scaled = values[at].abs() * 2048.0 / table[i] as f32;
        let level = ((scaled + deadzone) as i32).min(32000) * if values[at] < 0.0 { -1 } else { 1 };
        levels[i] = level;
        block[at] = level.wrapping_mul(table[i]) >> 11;
    }
    (dc, levels, idct(&block))
}

/// Les choix d'un bloc : le coût le plus bas parmi tout ce que permet ce type d'image.
fn encode_block(plane: &Plane, bx: usize, by: usize, key: bool, quant: usize, lambda: f32) -> Coded {
    let at = by * 8 * plane.stride + bx * 8;
    let source = plane.block(at);
    let mut best: Option<(f32, Coded)> = None;
    let mut consider = |coded: Coded, bits: usize| {
        let cost = plane.cost_of(at, &coded.pixels, bits, lambda);
        if best.as_ref().is_none_or(|b| cost < b.0) {
            best = Some((cost, coded));
        }
    };
    let mean = (source.iter().sum::<f32>() / 64.0).round().clamp(0.0, 255.0) as u8;
    consider(Coded { choice: Choice::Fill(mean), dct: LsbWriter::default(), pixels: [mean; 64] }, 4 + 8);
    if !key {
        let copied = |offset: usize| -> [u8; 64] { std::array::from_fn(|i| plane.previous[offset + (i / 8) * plane.stride + i % 8]) };
        consider(Coded { choice: Choice::Skip, dct: LsbWriter::default(), pixels: copied(at) }, 3);
        let (mx, my) = plane.search(bx, by);
        let moved = (at as isize + mx as isize + my as isize * plane.stride as isize) as usize;
        let reference = copied(moved);
        if mx != 0 || my != 0 {
            consider(Coded { choice: Choice::Motion(mx, my), dct: LsbWriter::default(), pixels: reference }, 3 + 12);
        }
        // Le résidu après le meilleur mouvement ; rien d'utile s'il ne reste aucun écart
        let residue: [f32; 64] = std::array::from_fn(|i| source[i] - reference[i] as f32);
        if residue.iter().any(|v| v.abs() > 2.0) {
            let (dc, levels, pixels) = quantized(&forward(&residue), &BINK_INTER_QUANT[quant], 0.2, 1023);
            let mut dct = LsbWriter::default();
            write_dct(&mut dct, &levels, quant);
            let bits = 3 + 12 + 6 + dct.bits();
            // Le décodeur ajoute sur 8 bits : un dépassement revient de l'autre côté, le coût le verra
            let result: [u8; 64] = std::array::from_fn(|i| reference[i].wrapping_add(pixels[i] as u8));
            consider(Coded { choice: Choice::Inter(mx, my, dc), dct, pixels: result }, bits);
        }
    }
    let (dc, levels, pixels) = quantized(&forward(&source), &BINK_INTRA_QUANT[quant], 0.33, 2047);
    let mut dct = LsbWriter::default();
    write_dct(&mut dct, &levels, quant);
    let bits = 3 + 8 + dct.bits();
    consider(Coded { choice: Choice::Intra(dc.max(0)), dct, pixels: std::array::from_fn(|i| pixels[i] as u8) }, bits);
    best.expect("le remplissage").1
}

/// Combien de valeurs écrire en tête de chaque rangée, comme `Bundle::count` les lit : une rangée sans valeur prend d'avance celles de la
/// prochaine qui en a, car un compte de 0 arrête le paquet pour tout le reste du plan.
struct Counts {
    rows: Vec<usize>,
    pending: usize,
    dead: bool,
    len: u32,
}

impl Counts {
    fn new(rows: Vec<usize>, len: u32) -> Counts {
        Counts { rows, pending: 0, dead: false, len }
    }

    /// La rangée dont il faut écrire les valeurs maintenant, s'il y en a. Écrit le compte.
    fn start(&mut self, out: &mut LsbWriter, row: usize) -> Option<usize> {
        if self.dead || self.pending > 0 {
            return None;
        }
        match (row..self.rows.len()).find(|r| self.rows[*r] > 0) {
            Some(source) => {
                out.put(self.len, self.rows[source] as u32);
                self.pending = self.rows[source];
                Some(source)
            }
            None => {
                out.put(self.len, 0);
                self.dead = true;
                None
            }
        }
    }

    fn finish(&mut self, row: usize) {
        self.pending = self.pending.saturating_sub(self.rows[row]);
    }
}

fn run_type_tokens(values: &[u8]) -> Vec<u8> {
    // Le type puis ses répétitions : 12 à 15 répètent le dernier
    let mut tokens = Vec::new();
    let mut last = 0u8;
    let mut at = 0;
    while at < values.len() {
        let same = values[at..].iter().take_while(|v| **v == last).count();
        if same >= 4 {
            let code = RLE_LENS.iter().rposition(|l| *l <= same).expect("au moins 4");
            tokens.push(12 + code as u8);
            at += RLE_LENS[code];
        } else {
            tokens.push(values[at]);
            last = values[at];
            at += 1;
        }
    }
    tokens
}

/// Un plan entier : ses bits, et le plan tel que le verra le jeu.
#[allow(clippy::too_many_arguments)]
fn encode_plane(out: &mut LsbWriter, plane: &Plane, width: usize, key: bool, quant: usize, lambda: f32, result: &mut [u8]) {
    let (bw, bh) = (plane.bw, plane.bh);
    let mut grid = Vec::with_capacity(bw * bh);
    for by in 0..bh {
        for bx in 0..bw {
            let coded = encode_block(plane, bx, by, key, quant, lambda);
            let at = by * 8 * plane.stride + bx * 8;
            for j in 0..8 {
                result[at + j * plane.stride..at + j * plane.stride + 8].copy_from_slice(&coded.pixels[j * 8..j * 8 + 8]);
            }
            grid.push(coded);
        }
    }
    let row_of = |by: usize| &grid[by * bw..(by + 1) * bw];
    let types: Vec<Vec<u8>> = (0..bh)
        .map(|by| {
            row_of(by)
                .iter()
                .map(|c| match c.choice {
                    Choice::Skip => TYPE_SKIP,
                    Choice::Fill(_) => TYPE_FILL,
                    Choice::Motion(..) => TYPE_MOTION,
                    Choice::Intra(_) => TYPE_INTRA,
                    Choice::Inter(..) => TYPE_INTER,
                })
                .collect()
        })
        .collect();
    let colors: Vec<Vec<u8>> = (0..bh).map(|by| row_of(by).iter().filter_map(|c| if let Choice::Fill(v) = c.choice { Some(v) } else { None }).collect()).collect();
    let offsets = |pick: fn(i8, i8) -> i8| -> Vec<Vec<i8>> {
        (0..bh)
            .map(|by| {
                row_of(by)
                    .iter()
                    .filter_map(|c| match c.choice {
                        Choice::Motion(x, y) | Choice::Inter(x, y, _) => Some(pick(x, y)),
                        _ => None,
                    })
                    .collect()
            })
            .collect()
    };
    let xs = offsets(|x, _| x);
    let ys = offsets(|_, y| y);
    let intra_dc: Vec<Vec<i32>> = (0..bh).map(|by| row_of(by).iter().filter_map(|c| if let Choice::Intra(dc) = c.choice { Some(dc) } else { None }).collect()).collect();
    let inter_dc: Vec<Vec<i32>> = (0..bh).map(|by| row_of(by).iter().filter_map(|c| if let Choice::Inter(_, _, dc) = c.choice { Some(dc) } else { None }).collect()).collect();

    // Les arbres, d'après tout le plan
    let mut type_freq = [0u32; 16];
    let type_tokens: Vec<Vec<u8>> = types.iter().map(|row| run_type_tokens(row)).collect();
    for token in type_tokens.iter().flatten() {
        type_freq[*token as usize] += 1;
    }
    let mut low_freq = [0u32; 16];
    for color in colors.iter().flatten() {
        low_freq[(color & 15) as usize] += 1;
    }
    let mut x_freq = [0u32; 16];
    let mut y_freq = [0u32; 16];
    for value in xs.iter().flatten() {
        x_freq[value.unsigned_abs() as usize] += 1;
    }
    for value in ys.iter().flatten() {
        y_freq[value.unsigned_abs() as usize] += 1;
    }
    let (type_tree, low_tree, x_tree, y_tree) = (WriteTree::choose(&type_freq), WriteTree::choose(&low_freq), WriteTree::choose(&x_freq), WriteTree::choose(&y_freq));
    let flat = WriteTree::flat();
    type_tree.write_head(out);
    flat.write_head(out);
    for _ in 0..16 {
        flat.write_head(out);
    }
    low_tree.write_head(out);
    flat.write_head(out);
    x_tree.write_head(out);
    y_tree.write_head(out);
    flat.write_head(out);

    let w8 = (width + 7) & !7;
    let width_bits = log2((w8 >> 3) + 511) + 1;
    let count_of = |rows: &[Vec<u8>]| rows.iter().map(Vec::len).collect::<Vec<_>>();
    let mut type_counts = Counts::new(count_of(&types), width_bits);
    let mut sub_counts = Counts::new(vec![0; bh], log2((w8 >> 4) + 511) + 1);
    let mut color_counts = Counts::new(count_of(&colors), log2(bw * 64 + 511) + 1);
    let mut pattern_counts = Counts::new(vec![0; bh], log2((bw << 3) + 511) + 1);
    let mut x_counts = Counts::new(xs.iter().map(Vec::len).collect(), width_bits);
    let mut y_counts = Counts::new(ys.iter().map(Vec::len).collect(), width_bits);
    let mut intra_counts = Counts::new(intra_dc.iter().map(Vec::len).collect(), width_bits);
    let mut inter_counts = Counts::new(inter_dc.iter().map(Vec::len).collect(), width_bits);
    let mut run_counts = Counts::new(vec![0; bh], log2(bw * 48 + 511) + 1);

    let write_dcs = |out: &mut LsbWriter, values: &[i32], signed: bool| {
        let first = values[0];
        out.put(if signed { 10 } else { 11 }, first.unsigned_abs());
        if signed && first != 0 {
            out.put(1, u32::from(first < 0));
        }
        let mut last = first;
        for chunk in values[1..].chunks(8) {
            let mut seen = last;
            let biggest = chunk.iter().map(|v| {
                let gap = (v - seen).unsigned_abs();
                seen = *v;
                gap
            });
            let size = biggest.max().map_or(0, |g| 32 - g.leading_zeros());
            out.put(4, size);
            for &value in chunk {
                let gap = value - last;
                if size > 0 {
                    out.put(size, gap.unsigned_abs());
                    if gap != 0 {
                        out.put(1, u32::from(gap < 0));
                    }
                }
                last = value;
            }
        }
    };
    let write_offsets = |out: &mut LsbWriter, values: &[i8], tree: &WriteTree| {
        let sign = |out: &mut LsbWriter, v: i8| {
            if v != 0 {
                out.put(1, u32::from(v < 0));
            }
        };
        if values.iter().all(|v| *v == values[0]) {
            out.put(1, 1);
            out.put(4, values[0].unsigned_abs() as u32);
            sign(out, values[0]);
        } else {
            out.put(1, 0);
            for &v in values {
                tree.write(out, v.unsigned_abs());
                sign(out, v);
            }
        }
    };

    for by in 0..bh {
        if let Some(source) = type_counts.start(out, by) {
            let row = &types[source];
            if row.iter().all(|t| *t == row[0]) {
                out.put(1, 1);
                out.put(4, row[0] as u32);
            } else {
                out.put(1, 0);
                for token in &type_tokens[source] {
                    type_tree.write(out, *token);
                }
            }
        }
        sub_counts.start(out, by);
        if let Some(source) = color_counts.start(out, by) {
            let row = &colors[source];
            let flat_tree = WriteTree::flat();
            if row.iter().all(|c| *c == row[0]) {
                out.put(1, 1);
                flat_tree.write(out, row[0] >> 4);
                low_tree.write(out, row[0] & 15);
            } else {
                out.put(1, 0);
                for color in row {
                    flat_tree.write(out, color >> 4);
                    low_tree.write(out, color & 15);
                }
            }
        }
        pattern_counts.start(out, by);
        if let Some(source) = x_counts.start(out, by) {
            write_offsets(out, &xs[source], &x_tree);
        }
        if let Some(source) = y_counts.start(out, by) {
            write_offsets(out, &ys[source], &y_tree);
        }
        if let Some(source) = intra_counts.start(out, by) {
            write_dcs(out, &intra_dc[source], false);
        }
        if let Some(source) = inter_counts.start(out, by) {
            write_dcs(out, &inter_dc[source], true);
        }
        run_counts.start(out, by);
        for coded in row_of(by) {
            out.extend(&coded.dct);
        }
        for counts in [&mut type_counts, &mut color_counts, &mut x_counts, &mut y_counts, &mut intra_counts, &mut inter_counts] {
            counts.finish(by);
        }
    }
    out.align32();
}

/// Une image en plans du décodeur : `Y`, puis cB et cR à demi-résolution, chacun étendu jusqu'au bout de ses blocs.
fn rgba_to_planes(rgba: &[u8], w: usize, h: usize, strides: &[usize], rows: &[usize]) -> Vec<Vec<u8>> {
    let mut planes: Vec<Vec<u8>> = (0..3).map(|p| vec![0u8; strides[p] * rows[p]]).collect();
    let px = |x: usize, y: usize| &rgba[(y.min(h - 1) * w + x.min(w - 1)) * 4..][..3];
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    for y in 0..rows[0] {
        for x in 0..strides[0] {
            let p = px(x, y);
            let luma = (66 * p[0] as i32 + 129 * p[1] as i32 + 25 * p[2] as i32 + 128) >> 8;
            planes[0][y * strides[0] + x] = (luma + 16).clamp(0, 255) as u8;
        }
    }
    for y in 0..rows[1] {
        for x in 0..strides[1] {
            let (mut u, mut v) = (0, 0);
            for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                let p = px(x.min(cw - 1) * 2 + dx, y.min(ch - 1) * 2 + dy);
                u += -38 * p[0] as i32 - 74 * p[1] as i32 + 112 * p[2] as i32;
                v += 112 * p[0] as i32 - 94 * p[1] as i32 - 18 * p[2] as i32;
            }
            planes[1][y * strides[1] + x] = ((u + 512) / 1024 + 128).clamp(0, 255) as u8;
            planes[2][y * strides[2] + x] = ((v + 512) / 1024 + 128).clamp(0, 255) as u8;
        }
    }
    planes
}

/// L'échelle, en pixels, du pas de ce quantificateur : pour peser les bits contre les écarts.
fn lambda_of(quant: usize) -> f32 {
    let step = BINK_INTER_QUANT[quant][4] as f32 / BINK_INTER_QUANT[0][4] as f32;
    2.0 * step * step
}

/// Une image : ses octets (le mot de tête, puis les plans), et ses plans décodés.
fn encode_frame(source: &[Vec<u8>], previous: &[Vec<u8>], strides: &[usize], w: usize, h: usize, key: bool, quant: usize) -> (Vec<u8>, Vec<Vec<u8>>) {
    let lambda = lambda_of(quant);
    let mut out = LsbWriter::default();
    out.put(16, 0);
    out.put(16, 0);
    let mut result: Vec<Vec<u8>> = source.iter().map(|p| vec![0; p.len()]).collect();
    let mut luma_end = 0;
    // Comme la lecture : la luminance, puis le plan 2, puis le plan 1
    for plane in [0usize, 2, 1] {
        let (width, bw, bh) = if plane == 0 { (w.max(8), (w + 7) >> 3, (h + 7) >> 3) } else { ((w >> 1).max(8), (w + 15) >> 4, (h + 15) >> 4) };
        let current = Plane { source: &source[plane], previous: &previous[plane], stride: strides[plane], bw, bh };
        encode_plane(&mut out, &current, width, key, quant, lambda, &mut result[plane]);
        if plane == 0 {
            luma_end = out.bytes.len();
        }
    }
    let mut bytes = out.bytes;
    bytes[..4].copy_from_slice(&(luma_end as u32).to_le_bytes());
    (bytes, result)
}

// --------------------------------------------------------------------------------------------
//  Le son
// --------------------------------------------------------------------------------------------

struct SoundWriter<'a> {
    samples: &'a [i16],
    audio: BinkAudio,
    cosines: Vec<f32>,
    at: usize,
}

impl<'a> SoundWriter<'a> {
    fn new(samples: &'a [i16], rate: u32, channels: usize) -> SoundWriter<'a> {
        let audio = BinkAudio::new(rate, channels);
        let n = audio.frame_len;
        let unit = std::f64::consts::PI / (2.0 * n as f64);
        SoundWriter { samples, cosines: (0..4 * n).map(|i| (unit * i as f64).cos() as f32).collect(), audio, at: 0 }
    }

    fn sample(&self, channel: usize, index: usize) -> f32 {
        *self.samples.get(index * self.audio.channels + channel).unwrap_or(&0) as f32 / 32768.0
    }

    /// La transformée directe d'un morceau, à l'échelle que lit le décodeur : `coeffs[0]` y est doublé.
    fn transform(&self, channel: usize) -> Vec<f32> {
        let n = self.audio.frame_len;
        let input: Vec<f32> = (0..n).map(|k| self.sample(channel, self.at + k)).collect();
        let mut out = vec![0f32; n];
        for (i, value) in out.iter_mut().enumerate() {
            let mut sum = 0f32;
            for (k, sample) in input.iter().enumerate() {
                sum += sample * self.cosines[(i * (2 * k + 1)) % (4 * n)];
            }
            *value = sum;
        }
        out[0] *= 0.5;
        out
    }

    fn write_float(out: &mut LsbWriter, value: f32) {
        let size = value.abs();
        let mut power = 0i32;
        while power < 31 && size >= 2f32.powi(power) {
            power += 1;
        }
        let mantissa = (size * 2f32.powi(23 - power)).round().min(((1u32 << 23) - 1) as f32) as u32;
        out.put(5, power as u32);
        out.put(23, mantissa);
        out.put(1, u32::from(value < 0.0));
    }

    /// Un canal d'un bloc, à ce niveau de bruit (plus grand : moins de bits).
    fn write_channel(&self, out: &mut LsbWriter, coeffs: &[f32], noise: f32) {
        let audio = &self.audio;
        let n = audio.frame_len;
        Self::write_float(out, coeffs[0] / audio.root);
        Self::write_float(out, coeffs[1] / audio.root);
        let mut chosen = [0usize; 25];
        for (band, slot) in chosen.iter_mut().enumerate().take(audio.num_bands) {
            let (from, to) = (audio.bands[band], audio.bands[band + 1].min(n));
            let width = to.saturating_sub(from).max(1);
            let energy: f32 = coeffs[from.min(n)..to].iter().map(|v| v * v).sum::<f32>() / width as f32;
            // Le bruit suit à peine la force de la bande : le plus fort masque un peu le reste
            let step = noise * (energy.sqrt() / audio.root + 1.0).powf(0.25) * audio.root;
            let index = ((step / audio.root).ln() / 0.152_891_65).round();
            *slot = index.clamp(0.0, 95.0) as usize;
            out.put(8, *slot as u32);
        }
        // Comme la lecture : la bande de chaque coefficient
        let quant_at: Vec<f32> = {
            let mut table = vec![0f32; n];
            let (mut k, mut q) = (0, audio.quant[chosen[0]]);
            for (i, slot) in table.iter_mut().enumerate().skip(2) {
                while k < audio.num_bands && audio.bands[k] <= i {
                    q = audio.quant[chosen[k]];
                    k += 1;
                }
                *slot = q;
            }
            table
        };
        let level = |i: usize| -> i32 { (coeffs[i] / quant_at[i]).round() as i32 };
        let mut i = 2;
        while i < n {
            let width_of = |from: usize, to: usize| (from..to).map(|c| 32 - level(c).unsigned_abs().min(0x7FFF).leading_zeros()).max().unwrap_or(0);
            let base_end = (i + 8).min(n);
            let width = width_of(i, base_end);
            // La suite la plus longue qui tient dans cette largeur
            let run = RLE_TAB.iter().rposition(|groups| {
                let end = (i + groups * 8).min(n);
                end > base_end && width_of(i, end) == width
            });
            let end = match run {
                Some(code) => {
                    out.put(1, 1);
                    out.put(4, code as u32);
                    (i + RLE_TAB[code] * 8).min(n)
                }
                None => {
                    out.put(1, 0);
                    base_end
                }
            };
            out.put(4, width);
            if width > 0 {
                for c in i..end {
                    let value = level(c).clamp(-0x7FFF, 0x7FFF);
                    out.put(width, value.unsigned_abs());
                    if value != 0 {
                        out.put(1, u32::from(value < 0));
                    }
                }
            }
            i = end;
        }
    }

    /// Un bloc d'au plus `budget` octets, le plus fidèle possible.
    fn block(&mut self, budget: usize) -> Vec<u8> {
        let channels = self.audio.channels;
        let coeffs: Vec<Vec<f32>> = (0..channels).map(|c| self.transform(c)).collect();
        let write = |noise: f32| {
            let mut out = LsbWriter::default();
            out.put(2, 0);
            for c in &coeffs {
                self.write_channel(&mut out, c, noise);
            }
            out.align32();
            out.bytes
        };
        // Du bruit le plus bas au plus haut, jusqu'à tenir dans la place
        let mut noise = 0.02f32;
        let mut bytes = write(noise);
        while bytes.len() > budget && noise < 1_000_000.0 {
            noise *= 1.3;
            bytes = write(noise);
        }
        self.at += self.audio.frame_len - self.audio.overlap;
        bytes
    }
}

// --------------------------------------------------------------------------------------------
//  Le fichier
// --------------------------------------------------------------------------------------------

struct OldFrame {
    key: bool,
    audio: Vec<u8>,
}

/// Écrit une vidéo Bink à partir d'images RGBA à la taille de l'original, en reprenant ses réglages et le rythme de son son.
/// `sound` : le nouveau son, entrelacé, à la fréquence et avec les voies de l'original. `None` : le son de l'original reste, tel quel.
pub fn encode_like(original: &[u8], images: impl Iterator<Item = Result<Vec<u8>, String>>, sound: Option<&[i16]>) -> Result<(Vec<u8>, Written), String> {
    let bytes: Arc<[u8]> = original.into();
    let (bink, info) = Bink::open(bytes)?;
    if bink.has_alpha {
        return Err("Bink : une video avec une couche alpha ne sait pas encore s'ecrire".into());
    }
    if bink.version != b'i' {
        return Err("Bink : seule la revision i sait s'ecrire".into());
    }
    let tracks = bink.tracks;
    let (w, h) = (bink.w, bink.h);
    let largest = le32(original, 0xC) as usize;
    let table_at = 0x2C + 12 * tracks;
    let old_frames: Vec<OldFrame> = (0..bink.index.len() - 1)
        .map(|i| {
            let start = bink.index[i] as usize;
            let mut at = start;
            for _ in 0..tracks {
                at += 4 + le32(original, at) as usize;
            }
            OldFrame { key: le32(original, table_at + i * 4) & 1 != 0, audio: original[start..at.min(original.len())].to_vec() }
        })
        .collect();
    if sound.is_some() && (tracks != 1 || bink.audio.is_none()) {
        return Err("Bink : ce son ne sait pas s'ecrire".into());
    }
    let mut writer = sound.map(|samples| SoundWriter::new(samples, info.sample_rate, info.channels as usize));
    let block_bytes = writer.as_ref().map_or(0, |s| (s.audio.frame_len - s.audio.overlap) * s.audio.channels * 2);
    let old_audio: usize = old_frames.iter().map(|f| f.audio.len()).sum();
    let old_blocks: usize = if block_bytes > 0 { old_frames.iter().map(|f| le32(&f.audio, 4) as usize).sum::<usize>().div_ceil(block_bytes) } else { 0 };
    let per_block = old_audio.checked_div(old_blocks).unwrap_or(0);
    let old_stream = bink.index[bink.index.len() - 1] as usize - bink.index[0] as usize;
    let per_frame = old_stream / old_frames.len().max(1);

    let strides = bink.cur.strides.clone();
    let rows: Vec<usize> = (0..3).map(|p| bink.cur.planes[p].len() / strides[p]).collect();
    let mut previous: Vec<Vec<u8>> = (0..3).map(|p| vec![0u8; strides[p] * rows[p]]).collect();
    let mut frames: Vec<(bool, Vec<u8>)> = Vec::new();
    let mut written = Written::default();
    let mut bank = largest as isize;
    let mut quant = 8usize;
    for (index, image) in images.enumerate() {
        let image = image?;
        if image.len() != w * h * 4 {
            return Err(format!("image de {} octets, {} attendus", image.len(), w * h * 4));
        }
        let source = rgba_to_planes(&image, w, h, &strides, &rows);
        let old = old_frames.get(index % old_frames.len().max(1));
        let key = index == 0 || old_frames.get(index).is_some_and(|f| f.key);
        // Le son de cette image, au rythme de l'original
        let audio = match (&mut writer, old) {
            (Some(writer), Some(old)) => {
                let wanted = if old.audio.len() >= 8 { le32(&old.audio, 4) as usize } else { 0 };
                let blocks = wanted.div_ceil(block_bytes);
                let room = (largest / 2).checked_div(blocks).unwrap_or(0);
                let mut packet = Vec::new();
                for _ in 0..blocks {
                    packet.extend(writer.block(per_block.min(room).max(32)));
                }
                let mut out = Vec::with_capacity(packet.len() + 8);
                if blocks > 0 {
                    out.extend_from_slice(&(packet.len() as u32 + 4).to_le_bytes());
                    out.extend_from_slice(&((blocks * block_bytes) as u32).to_le_bytes());
                    out.extend_from_slice(&packet);
                } else {
                    out.extend_from_slice(&0u32.to_le_bytes());
                }
                out
            }
            (None, Some(_)) => match old_frames.get(index) {
                Some(frame) => frame.audio.clone(),
                // L'image dure plus longtemps que l'original : du silence
                None => vec![0; tracks * 4],
            },
            _ => vec![0; tracks * 4],
        };
        let budget = (per_frame as isize + bank).clamp(largest as isize / 2, largest as isize) as usize;
        let mut chosen = quant.min(if key { 6 } else { 15 });
        let (video, picture) = loop {
            let (video, picture) = encode_frame(&source, &previous, &strides, w, h, key, chosen);
            if audio.len() + video.len() <= budget || chosen >= 15 {
                break (video, picture);
            }
            chosen = (chosen + 1).min(15);
        };
        let size = audio.len() + video.len();
        if size > largest {
            return Err(format!("image {index} : {size} octets, le jeu en attend {largest} au plus"));
        }
        bank = (bank + per_frame as isize - size as isize).clamp(-(largest as isize), largest as isize * 4);
        // Le quantificateur suit ce qu'il reste à dépenser, sans s'emballer sur une image clé
        quant = if key {
            quant.max(chosen)
        } else if size > per_frame + per_frame / 4 && bank < 0 {
            (chosen + 1).min(15)
        } else if size < per_frame - per_frame / 4 && bank > 0 {
            chosen.saturating_sub(1)
        } else {
            chosen
        };
        written.worst_level = written.worst_level.max(if chosen >= 14 { 6 } else if chosen >= 12 { 5 } else { 0 });
        previous = picture;
        let mut frame = audio;
        frame.extend_from_slice(&video);
        frames.push((key, frame));
        written.frames += 1;
    }
    if written.frames == 0 {
        return Err("aucune image dans la video".into());
    }
    let mut out = original[..table_at].to_vec();
    let mut at = table_at + 4 * (frames.len() + 1);
    for (key, frame) in &frames {
        out.extend_from_slice(&(at as u32 | u32::from(*key)).to_le_bytes());
        at += frame.len();
    }
    out.extend_from_slice(&(at as u32).to_le_bytes());
    for (_, frame) in &frames {
        out.extend_from_slice(frame);
    }
    let patch = |out: &mut Vec<u8>, at: usize, value: u32| out[at..at + 4].copy_from_slice(&value.to_le_bytes());
    let total = out.len() as u32;
    patch(&mut out, 4, total - 8);
    patch(&mut out, 8, written.frames);
    patch(&mut out, 0x10, written.frames);
    written.size = out.len();
    Ok((out, written))
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

    /// Un fichier minimal, sans son.
    fn empty_bink(frames: u32, w: u32, h: u32, largest: u32) -> Vec<u8> {
        let mut out = b"BIKi".to_vec();
        for value in [0, frames, largest, frames, w, h, 15, 1, 0, 0] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        let first = 0x2C + 4 * (frames + 1);
        for i in 0..=frames {
            out.extend_from_slice(&(first + i * 4 + u32::from(i == 0)).to_le_bytes());
        }
        for _ in 0..frames {
            out.extend_from_slice(&0u32.to_le_bytes());
        }
        let total = out.len() as u32 - 8;
        out[4..8].copy_from_slice(&total.to_le_bytes());
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
        let original = empty_bink(2, w as u32, h as u32, 0x8000);
        let (bytes, written) = encode_like(&original, images.iter().map(|i| Ok(i.clone())), None).unwrap();
        assert_eq!(written.frames, 10);
        let mut video = crate::Video::open(bytes).unwrap();
        let (mut pcm, mut frame, mut count) = (Vec::new(), Vec::new(), 0);
        while video.advance(&mut pcm) {
            video.rgba(&mut frame);
            let quality = psnr(&frame, &images[count]);
            assert!(quality > 22.0, "image {count} : {quality} dB");
            count += 1;
        }
        assert_eq!(count, 10);
        assert_eq!(video.errors(), (0, None));
    }
}
