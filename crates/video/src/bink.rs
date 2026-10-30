//! Bink 1 (`.bik`) : le conteneur, l'image et le son DCT.
//! Portage de libavcodec/bink.c, binkdsp.c, binkaudio.c et libavformat/bink.c de FFmpeg (LGPL 2.1+).
//! Toutes les vidéos Bink des jeux DS connus sont en révision `i` : la révision `b`, à part, n'est pas portée.

use std::sync::{Arc, OnceLock};

use crate::bits::{Lsb, R, Vlc, clip8};
use crate::tables::*;
use crate::{Info, Stream, le16, le32};

const FLAG_ALPHA: u32 = 0x0010_0000;
const AUDIO_DCT: u16 = 0x1000;
const AUDIO_STEREO: u16 = 0x2000;

fn trees() -> &'static Vec<Vlc> {
    static TREES: OnceLock<Vec<Vlc>> = OnceLock::new();
    TREES.get_or_init(|| (0..16).map(|i| Vlc::from_codes_lsb(&BINK_TREE_LENS[i], &BINK_TREE_BITS[i])).collect())
}

const BLOCK_TYPES: usize = 0;
const SUB_BLOCK_TYPES: usize = 1;
const COLORS: usize = 2;
const PATTERN: usize = 3;
const X_OFF: usize = 4;
const Y_OFF: usize = 5;
const INTRA_DC: usize = 6;
const INTER_DC: usize = 7;
const RUN: usize = 8;
const SOURCES: usize = 9;

#[derive(Clone, Copy, Default)]
struct Tree {
    vlc: usize,
    syms: [u8; 16],
}

impl Tree {
    #[inline]
    fn huff(&self, gb: &mut Lsb) -> u8 {
        self.syms[trees()[self.vlc].read_lsb(gb) as usize & 15]
    }
}

#[derive(Default)]
struct Bundle {
    len: u32,
    tree: Tree,
    data: Vec<u8>,
    /// `None` : plus rien à lire dans cette ligne ni les suivantes.
    dec: Option<usize>,
    ptr: usize,
}

impl Bundle {
    /// Le nombre de valeurs à lire maintenant.
    fn count(&mut self, gb: &mut Lsb) -> usize {
        match self.dec {
            Some(dec) if dec <= self.ptr => {}
            _ => return 0,
        }
        let t = gb.get(self.len) as usize;
        if t == 0 {
            self.dec = None;
        }
        t
    }

    #[inline]
    fn next_u8(&mut self) -> u8 {
        let value = *self.data.get(self.ptr).unwrap_or(&0);
        self.ptr += 1;
        value
    }

    #[inline]
    fn next_i16(&mut self) -> i32 {
        let bytes = [*self.data.get(self.ptr).unwrap_or(&0), *self.data.get(self.ptr + 1).unwrap_or(&0)];
        self.ptr += 2;
        i16::from_le_bytes(bytes) as i32
    }
}

fn merge(gb: &mut Lsb, dst: &mut [u8], src: &[u8], size: usize) {
    let (mut a, mut b, mut left_a, mut left_b, mut out) = (0, size, size, size, 0);
    loop {
        if !gb.bit() {
            dst[out] = src[a];
            a += 1;
            left_a -= 1;
        } else {
            dst[out] = src[b];
            b += 1;
            left_b -= 1;
        }
        out += 1;
        if left_a == 0 || left_b == 0 {
            break;
        }
    }
    while left_a > 0 {
        dst[out] = src[a];
        out += 1;
        a += 1;
        left_a -= 1;
    }
    while left_b > 0 {
        dst[out] = src[b];
        out += 1;
        b += 1;
        left_b -= 1;
    }
}

fn read_tree(gb: &mut Lsb) -> R<Tree> {
    if gb.left() < 4 {
        return Err("arbre");
    }
    let mut tree = Tree { vlc: gb.get(4) as usize, syms: [0; 16] };
    if tree.vlc == 0 {
        tree.syms = std::array::from_fn(|i| i as u8);
        return Ok(tree);
    }
    if gb.bit() {
        let mut len = gb.get(3) as usize;
        let mut seen = [false; 16];
        for i in 0..=len {
            tree.syms[i] = gb.get(4) as u8;
            seen[tree.syms[i] as usize] = true;
        }
        for (i, &used) in seen.iter().enumerate() {
            if len >= 15 {
                break;
            }
            if !used {
                len += 1;
                tree.syms[len] = i as u8;
            }
        }
    } else {
        let len = gb.get(2) as usize;
        let mut input: [u8; 16] = std::array::from_fn(|i| i as u8);
        let mut output = [0u8; 16];
        for i in 0..=len {
            let size = 1 << i;
            for t in (0..16).step_by(size << 1) {
                merge(gb, &mut output[t..], &input[t..t + 2 * size], size);
            }
            std::mem::swap(&mut input, &mut output);
        }
        tree.syms = input;
    }
    Ok(tree)
}

fn log2(v: usize) -> u32 {
    usize::BITS - 1 - v.max(1).leading_zeros()
}

const RLE_LENS: [usize; 4] = [4, 8, 12, 32];

fn idct_transform(s: [i32; 8]) -> [i32; 8] {
    let mul = |x: i32, y: i32| x.wrapping_mul(y) >> 11;
    let a0 = s[0].wrapping_add(s[4]);
    let a1 = s[0].wrapping_sub(s[4]);
    let a2 = s[2].wrapping_add(s[6]);
    let a3 = mul(2896, s[2].wrapping_sub(s[6]));
    let a4 = s[5].wrapping_add(s[3]);
    let a5 = s[5].wrapping_sub(s[3]);
    let a6 = s[1].wrapping_add(s[7]);
    let a7 = s[1].wrapping_sub(s[7]);
    let b0 = a4.wrapping_add(a6);
    let b1 = mul(3784, a5.wrapping_add(a7));
    let b2 = mul(-5352, a5).wrapping_sub(b0).wrapping_add(b1);
    let b3 = mul(2896, a6.wrapping_sub(a4)).wrapping_sub(b2);
    let b4 = mul(2217, a7).wrapping_add(b3).wrapping_sub(b1);
    [
        a0.wrapping_add(a2).wrapping_add(b0),
        a1.wrapping_add(a3).wrapping_sub(a2).wrapping_add(b2),
        a1.wrapping_sub(a3).wrapping_add(a2).wrapping_add(b3),
        a0.wrapping_sub(a2).wrapping_sub(b4),
        a0.wrapping_sub(a2).wrapping_add(b4),
        a1.wrapping_sub(a3).wrapping_add(a2).wrapping_sub(b3),
        a1.wrapping_add(a3).wrapping_sub(a2).wrapping_sub(b2),
        a0.wrapping_add(a2).wrapping_sub(b0),
    ]
}

/// L'IDCT 8x8 : les valeurs de chaque pixel, à écrire ou à ajouter.
fn idct(block: &[i32; 64]) -> [i32; 64] {
    let mut temp = [0i32; 64];
    for i in 0..8 {
        if (1..8).all(|k| block[i + 8 * k] == 0) {
            for k in 0..8 {
                temp[i + 8 * k] = block[i];
            }
        } else {
            let out = idct_transform(std::array::from_fn(|k| block[i + 8 * k]));
            for k in 0..8 {
                temp[i + 8 * k] = out[k];
            }
        }
    }
    let mut result = [0i32; 64];
    for i in 0..8 {
        let out = idct_transform(std::array::from_fn(|k| temp[8 * i + k]));
        for k in 0..8 {
            result[8 * i + k] = out[k].wrapping_add(0x7F) >> 8;
        }
    }
    result
}

struct Picture {
    planes: Vec<Vec<u8>>,
    strides: Vec<usize>,
}

pub struct Bink {
    bytes: Arc<[u8]>,
    version: u8,
    w: usize,
    h: usize,
    has_alpha: bool,
    index: Vec<u32>,
    tracks: usize,
    bundle: Vec<Bundle>,
    col_high: [Tree; 16],
    col_last: usize,
    cur: Picture,
    last: Picture,
    audio: Option<BinkAudio>,
    frame: usize,
    file_end: usize,
}

impl Bink {
    pub fn open(bytes: Arc<[u8]>) -> Result<(Bink, Info), String> {
        if bytes.len() < 0x30 || &bytes[..3] != b"BIK" {
            return Err("Bink : en-tete".into());
        }
        let version = bytes[3];
        if version == b'b' {
            return Err("Bink : la revision b n'est pas portee".into());
        }
        let u = |at: usize| le32(&bytes, at);
        let (frames, w, h, num, den, flags, tracks) = (u(8) as usize, u(0x14) as usize, u(0x18) as usize, u(0x1C), u(0x20), u(0x24), u(0x28) as usize);
        if w == 0 || h == 0 || w > 7680 || h > 4800 || frames == 0 || frames > 1_000_000 || tracks > 256 || num == 0 || den == 0 {
            return Err(format!("Bink : {w}x{h}, {frames} images, {tracks} pistes"));
        }
        let mut at = 0x2C + if version == b'k' { 4 } else { 0 };
        let (mut rate, mut audio_flags) = (0, 0);
        if tracks > 0 {
            at += 4 * tracks;
            if at + 4 > bytes.len() {
                return Err("Bink : en-tete".into());
            }
            rate = le16(&bytes, at) as u32;
            audio_flags = le16(&bytes, at + 2);
            at += 8 * tracks;
        }
        if at + 4 * (frames + 1) > bytes.len() {
            return Err("Bink : table des images".into());
        }
        let index: Vec<u32> = (0..=frames).map(|i| le32(&bytes, at + i * 4) & !1).collect();
        let file_end = (u(4) as usize + 8).min(bytes.len());
        let has_alpha = flags & FLAG_ALPHA != 0;
        let (bw, bh) = ((w + 7) >> 3, (h + 7) >> 3);
        let bundle = (0..SOURCES).map(|_| Bundle { data: vec![0; bw * bh * 64], ..Bundle::default() }).collect();
        let picture = || {
            // Un bloc de 16x16 peut dépasser l'image d'une rangée
            let stride = |w: usize| ((w + 15) & !15) + 16;
            let rows = |h: usize| ((h + 15) & !15) + 16;
            let mut planes = vec![vec![0u8; stride(w) * rows(h)], vec![0u8; stride(w >> 1) * rows(h >> 1)], vec![0u8; stride(w >> 1) * rows(h >> 1)]];
            let mut strides = vec![stride(w), stride(w >> 1), stride(w >> 1)];
            if has_alpha {
                planes.push(vec![0u8; stride(w) * rows(h)]);
                strides.push(stride(w));
            }
            Picture { planes, strides }
        };
        let sound = tracks > 0 && audio_flags & AUDIO_DCT != 0 && rate > 0;
        let channels: usize = if audio_flags & AUDIO_STEREO != 0 { 2 } else { 1 };
        let info = Info { format: "Bink", width: w as u32, height: h as u32, frames: frames as u32, fps: num as f32 / den as f32, sample_rate: if sound { rate } else { 0 }, channels: if sound { channels as u16 } else { 0 } };
        let audio = sound.then(|| BinkAudio::new(rate, channels));
        let bink = Bink { bytes, version, w, h, has_alpha, index, tracks, bundle, col_high: Default::default(), col_last: 0, cur: picture(), last: picture(), audio, frame: 0, file_end };
        Ok((bink, info))
    }

    fn init_lengths(&mut self, width: usize, bw: usize) {
        let width = (width + 7) & !7;
        let b = &mut self.bundle;
        b[BLOCK_TYPES].len = log2((width >> 3) + 511) + 1;
        b[SUB_BLOCK_TYPES].len = log2((width >> 4) + 511) + 1;
        b[COLORS].len = log2(bw * 64 + 511) + 1;
        let dc = log2((width >> 3) + 511) + 1;
        b[INTRA_DC].len = dc;
        b[INTER_DC].len = dc;
        b[X_OFF].len = dc;
        b[Y_OFF].len = dc;
        b[PATTERN].len = log2((bw << 3) + 511) + 1;
        b[RUN].len = log2(bw * 48 + 511) + 1;
    }

    fn read_bundle(&mut self, gb: &mut Lsb, num: usize) -> R {
        if num == COLORS {
            for i in 0..16 {
                self.col_high[i] = read_tree(gb)?;
            }
            self.col_last = 0;
        }
        if num != INTRA_DC && num != INTER_DC {
            self.bundle[num].tree = read_tree(gb)?;
        }
        self.bundle[num].dec = Some(0);
        self.bundle[num].ptr = 0;
        Ok(())
    }

    fn read_runs(&mut self, gb: &mut Lsb) -> R {
        let b = &mut self.bundle[RUN];
        let t = b.count(gb);
        let Some(dec) = b.dec.filter(|_| t > 0) else { return Ok(()) };
        if dec + t > b.data.len() || gb.left() < 1 {
            return Err("suites");
        }
        if gb.bit() {
            let v = gb.get(4) as u8;
            b.data[dec..dec + t].fill(v);
        } else {
            let tree = b.tree;
            for slot in &mut b.data[dec..dec + t] {
                *slot = tree.huff(gb);
            }
        }
        b.dec = Some(dec + t);
        Ok(())
    }

    fn read_motion(&mut self, gb: &mut Lsb, num: usize) -> R {
        let b = &mut self.bundle[num];
        let t = b.count(gb);
        let Some(dec) = b.dec.filter(|_| t > 0) else { return Ok(()) };
        if dec + t > b.data.len() || gb.left() < 1 {
            return Err("mouvement");
        }
        if gb.bit() {
            let v = gb.get(4) as i32;
            let v = gb.sign(v) as u8;
            b.data[dec..dec + t].fill(v);
        } else {
            let tree = b.tree;
            for slot in &mut b.data[dec..dec + t] {
                let v = tree.huff(gb) as i32;
                *slot = gb.sign(v) as u8;
            }
        }
        b.dec = Some(dec + t);
        Ok(())
    }

    fn read_block_types(&mut self, gb: &mut Lsb, num: usize) -> R {
        let k = self.version == b'k';
        let b = &mut self.bundle[num];
        let mut t = b.count(gb);
        let Some(dec) = b.dec.filter(|_| t > 0) else { return Ok(()) };
        if k {
            t ^= 0xBB;
            if t == 0 {
                b.dec = None;
                return Ok(());
            }
        }
        let end = dec + t;
        if end > b.data.len() || gb.left() < 1 {
            return Err("types de blocs");
        }
        let mut at = dec;
        if gb.bit() {
            let v = gb.get(4) as u8;
            b.data[dec..end].fill(v);
            at = end;
        } else {
            let tree = b.tree;
            let mut last = 0;
            while at < end {
                let v = tree.huff(gb);
                if v < 12 {
                    last = v;
                    b.data[at] = v;
                    at += 1;
                } else {
                    let run = RLE_LENS[v as usize - 12];
                    if end - at < run {
                        return Err("types de blocs");
                    }
                    b.data[at..at + run].fill(last);
                    at += run;
                }
            }
        }
        b.dec = Some(at);
        Ok(())
    }

    fn read_patterns(&mut self, gb: &mut Lsb) -> R {
        let b = &mut self.bundle[PATTERN];
        let t = b.count(gb);
        let Some(dec) = b.dec.filter(|_| t > 0) else { return Ok(()) };
        if dec + t > b.data.len() {
            return Err("motifs");
        }
        let tree = b.tree;
        for slot in &mut b.data[dec..dec + t] {
            if gb.left() < 2 {
                return Err("motifs");
            }
            let low = tree.huff(gb);
            *slot = low | tree.huff(gb) << 4;
        }
        b.dec = Some(dec + t);
        Ok(())
    }

    fn read_colors(&mut self, gb: &mut Lsb) -> R {
        let old = self.version < b'i';
        let t = self.bundle[COLORS].count(gb);
        let Some(dec) = self.bundle[COLORS].dec.filter(|_| t > 0) else { return Ok(()) };
        if dec + t > self.bundle[COLORS].data.len() || gb.left() < 1 {
            return Err("couleurs");
        }
        let tree = self.bundle[COLORS].tree;
        let color = |value: u8| -> u8 {
            if !old {
                return value;
            }
            let sign = (value as i8 >> 7) as i32;
            ((((value & 0x7F) as i32 ^ sign) - sign) + 0x80) as u8
        };
        if gb.bit() {
            self.col_last = self.col_high[self.col_last].huff(gb) as usize;
            let v = (self.col_last as u8) << 4 | tree.huff(gb);
            self.bundle[COLORS].data[dec..dec + t].fill(color(v));
        } else {
            for i in 0..t {
                if gb.left() < 2 {
                    return Err("couleurs");
                }
                self.col_last = self.col_high[self.col_last].huff(gb) as usize;
                let v = (self.col_last as u8) << 4 | tree.huff(gb);
                self.bundle[COLORS].data[dec + i] = color(v);
            }
        }
        self.bundle[COLORS].dec = Some(dec + t);
        Ok(())
    }

    fn read_dcs(&mut self, gb: &mut Lsb, num: usize, has_sign: bool) -> R {
        let b = &mut self.bundle[num];
        let mut len = b.count(gb);
        let Some(mut dec) = b.dec.filter(|_| len > 0) else { return Ok(()) };
        let start_bits = 11 - u32::from(has_sign);
        if gb.left() < start_bits as isize {
            return Err("DC");
        }
        let mut v = gb.get(start_bits) as i32;
        if has_sign {
            v = gb.sign(v);
        }
        let mut put = |data: &mut Vec<u8>, value: i32| -> R {
            let bytes = (value as i16).to_le_bytes();
            if dec + 2 > data.len() {
                return Err("DC");
            }
            data[dec] = bytes[0];
            data[dec + 1] = bytes[1];
            dec += 2;
            Ok(())
        };
        put(&mut b.data, v)?;
        len -= 1;
        let mut i = 0;
        while i < len {
            let chunk = (len - i).min(8);
            let bsize = gb.get(4);
            for _ in 0..chunk {
                if bsize != 0 {
                    let value = gb.get(bsize) as i32;
                    v += gb.sign(value);
                    if !(-32768..=32767).contains(&v) {
                        return Err("DC");
                    }
                }
                put(&mut b.data, v)?;
            }
            i += 8;
        }
        b.dec = Some(dec);
        Ok(())
    }

    fn value(&mut self, num: usize) -> i32 {
        let b = &mut self.bundle[num];
        match num {
            X_OFF | Y_OFF => b.next_u8() as i8 as i32,
            INTRA_DC | INTER_DC => b.next_i16(),
            _ => b.next_u8() as i32,
        }
    }

    /// Les pixels d'un bloc de suites, dans l'ordre d'un motif.
    fn run_block(&mut self, gb: &mut Lsb, out: &mut [u8; 64]) -> R {
        let scan = &BINK_PATTERNS[gb.get(4) as usize];
        let mut i = 0usize;
        let mut at = 0usize;
        loop {
            let run = self.value(RUN) as usize + 1;
            i += run;
            if i > 64 {
                return Err("suite trop longue");
            }
            if gb.bit() {
                let v = self.value(COLORS) as u8;
                for _ in 0..run {
                    out[scan[at] as usize] = v;
                    at += 1;
                }
            } else {
                for _ in 0..run {
                    out[scan[at] as usize] = self.value(COLORS) as u8;
                    at += 1;
                }
            }
            if i >= 63 {
                break;
            }
        }
        if i == 63 {
            out[scan[at] as usize] = self.value(COLORS) as u8;
        }
        Ok(())
    }

    fn dct_block(&mut self, gb: &mut Lsb, dc: usize, quant: &[[i32; 64]; 16]) -> R<[i32; 64]> {
        let mut block = [0i32; 64];
        block[0] = self.value(dc);
        let mut idx = [0usize; 64];
        let (quant_idx, count) = read_dct_coeffs(gb, &mut block, &mut idx)?;
        let q = &quant[quant_idx];
        block[0] = block[0].wrapping_mul(q[0]) >> 11;
        for &i in &idx[..count] {
            let at = BINK_SCAN[i] as usize;
            block[at] = block[at].wrapping_mul(q[i]) >> 11;
        }
        Ok(block)
    }

    fn decode_plane(&mut self, gb: &mut Lsb, plane: usize, chroma: bool) -> R {
        let (bw, bh) = if chroma { ((self.w + 15) >> 4, (self.h + 15) >> 4) } else { ((self.w + 7) >> 3, (self.h + 7) >> 3) };
        let width = self.w >> usize::from(chroma);
        let height = self.h >> usize::from(chroma);
        let stride = self.cur.strides[plane];
        if self.version == b'k' && gb.bit() {
            let fill = gb.get(8) as u8;
            for y in 0..height {
                self.cur.planes[plane][y * stride..y * stride + width].fill(fill);
            }
            gb.align32();
            return Ok(());
        }
        self.init_lengths(width.max(8), bw);
        for i in 0..SOURCES {
            self.read_bundle(gb, i)?;
        }
        let mut dst = std::mem::take(&mut self.cur.planes[plane]);
        let prev = std::mem::take(&mut self.last.planes[plane]);
        let result = self.plane_blocks(gb, &mut dst, &prev, stride, bw, bh);
        self.cur.planes[plane] = dst;
        self.last.planes[plane] = prev;
        result?;
        gb.align32();
        Ok(())
    }

    fn plane_blocks(&mut self, gb: &mut Lsb, dst: &mut [u8], prev: &[u8], stride: usize, bw: usize, bh: usize) -> R {
        let ref_end = ((bw - 1 + stride * (bh - 1)) * 8) as isize;
        let put_ref = |this: &mut Self, dst: &mut [u8], at: usize| -> R {
            let (xoff, yoff) = (this.value(X_OFF) as isize, this.value(Y_OFF) as isize);
            let source = at as isize + xoff + yoff * stride as isize;
            if source < 0 || source > ref_end {
                return Err("copie hors de l'image");
            }
            copy8(dst, at, prev, source as usize, stride);
            Ok(())
        };
        for by in 0..bh {
            self.read_block_types(gb, BLOCK_TYPES)?;
            self.read_block_types(gb, SUB_BLOCK_TYPES)?;
            self.read_colors(gb)?;
            self.read_patterns(gb)?;
            self.read_motion(gb, X_OFF)?;
            self.read_motion(gb, Y_OFF)?;
            self.read_dcs(gb, INTRA_DC, false)?;
            self.read_dcs(gb, INTER_DC, true)?;
            self.read_runs(gb)?;
            let mut bx = 0;
            while bx < bw {
                let at = 8 * by * stride + 8 * bx;
                let block = self.value(BLOCK_TYPES);
                // Un bloc 16x16 sur une rangée impaire : déjà fait
                if ((by & 1) != 0 || (bx & 1) != 0) && block == 1 {
                    bx += 2;
                    continue;
                }
                match block {
                    0 => copy8(dst, at, prev, at, stride),
                    1 => {
                        let sub = self.value(SUB_BLOCK_TYPES);
                        let mut small = [0u8; 64];
                        match sub {
                            3 => {
                                if gb.left() < 4 {
                                    return Err("fin des donnees");
                                }
                                self.run_block(gb, &mut small)?;
                            }
                            5 => {
                                let coeffs = self.dct_block(gb, INTRA_DC, &BINK_INTRA_QUANT)?;
                                let pixels = idct(&coeffs);
                                for (out, value) in small.iter_mut().zip(pixels) {
                                    *out = value as u8;
                                }
                            }
                            6 => {
                                let v = self.value(COLORS) as u8;
                                for y in 0..16 {
                                    dst[at + y * stride..at + y * stride + 16].fill(v);
                                }
                            }
                            8 => {
                                let colors = [self.value(COLORS) as u8, self.value(COLORS) as u8];
                                for j in 0..8 {
                                    let mut v = self.value(PATTERN);
                                    for i in 0..8 {
                                        small[i + j * 8] = colors[(v & 1) as usize];
                                        v >>= 1;
                                    }
                                }
                            }
                            9 => {
                                for value in &mut small {
                                    *value = self.value(COLORS) as u8;
                                }
                            }
                            _ => return Err("type de bloc 16x16"),
                        }
                        if sub != 6 {
                            for j in 0..8 {
                                for i in 0..8 {
                                    let v = small[i + j * 8];
                                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                                        dst[at + (2 * j + dy) * stride + 2 * i + dx] = v;
                                    }
                                }
                            }
                        }
                        bx += 1;
                    }
                    2 => put_ref(self, dst, at)?,
                    3 => {
                        let mut pixels = [0u8; 64];
                        // Le bloc garde les pixels que le motif n'atteint pas
                        for j in 0..8 {
                            pixels[j * 8..j * 8 + 8].copy_from_slice(&dst[at + j * stride..at + j * stride + 8]);
                        }
                        self.run_block(gb, &mut pixels)?;
                        for j in 0..8 {
                            dst[at + j * stride..at + j * stride + 8].copy_from_slice(&pixels[j * 8..j * 8 + 8]);
                        }
                    }
                    4 => {
                        put_ref(self, dst, at)?;
                        let mut residue = [0i16; 64];
                        let masks = gb.get(7) as i32;
                        read_residue(gb, &mut residue, masks);
                        for j in 0..8 {
                            for i in 0..8 {
                                let p = &mut dst[at + j * stride + i];
                                *p = p.wrapping_add(residue[j * 8 + i] as u8);
                            }
                        }
                    }
                    5 => {
                        let coeffs = self.dct_block(gb, INTRA_DC, &BINK_INTRA_QUANT)?;
                        let pixels = idct(&coeffs);
                        for j in 0..8 {
                            for i in 0..8 {
                                dst[at + j * stride + i] = pixels[j * 8 + i] as u8;
                            }
                        }
                    }
                    6 => {
                        let v = self.value(COLORS) as u8;
                        for y in 0..8 {
                            dst[at + y * stride..at + y * stride + 8].fill(v);
                        }
                    }
                    7 => {
                        put_ref(self, dst, at)?;
                        let coeffs = self.dct_block(gb, INTER_DC, &BINK_INTER_QUANT)?;
                        let pixels = idct(&coeffs);
                        for j in 0..8 {
                            for i in 0..8 {
                                let p = &mut dst[at + j * stride + i];
                                *p = p.wrapping_add(pixels[j * 8 + i] as u8);
                            }
                        }
                    }
                    8 => {
                        let colors = [self.value(COLORS) as u8, self.value(COLORS) as u8];
                        for i in 0..8 {
                            let mut v = self.value(PATTERN);
                            for j in 0..8 {
                                dst[at + i * stride + j] = colors[(v & 1) as usize];
                                v >>= 1;
                            }
                        }
                    }
                    9 => {
                        let colors = &mut self.bundle[COLORS];
                        for i in 0..8 {
                            for j in 0..8 {
                                dst[at + i * stride + j] = colors.next_u8();
                            }
                        }
                    }
                    _ => return Err("type de bloc"),
                }
                bx += 1;
            }
        }
        Ok(())
    }

    fn decode_video(&mut self, packet: &[u8]) -> R {
        let mut gb = Lsb::new(packet);
        let total = packet.len() as isize * 8;
        if self.has_alpha {
            if self.version >= b'i' {
                gb.skip(32);
            }
            self.decode_plane(&mut gb, 3, false)?;
        }
        if self.version >= b'i' {
            gb.skip(32);
        }
        for plane in 0..3 {
            let index = if plane == 0 || self.version < b'h' { plane } else { plane ^ 3 };
            self.decode_plane(&mut gb, index, plane > 0)?;
            if gb.pos() as isize >= total {
                break;
            }
        }
        Ok(())
    }
}

fn copy8(dst: &mut [u8], at: usize, src: &[u8], from: usize, stride: usize) {
    for y in 0..8 {
        let row = from + y * stride;
        if let Some(line) = src.get(row..row + 8) {
            dst[at + y * stride..at + y * stride + 8].copy_from_slice(line);
        }
    }
}

/// Lit un bloc de coefficients. Rend (n° de quantification, nombre de coefficients).
fn read_dct_coeffs(gb: &mut Lsb, block: &mut [i32; 64], idx: &mut [usize; 64]) -> R<(usize, usize)> {
    if gb.left() < 4 {
        return Err("fin des donnees");
    }
    let mut coef = [0i32; 128];
    let mut mode = [0i32; 128];
    let (mut start, mut end) = (64usize, 64usize);
    for (c, m) in [(4, 0), (24, 0), (44, 0), (1, 3), (2, 3), (3, 3)] {
        coef[end] = c;
        mode[end] = m;
        end += 1;
    }
    let mut count = 0usize;
    let mut bits = gb.get(4) as i32 - 1;
    let read_value = |gb: &mut Lsb, bits: i32| -> i32 {
        if bits == 0 {
            1 - ((gb.get(1) as i32) << 1)
        } else {
            let t = gb.get(bits as u32) as i32 | 1 << bits;
            if gb.bit() { -t } else { t }
        }
    };
    while bits >= 0 {
        let mut pos = start;
        while pos < end {
            if (mode[pos] | coef[pos]) == 0 || !gb.bit() {
                pos += 1;
                continue;
            }
            let mut ccoef = coef[pos];
            let m = mode[pos];
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
                        if gb.bit() {
                            start = start.checked_sub(1).ok_or("liste")?;
                            coef[start] = ccoef;
                            mode[start] = 3;
                        } else {
                            let value = read_value(gb, bits);
                            if count >= 64 || ccoef >= 64 {
                                return Err("trop de coefficients");
                            }
                            block[BINK_SCAN[ccoef as usize] as usize] = value;
                            idx[count] = ccoef as usize;
                            count += 1;
                        }
                        ccoef += 1;
                    }
                }
                1 => {
                    mode[pos] = 2;
                    for _ in 0..3 {
                        ccoef += 4;
                        if end >= 128 {
                            return Err("liste");
                        }
                        coef[end] = ccoef;
                        mode[end] = 2;
                        end += 1;
                    }
                }
                _ => {
                    let value = read_value(gb, bits);
                    if count >= 64 || ccoef >= 64 {
                        return Err("trop de coefficients");
                    }
                    block[BINK_SCAN[ccoef as usize] as usize] = value;
                    idx[count] = ccoef as usize;
                    count += 1;
                    coef[pos] = 0;
                    mode[pos] = 0;
                    pos += 1;
                }
            }
        }
        bits -= 1;
    }
    Ok((gb.get(4) as usize, count))
}

fn read_residue(gb: &mut Lsb, block: &mut [i16; 64], mut masks: i32) {
    let mut coef = [0i32; 128];
    let mut mode = [0i32; 128];
    let (mut start, mut end) = (64usize, 64usize);
    for (c, m) in [(4, 0), (24, 0), (44, 0), (0, 2)] {
        coef[end] = c;
        mode[end] = m;
        end += 1;
    }
    let mut nz = [0usize; 64];
    let mut nz_count = 0usize;
    let mut mask = 1i32 << gb.get(3);
    while mask != 0 {
        for i in 0..nz_count {
            if !gb.bit() {
                continue;
            }
            let p = &mut block[nz[i]];
            if *p < 0 {
                *p = p.wrapping_sub(mask as i16);
            } else {
                *p = p.wrapping_add(mask as i16);
            }
            masks -= 1;
            if masks < 0 {
                return;
            }
        }
        let mut pos = start;
        while pos < end {
            if (coef[pos] | mode[pos]) == 0 || !gb.bit() {
                pos += 1;
                continue;
            }
            let mut ccoef = coef[pos];
            let m = mode[pos];
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
                        if gb.bit() {
                            if start == 0 {
                                return;
                            }
                            start -= 1;
                            coef[start] = ccoef;
                            mode[start] = 3;
                        } else {
                            if nz_count >= 64 || ccoef >= 64 {
                                return;
                            }
                            let at = BINK_SCAN[ccoef as usize] as usize;
                            nz[nz_count] = at;
                            nz_count += 1;
                            block[at] = if gb.bit() { -(mask as i16) } else { mask as i16 };
                            masks -= 1;
                            if masks < 0 {
                                return;
                            }
                        }
                        ccoef += 1;
                    }
                }
                1 => {
                    mode[pos] = 2;
                    for _ in 0..3 {
                        ccoef += 4;
                        if end >= 128 {
                            return;
                        }
                        coef[end] = ccoef;
                        mode[end] = 2;
                        end += 1;
                    }
                }
                _ => {
                    if nz_count >= 64 || ccoef >= 64 {
                        return;
                    }
                    let at = BINK_SCAN[ccoef as usize] as usize;
                    nz[nz_count] = at;
                    nz_count += 1;
                    block[at] = if gb.bit() { -(mask as i16) } else { mask as i16 };
                    coef[pos] = 0;
                    mode[pos] = 0;
                    pos += 1;
                    masks -= 1;
                    if masks < 0 {
                        return;
                    }
                }
            }
        }
        mask >>= 1;
    }
}

const CRITICAL_FREQS: [u32; 25] = [100, 200, 300, 400, 510, 630, 770, 920, 1080, 1270, 1480, 1720, 2000, 2320, 2700, 3150, 3700, 4400, 5300, 6400, 7700, 9500, 12000, 15500, 24500];
const RLE_TAB: [usize; 16] = [2, 3, 4, 5, 6, 8, 9, 10, 11, 12, 13, 14, 15, 16, 32, 64];

/// Le son Bink, variante DCT.
struct BinkAudio {
    channels: usize,
    frame_len: usize,
    overlap: usize,
    num_bands: usize,
    root: f32,
    bands: [usize; 26],
    quant: [f32; 96],
    previous: Vec<Vec<f32>>,
    first: bool,
    dct: Dct3,
}

impl BinkAudio {
    fn new(rate: u32, channels: usize) -> BinkAudio {
        let bits = if rate < 22050 {
            9
        } else if rate < 44100 {
            10
        } else {
            11
        };
        let frame_len = 1usize << bits;
        let half_rate = (rate as u64 + 1) / 2;
        let root = (frame_len as f64 / ((frame_len as f64).sqrt() * 32768.0)) as f32;
        let quant = std::array::from_fn(|i| (i as f32 * 0.152_891_65_f32).exp() * root);
        let mut num_bands = 1;
        while num_bands < 25 && half_rate > CRITICAL_FREQS[num_bands - 1] as u64 {
            num_bands += 1;
        }
        let mut bands = [0usize; 26];
        bands[0] = 2;
        for (i, band) in bands.iter_mut().enumerate().take(num_bands).skip(1) {
            *band = ((CRITICAL_FREQS[i - 1] as u64 * frame_len as u64 / half_rate) & !1) as usize;
        }
        bands[num_bands] = frame_len;
        BinkAudio { channels, frame_len, overlap: frame_len / 16, num_bands, root, bands, quant, previous: vec![vec![0.0; frame_len / 16]; channels], first: true, dct: Dct3::new(frame_len) }
    }

    fn float(gb: &mut Lsb) -> f32 {
        let power = gb.get(5) as i32;
        let mantissa = gb.get(23) as f32;
        let f = mantissa * 2f32.powi(power - 23);
        if gb.bit() { -f } else { f }
    }

    /// Un paquet : plusieurs blocs, chacun ajouté à `pcm`.
    fn decode(&mut self, packet: &[u8], pcm: &mut Vec<f32>) -> R {
        if packet.len() < 4 {
            return Err("son : paquet");
        }
        let mut gb = Lsb::new(packet);
        gb.skip(32);
        let mut out = vec![vec![0f32; self.frame_len]; self.channels];
        while gb.left() > 0 {
            let mut coeffs = vec![0f32; self.frame_len + 2];
            gb.skip(2);
            for ch in 0..self.channels {
                if gb.left() < 58 {
                    return Err("son : fin des donnees");
                }
                coeffs[0] = Self::float(&mut gb) * self.root;
                coeffs[1] = Self::float(&mut gb) * self.root;
                if gb.left() < self.num_bands as isize * 8 {
                    return Err("son : fin des donnees");
                }
                let mut quant = [0f32; 25];
                for q in quant.iter_mut().take(self.num_bands) {
                    *q = self.quant[(gb.get(8) as usize).min(95)];
                }
                let mut k = 0;
                let mut q = quant[0];
                let mut i = 2;
                while i < self.frame_len {
                    let mut j = if gb.bit() { i + RLE_TAB[gb.get(4) as usize] * 8 } else { i + 8 };
                    j = j.min(self.frame_len);
                    let width = gb.get(4);
                    if width == 0 {
                        coeffs[i..j].fill(0.0);
                        i = j;
                        while self.bands[k] < i {
                            q = quant[k];
                            k += 1;
                        }
                    } else {
                        while i < j {
                            if self.bands[k] == i {
                                q = quant[k];
                                k += 1;
                            }
                            let coeff = gb.get(width);
                            coeffs[i] = if coeff != 0 {
                                if gb.bit() { -q * coeff as f32 } else { q * coeff as f32 }
                            } else {
                                0.0
                            };
                            i += 1;
                        }
                    }
                }
                coeffs[0] /= 0.5;
                self.dct.run(&coeffs[..self.frame_len], &mut out[ch]);
            }
            let count = self.overlap * self.channels;
            for ch in 0..self.channels {
                if !self.first {
                    let mut j = ch;
                    for i in 0..self.overlap {
                        out[ch][i] = (self.previous[ch][i] * (count - j) as f32 + out[ch][i] * j as f32) / count as f32;
                        j += self.channels;
                    }
                }
                let keep = self.frame_len - self.overlap;
                self.previous[ch].copy_from_slice(&out[ch][keep..]);
            }
            self.first = false;
            let samples = self.frame_len - self.overlap;
            let start = pcm.len();
            pcm.resize(start + samples * self.channels, 0.0);
            for s in 0..samples {
                for ch in 0..self.channels {
                    pcm[start + s * self.channels + ch] = out[ch][s];
                }
            }
            gb.align32();
        }
        Ok(())
    }
}

/// DCT-III en passant par une FFT, à l'échelle de celle de FFmpeg.
pub(crate) struct Dct3 {
    n: usize,
    twiddle: Vec<(f32, f32)>,
    roots: Vec<(f32, f32)>,
    re: Vec<f32>,
    im: Vec<f32>,
}

impl Dct3 {
    pub(crate) fn new(n: usize) -> Dct3 {
        let unit = std::f64::consts::PI / (2.0 * n as f64);
        let twiddle = (0..n).map(|i| ((unit * i as f64).cos() as f32, (unit * i as f64).sin() as f32)).collect();
        let roots = (0..n / 2).map(|i| ((4.0 * unit * i as f64).cos() as f32, (4.0 * unit * i as f64).sin() as f32)).collect();
        Dct3 { n, twiddle, roots, re: vec![0.0; n], im: vec![0.0; n] }
    }

    pub(crate) fn run(&mut self, input: &[f32], out: &mut [f32]) {
        let n = self.n;
        self.re[0] = input[0] * 0.5;
        self.im[0] = 0.0;
        for k in 1..n {
            let (a, b) = (input[k], input[n - k]);
            let (c, s) = self.twiddle[k];
            // (a - i b) * e^(i theta) / 2
            self.re[k] = (a * c + b * s) * 0.5;
            self.im[k] = (a * s - b * c) * 0.5;
        }
        let bits = n.trailing_zeros();
        for i in 0..n {
            let j = (i as u32).reverse_bits() as usize >> (32 - bits);
            if j > i {
                self.re.swap(i, j);
                self.im.swap(i, j);
            }
        }
        let mut size = 2;
        while size <= n {
            let step = n / size;
            for start in (0..n).step_by(size) {
                for k in 0..size / 2 {
                    let (c, s) = self.roots[k * step];
                    let (a, b) = (start + k, start + k + size / 2);
                    let tr = self.re[b] * c - self.im[b] * s;
                    let ti = self.re[b] * s + self.im[b] * c;
                    self.re[b] = self.re[a] - tr;
                    self.im[b] = self.im[a] - ti;
                    self.re[a] += tr;
                    self.im[a] += ti;
                }
            }
            size <<= 1;
        }
        let scale = 2.0 / n as f32;
        for k in 0..n / 2 {
            out[2 * k] = self.re[k] * scale;
            out[2 * k + 1] = self.re[n - 1 - k] * scale;
        }
    }
}

impl Stream for Bink {
    fn yuv(&self, out: &mut Vec<u8>) {
        out.clear();
        for plane in 0..3 {
            let (w, h) = if plane == 0 { (self.w, self.h) } else { (self.w.div_ceil(2), self.h.div_ceil(2)) };
            let stride = self.last.strides[plane];
            for y in 0..h {
                out.extend_from_slice(&self.last.planes[plane][y * stride..y * stride + w]);
            }
        }
    }

    fn rewind(&mut self) {
        self.frame = 0;
        for plane in self.last.planes.iter_mut().chain(self.cur.planes.iter_mut()) {
            plane.fill(0);
        }
        if let Some(audio) = &mut self.audio {
            audio.first = true;
        }
    }

    fn next(&mut self, pcm: &mut Vec<f32>) -> Option<R> {
        if self.frame + 1 >= self.index.len() {
            return None;
        }
        let start = self.index[self.frame] as usize;
        let mut end = self.index[self.frame + 1] as usize;
        if self.frame + 2 == self.index.len() || end > self.file_end {
            end = self.file_end;
        }
        self.frame += 1;
        if start >= end || end > self.bytes.len() {
            return Some(Err("position d'image"));
        }
        let bytes = self.bytes.clone();
        let mut at = start;
        let mut result = Ok(());
        for track in 0..self.tracks {
            if at + 4 > end {
                return Some(Err("son : taille"));
            }
            let size = le32(&bytes, at) as usize;
            at += 4;
            if at + size > end {
                return Some(Err("son : taille"));
            }
            if track == 0 && size >= 4 {
                if let Some(audio) = &mut self.audio {
                    result = audio.decode(&bytes[at..at + size], pcm);
                }
            }
            at += size;
        }
        if let Err(e) = self.decode_video(&bytes[at..end]) {
            return Some(Err(e));
        }
        std::mem::swap(&mut self.cur, &mut self.last);
        Some(result)
    }

    fn rgba(&self, out: &mut Vec<u8>) {
        let (w, h) = (self.w, self.h);
        out.clear();
        out.resize(w * h * 4, 255);
        let p = &self.last;
        for y in 0..h {
            for x in 0..w {
                let c = 298 * (p.planes[0][y * p.strides[0] + x] as i32 - 16);
                let at = (y / 2) * p.strides[1] + x / 2;
                let (d, e) = (p.planes[1][at] as i32 - 128, p.planes[2][at] as i32 - 128);
                let px = &mut out[(y * w + x) * 4..][..4];
                px[0] = clip8((c + 409 * e + 128) >> 8);
                px[1] = clip8((c - 100 * d - 208 * e + 128) >> 8);
                px[2] = clip8((c + 516 * d + 128) >> 8);
                if self.has_alpha {
                    px[3] = p.planes[3][y * p.strides[3] + x];
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Dct3;

    #[test]
    fn dct3_matches_its_formula() {
        let n = 64;
        let input: Vec<f32> = (0..n).map(|i| ((i * 37 % 19) as f32 - 9.0) / 7.0).collect();
        let mut out = vec![0f32; n];
        Dct3::new(n).run(&input, &mut out);
        for (k, value) in out.iter().enumerate() {
            let mut sum = input[0] as f64 * 0.5;
            for (i, x) in input.iter().enumerate().skip(1) {
                sum += *x as f64 * (std::f64::consts::PI * i as f64 * (2 * k + 1) as f64 / (2 * n) as f64).cos();
            }
            assert!((sum * 2.0 / n as f64 - *value as f64).abs() < 1e-4, "k={k}");
        }
    }
}
