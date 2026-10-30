//! Formats 2D du SDK Nitro : palettes NCLR, tuiles NCGR, écrans NSCR, cellules NCER ; et les fichiers source
//! de NITRO-CHARACTER, avec les mêmes données : NCCL, NCCG, NCSC.
//!
//! Chaque fichier a un en-tête de 16 octets (signature, 0xFEFF, version, taille, taille de
//! l'en-tête, nombre de blocs) puis des blocs (signature, taille avec ces 8 octets, données).
//! Les offsets dans les blocs comptent à partir des données du bloc (après ses 8 octets).
//! Formats décrits par GBATEK (registres, OAM) et la doc de NitroSystem ; même logique que
//! Tinke et NitroPaint, réécrite ici.

use crate::errors::Result;
use crate::{bgr555, Rgba};

fn u16_at(b: &[u8], off: usize) -> Result<u16> {
    match b.get(off..off + 2) {
        Some(s) => Ok(u16::from_le_bytes([s[0], s[1]])),
        None => bail!("fichier tronque (offset {:#x})", off),
    }
}

fn u32_at(b: &[u8], off: usize) -> Result<u32> {
    match b.get(off..off + 4) {
        Some(s) => Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]])),
        None => bail!("fichier tronque (offset {:#x})", off),
    }
}

/// Les données (après l'en-tête de 8 octets) du premier bloc dont la signature est l'une de
/// `names` (écrites à l'envers dans les fichiers : « TTLP » pour PLTT).
fn block<'a>(file: &'a [u8], names: &[&[u8; 4]]) -> Result<&'a [u8]> {
    let header = u16_at(file, 12)? as usize;
    let count = u16_at(file, 14)? as usize;
    let mut off = if header >= 16 { header } else { 16 };
    for _ in 0..count.max(1) {
        let Some(magic) = file.get(off..off + 4) else { break };
        let size = u32_at(file, off + 4)? as usize;
        let end = if size < 8 { file.len() } else { (off + size).min(file.len()) };
        if names.iter().any(|n| n.as_slice() == magic) {
            return Ok(&file[off + 8..end]);
        }
        if size < 8 {
            break;
        }
        off += size;
    }
    bail!("bloc {} absent", String::from_utf8_lossy(names[0]))
}

// --------------------------------------------------------------------------------------------
//  Palettes
// --------------------------------------------------------------------------------------------

pub struct Palette {
    pub colors: Vec<[u8; 4]>,
}

impl Palette {
    /// Couleur `index` du groupe de 16 couleurs `bank` (4 bpp) ; `bank` 0 en 8 bpp.
    fn color(&self, bank: usize, index: u8, bpp8: bool) -> [u8; 4] {
        if index == 0 {
            return [0; 4];
        }
        let i = if bpp8 { index as usize } else { bank * 16 + index as usize };
        self.colors.get(i).copied().unwrap_or([255, 0, 255, 255])
    }

    /// Nombre de groupes de 16 couleurs.
    pub fn banks(&self) -> usize {
        self.colors.len().div_ceil(16).max(1)
    }

    /// Les couleurs en carrés de 16 par ligne (aperçu de la palette seule).
    pub fn swatches(&self) -> Rgba {
        const CELL: u32 = 12;
        let rows = self.banks() as u32;
        let (width, height) = (16 * CELL, rows * CELL);
        let mut data = vec![0; (width * height * 4) as usize];
        for (i, c) in self.colors.iter().enumerate() {
            let (cx, cy) = (i as u32 % 16, i as u32 / 16);
            for y in 1..CELL {
                for x in 1..CELL {
                    let p = (((cy * CELL + y) * width + cx * CELL + x) * 4) as usize;
                    data[p..p + 4].copy_from_slice(c);
                }
            }
        }
        Rgba { width, height, data }
    }
}

pub fn read_palette(file: &[u8]) -> Result<Palette> {
    // Moteur maison : TTLP, taille, couleurs. MySims : NCL, nombre de couleurs, 0, couleurs
    if file.get(0..4) == Some(b"TTLP") {
        let size = u32_at(file, 4)? as usize;
        let data = file.get(8..).unwrap_or(&[]);
        return Ok(raw_palette(&data[..size.min(data.len())]));
    }
    if file.get(0..4) == Some(b"NCL\0") {
        let size = u32_at(file, 4)? as usize * 2;
        let data = file.get(8..).unwrap_or(&[]);
        return Ok(raw_palette(&data[..size.min(data.len())]));
    }
    if file.get(0..4) == Some(b"NCCL") {
        // Bloc PALT : couleurs par groupe, nombre de groupes, couleurs
        let b = block(file, &[b"PALT"])?;
        return Ok(raw_palette(b.get(8..).unwrap_or(&[])));
    }
    let b = block(file, &[b"TTLP", b"PLTT"])?;
    let size = u32_at(b, 8)? as usize;
    let offset = u32_at(b, 12)? as usize;
    let data = b.get(offset..).unwrap_or(&[]);
    let len = if size == 0 || size > data.len() { data.len() } else { size };
    let colors = data[..len & !1].chunks_exact(2).map(|c| bgr555(u16::from_le_bytes([c[0], c[1]]))).collect();
    Ok(Palette { colors })
}

// --------------------------------------------------------------------------------------------
//  Tuiles
// --------------------------------------------------------------------------------------------

pub struct Tiles {
    pub bpp8: bool,
    /// Pixels rangés en bitmap (NCBR, ou NCGR « linéaire ») et non tuile par tuile.
    pub linear: bool,
    /// Index de couleur de chaque pixel, dans l'ordre du fichier : 64 par tuile ligne par ligne,
    /// ou lignes de l'image entière si `linear`.
    pub pixels: Vec<u8>,
    /// Largeur et hauteur en tuiles écrites dans le fichier (absentes en mode 1D).
    pub width: Option<u32>,
    pub height: Option<u32>,
}

impl Tiles {
    pub fn count(&self) -> usize {
        self.pixels.len() / 64
    }

    /// Largeur en tuiles pour l'aperçu de la planche : celle du fichier, sinon un carré si le compte tombe juste
    /// (les icônes : 16 tuiles = 4x4), sinon 32.
    pub fn default_width(&self) -> u32 {
        let count = self.count() as u32;
        let root = (count as f64).sqrt() as u32;
        match self.width {
            Some(w) if w > 0 && w as usize <= self.count() => w,
            _ if root > 0 && root * root == count && count <= 1024 => root,
            _ => count.clamp(1, 32),
        }
    }

    /// Pixel (`x`, `y`) de la tuile `tile` (planche de `self.width` tuiles de large, 32 sans).
    fn tile_pixel(&self, tile: usize, x: usize, y: usize) -> u8 {
        let i = if self.linear {
            let wt = self.width.unwrap_or(32) as usize;
            (tile / wt * 8 + y) * wt * 8 + tile % wt * 8 + x
        } else {
            tile * 64 + y * 8 + x
        };
        self.pixels.get(i).copied().unwrap_or(0)
    }
}

pub fn read_tiles(file: &[u8]) -> Result<Tiles> {
    // MySims : NCG, nombre de tuiles (u16), 1 = 8 bits par pixel (u16), pixels
    if file.get(0..4) == Some(b"NCG\0") {
        let bpp8 = u16_at(file, 6)? == 1;
        let data = file.get(8..).unwrap_or(&[]);
        let pixels = if bpp8 { data.to_vec() } else { data.iter().flat_map(|p| [p & 15, p >> 4]).collect() };
        return Ok(Tiles { bpp8, linear: false, pixels, width: None, height: None });
    }
    if file.get(0..4) == Some(b"NCCG") {
        // Bloc CHAR : largeur et hauteur en tuiles, 1 = 8 bits par pixel, pixels
        let b = block(file, &[b"CHAR"])?;
        let bpp8 = u32_at(b, 8)? == 1;
        let data = b.get(12..).unwrap_or(&[]);
        let pixels = if bpp8 { data.to_vec() } else { data.iter().flat_map(|p| [p & 15, p >> 4]).collect() };
        return Ok(Tiles { bpp8, linear: false, pixels, width: Some(u32_at(b, 0)?), height: Some(u32_at(b, 4)?) });
    }
    let b = block(file, &[b"RAHC", b"CHAR"])?;
    let h = u16_at(b, 0)?;
    let w = u16_at(b, 2)?;
    let bpp8 = u32_at(b, 4)? == 4;
    let linear = u32_at(b, 12)? & 1 == 1;
    let size = u32_at(b, 16)? as usize;
    let offset = u32_at(b, 20)? as usize;
    let data = b.get(offset..).unwrap_or(&[]);
    let data = &data[..if size == 0 || size > data.len() { data.len() } else { size }];
    let mut pixels = Vec::with_capacity(data.len() * 2);
    for &byte in data {
        if bpp8 {
            pixels.push(byte);
        } else {
            pixels.push(byte & 15);
            pixels.push(byte >> 4);
        }
    }
    let width = (w != 0xFFFF && w != 0).then_some(w as u32);
    let height = (h != 0xFFFF && h != 0).then_some(h as u32);
    Ok(Tiles { bpp8, linear, pixels, width, height })
}

fn put(out: &mut Rgba, x: i64, y: i64, color: [u8; 4]) {
    if color[3] == 0 || x < 0 || y < 0 || x >= out.width as i64 || y >= out.height as i64 {
        return;
    }
    let p = ((y as u32 * out.width + x as u32) * 4) as usize;
    out.data[p..p + 4].copy_from_slice(&color);
}

/// Dessine la tuile `tile` en (`px`, `py`) dans `out`.
#[allow(clippy::too_many_arguments)]
fn draw_tile(out: &mut Rgba, tiles: &Tiles, tile: usize, palette: &Palette, bank: usize, px: i64, py: i64, hflip: bool, vflip: bool) {
    for y in 0..8 {
        for x in 0..8 {
            let (sx, sy) = (if hflip { 7 - x } else { x }, if vflip { 7 - y } else { y });
            let index = tiles.tile_pixel(tile, sx, sy);
            put(out, px + x as i64, py + y as i64, palette.color(bank, index, tiles.bpp8));
        }
    }
}

/// Toutes les tuiles, `width` tuiles par ligne, avec le groupe de couleurs `bank` (4 bpp).
pub fn render_tiles(tiles: &Tiles, palette: &Palette, bank: usize, width: u32) -> Rgba {
    let width = width.max(1);
    let rows = (tiles.count() as u32).div_ceil(width).max(1);
    let mut out = Rgba { width: width * 8, height: rows * 8, data: vec![0; (width * 8 * rows * 8 * 4) as usize] };
    if tiles.linear {
        // Bitmap : `width` * 8 pixels par ligne
        for (i, &index) in tiles.pixels.iter().enumerate() {
            let (x, y) = (i as u32 % out.width, i as u32 / out.width);
            put(&mut out, x as i64, y as i64, palette.color(bank, index, tiles.bpp8));
        }
        return out;
    }
    for t in 0..tiles.count() {
        let (tx, ty) = (t as u32 % width, t as u32 / width);
        draw_tile(&mut out, tiles, t, palette, bank, tx as i64 * 8, ty as i64 * 8, false, false);
    }
    out
}

// --------------------------------------------------------------------------------------------
//  Écrans (fonds de tuiles)
// --------------------------------------------------------------------------------------------

pub struct Screen {
    /// Taille en pixels.
    pub width: u32,
    pub height: u32,
    /// Une entrée par tuile de l'écran : n° de tuile (bits 0-9), miroir H (10), V (11), palette (12-15).
    pub entries: Vec<u16>,
}

pub fn read_screen(file: &[u8]) -> Result<Screen> {
    // MySims : NSC, nombre d'entrées, largeur et hauteur en tuiles (u8), entrées
    if file.get(0..4) == Some(b"NSC\0") {
        let (Some(&width), Some(&height)) = (file.get(8), file.get(9)) else { bail!("ecran trop court") };
        let entries = file.get(12..).unwrap_or(&[]).chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        return Ok(Screen { width: width as u32 * 8, height: height as u32 * 8, entries });
    }
    if file.get(0..4) == Some(b"NCSC") {
        // Bloc SCRN : largeur et hauteur en tuiles, 8 octets, entrées
        let b = block(file, &[b"SCRN"])?;
        let (width, height) = (u32_at(b, 0)?, u32_at(b, 4)?);
        if width > 512 || height > 512 {
            bail!("ecran de {}x{} tuiles : fichier inattendu", width, height);
        }
        let entries = b.get(16..).unwrap_or(&[]).chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        return Ok(Screen { width: width * 8, height: height * 8, entries });
    }
    let b = block(file, &[b"NRCS", b"SCRN"])?;
    let width = u16_at(b, 0)? as u32;
    let height = u16_at(b, 2)? as u32;
    if width > 4096 || height > 4096 {
        bail!("ecran de {}x{} : fichier inattendu", width, height);
    }
    let size = u32_at(b, 8)? as usize;
    let data = b.get(12..).unwrap_or(&[]);
    let data = &data[..size.min(data.len())];
    let cells = (width / 8 * (height / 8)) as usize;
    // Fond « affine » : une entrée d'un octet par tuile
    let entries: Vec<u16> = if data.len() < cells * 2 && data.len() >= cells {
        data.iter().map(|&b| b as u16).collect()
    } else {
        data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect()
    };
    Ok(Screen { width, height, entries })
}

pub fn render_screen(screen: &Screen, tiles: &Tiles, palette: &Palette) -> Rgba {
    let (wt, ht) = (screen.width / 8, screen.height / 8);
    let mut out = Rgba { width: screen.width, height: screen.height, data: vec![0; (screen.width * screen.height * 4) as usize] };
    // Fonds texte de plus de 256 pixels de large : rangés par blocs de 32x32 tuiles
    let blocks = wt > 32 && screen.entries.len() as u32 >= wt * ht;
    for ty in 0..ht {
        for tx in 0..wt {
            let i = if blocks { ((ty / 32) * (wt / 32) + tx / 32) * 1024 + (ty % 32) * 32 + tx % 32 } else { ty * wt + tx };
            let Some(&e) = screen.entries.get(i as usize) else { continue };
            draw_tile(&mut out, tiles, (e & 0x3FF) as usize, palette, (e >> 12) as usize, tx as i64 * 8, ty as i64 * 8, e & 0x400 != 0, e & 0x800 != 0);
        }
    }
    out
}

// --------------------------------------------------------------------------------------------
//  Cellules (sprites faits de plusieurs OBJ)
// --------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct Obj {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub tile: u32,
    pub bank: u32,
    pub hflip: bool,
    pub vflip: bool,
    pub bpp8: bool,
}

pub struct Cells {
    pub cells: Vec<Vec<Obj>>,
    /// NNSG2dCharacterDataMapingType : 0 = 1D 32 Ko, 1 = 1D 64 Ko, 2 = 1D 128 Ko, 3 = 1D 256 Ko, 4 = 2D.
    pub mapping: u32,
    /// Largeur et hauteur de chaque cellule, quand le fichier les donne (JNCD).
    pub sizes: Vec<(u32, u32)>,
}

const OBJ_SIZES: [[(u32, u32); 4]; 3] = [
    [(8, 8), (16, 16), (32, 32), (64, 64)],
    [(16, 8), (32, 8), (32, 16), (64, 32)],
    [(8, 16), (8, 32), (16, 32), (32, 64)],
];

pub fn read_cells(file: &[u8]) -> Result<Cells> {
    let b = block(file, &[b"KBEC", b"CEBK"])?;
    let count = u16_at(b, 0)? as usize;
    let bounded = u16_at(b, 2)? & 1 == 1;
    let cells_off = u32_at(b, 4)? as usize;
    let mapping = u32_at(b, 8)?;
    let cell_size = if bounded { 16 } else { 8 };
    let objs_off = cells_off + count * cell_size;
    let mut cells = Vec::with_capacity(count);
    for c in 0..count {
        let at = cells_off + c * cell_size;
        let n = u16_at(b, at)? as usize;
        let first = u32_at(b, at + 4)? as usize;
        let mut objs = Vec::with_capacity(n);
        for o in 0..n {
            let p = objs_off + first + o * 6;
            let (a0, a1, a2) = (u16_at(b, p)?, u16_at(b, p + 2)?, u16_at(b, p + 4)?);
            let shape = ((a0 >> 14) & 3) as usize;
            let size = ((a1 >> 14) & 3) as usize;
            let (width, height) = OBJ_SIZES[shape.min(2)][size];
            let affine = a0 & 0x100 != 0;
            objs.push(Obj {
                x: ((a1 & 0x1FF) as i32) << 23 >> 23,
                y: (a0 & 0xFF) as i8 as i32,
                width,
                height,
                tile: (a2 & 0x3FF) as u32,
                bank: (a2 >> 12) as u32,
                hflip: !affine && a1 & 0x1000 != 0,
                vflip: !affine && a1 & 0x2000 != 0,
                bpp8: a0 & 0x2000 != 0,
            });
        }
        cells.push(objs);
    }
    Ok(Cells { cells, mapping, sizes: Vec::new() })
}

/// Une cellule (les OBJ du premier au dernier sont dessinés du dessus au dessous) ;
/// (0, 0) de la cellule au centre de l'image.
pub fn render_cell(cells: &Cells, index: usize, tiles: &Tiles, palette: &Palette) -> Rgba {
    let Some(objs) = cells.cells.get(index) else { return Rgba { width: 1, height: 1, data: vec![0; 4] } };
    let reach = objs.iter().map(|o| o.x.abs().max((o.x + o.width as i32).abs()).max(o.y.abs()).max((o.y + o.height as i32).abs())).max().unwrap_or(8).max(8) as u32;
    let size = reach * 2;
    let mut out = Rgba { width: size, height: size, data: vec![0; (size * size * 4) as usize] };
    // En 8 bpp une tuile compte pour deux n°
    let units = if tiles.bpp8 { 2 } else { 1 };
    // 2D : la mémoire des OBJ est une planche de 32 n° de large
    let sheet = tiles.width.unwrap_or(32 / units as u32) as usize;
    let pixel = |obj: &Obj, x: usize, y: usize| -> u8 {
        let w = obj.width as usize;
        let i = if cells.mapping >= 4 {
            let t = obj.tile as usize / units;
            let (col, row) = (t % sheet + x / 8, t / sheet + y / 8);
            if tiles.linear { (row * 8 + y % 8) * sheet * 8 + col * 8 + x % 8 } else { (row * sheet + col) * 64 + (y % 8) * 8 + x % 8 }
        } else {
            // 1D : le n° compte des blocs de 32 << mode octets
            let bytes = obj.tile as usize * (32usize << cells.mapping);
            let first = bytes * 2 / units;
            if tiles.linear { first + y * w + x } else { first + ((y / 8) * (w / 8) + x / 8) * 64 + (y % 8) * 8 + x % 8 }
        };
        tiles.pixels.get(i).copied().unwrap_or(0)
    };
    // Le premier OBJ est au-dessus : on dessine du dernier au premier
    for obj in objs.iter().rev() {
        for y in 0..obj.height as usize {
            for x in 0..obj.width as usize {
                let sx = if obj.hflip { obj.width as usize - 1 - x } else { x };
                let sy = if obj.vflip { obj.height as usize - 1 - y } else { y };
                let color = palette.color(obj.bank as usize, pixel(obj, sx, sy), tiles.bpp8);
                put(&mut out, reach as i64 + obj.x as i64 + x as i64, reach as i64 + obj.y as i64 + y as i64, color);
            }
        }
    }
    out
}

// --------------------------------------------------------------------------------------------
//  Animations de cellules (NANR)
// --------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimFrame {
    pub cell: usize,
    pub x: i32,
    pub y: i32,
    /// Durée en images (1/60 s).
    pub duration: u16,
}

/// Les séquences d'animation du fichier (chacune, ses images).
pub fn read_animations(file: &[u8]) -> Result<Vec<Vec<AnimFrame>>> {
    let b = block(file, &[b"KNBA", b"ABNK"])?;
    let count = u16_at(b, 0)? as usize;
    let sequences_at = u32_at(b, 4)? as usize;
    let frames_at = u32_at(b, 8)? as usize;
    let data_at = u32_at(b, 12)? as usize;
    let mut sequences = Vec::with_capacity(count);
    for s in 0..count {
        let at = sequences_at + s * 16;
        let frames = u16_at(b, at)? as usize;
        // 0 = n° de cellule seul ; 1 = + rotation, échelle, position ; 2 = + position
        let data_type = u16_at(b, at + 4)? & 0xFF;
        let first = u32_at(b, at + 12)? as usize;
        let mut sequence = Vec::with_capacity(frames);
        for f in 0..frames {
            let frame = frames_at + first + f * 8;
            let data = data_at + u32_at(b, frame)? as usize;
            let duration = u16_at(b, frame + 4)?;
            let cell = u16_at(b, data)? as usize;
            let position_at = match data_type {
                1 => Some(data + 12),
                2 => Some(data + 4),
                _ => None,
            };
            let (x, y) = match position_at {
                Some(p) => (u16_at(b, p)? as i16 as i32, u16_at(b, p + 2)? as i16 as i32),
                None => (0, 0),
            };
            sequence.push(AnimFrame { cell, x, y, duration });
        }
        sequences.push(sequence);
    }
    Ok(sequences)
}

/// Les images d'une séquence, toutes de la même taille, (0, 0) au centre.
pub fn render_sequence(cells: &Cells, frames: &[AnimFrame], tiles: &Tiles, palette: &Palette) -> Vec<(Rgba, u16)> {
    let images: Vec<Rgba> = frames.iter().map(|f| render_cell(cells, f.cell, tiles, palette)).collect();
    let reach = frames.iter().zip(&images).map(|(f, i)| f.x.abs().max(f.y.abs()) as u32 + i.width / 2).max().unwrap_or(8).max(8);
    let size = reach * 2;
    frames
        .iter()
        .zip(&images)
        .map(|(f, image)| {
            let mut out = Rgba { width: size, height: size, data: vec![0; (size * size * 4) as usize] };
            let half = image.width as i64 / 2;
            blit(&mut out, image, reach as i64 + f.x as i64 - half, reach as i64 + f.y as i64 - half);
            (out, f.duration.max(1))
        })
        .collect()
}

fn blit(out: &mut Rgba, image: &Rgba, px: i64, py: i64) {
    for y in 0..image.height as i64 {
        for x in 0..image.width as i64 {
            let at = ((y as u32 * image.width + x as u32) * 4) as usize;
            let color = [image.data[at], image.data[at + 1], image.data[at + 2], image.data[at + 3]];
            put(out, px + x, py + y, color);
        }
    }
}

// --------------------------------------------------------------------------------------------
//  Cellules JNCD et mise en page JNCL (Nintendogs)
// --------------------------------------------------------------------------------------------

/// Un JNCD : cellules (6 octets : largeur, hauteur, premier OBJ, nombre), OBJ (12 octets : attributs
/// OAM, 0, n° de bloc de 128 octets, taille en blocs), et les tuiles dans le même fichier.
pub fn read_jcells(file: &[u8]) -> Result<(Cells, Tiles)> {
    if file.get(0..4) != Some(b"JNCD") {
        bail!("pas un fichier JNCD");
    }
    let count = u16_at(file, 6)? as usize;
    let cells_at = u32_at(file, 8)? as usize;
    let objs_at = u32_at(file, 12)? as usize;
    let data_at = u32_at(file, 16)? as usize;
    let data_size = u32_at(file, 20)? as usize;
    let mut cells = Vec::with_capacity(count);
    let mut sizes = Vec::with_capacity(count);
    let mut bpp8 = false;
    for c in 0..count {
        let at = cells_at + c * 6;
        let size = u16_at(file, at)?;
        sizes.push(((size & 0xFF) as u32, (size >> 8) as u32));
        let first = u16_at(file, at + 2)? as usize;
        let n = u16_at(file, at + 4)? as usize;
        let mut objs = Vec::with_capacity(n);
        for o in first..first + n {
            let p = objs_at + o * 12;
            let (a0, a1, a2) = (u16_at(file, p)?, u16_at(file, p + 2)?, u16_at(file, p + 4)?);
            let (width, height) = OBJ_SIZES[((a0 >> 14) & 3).min(2) as usize][((a1 >> 14) & 3) as usize];
            bpp8 |= a0 & 0x2000 != 0;
            objs.push(Obj {
                x: ((a1 & 0x1FF) as i32) << 23 >> 23,
                y: (a0 & 0xFF) as i8 as i32,
                width,
                height,
                tile: u16_at(file, p + 8)? as u32,
                bank: (a2 >> 12) as u32,
                hflip: a0 & 0x100 == 0 && a1 & 0x1000 != 0,
                vflip: a0 & 0x100 == 0 && a1 & 0x2000 != 0,
                bpp8: a0 & 0x2000 != 0,
            });
        }
        cells.push(objs);
    }
    let data = file.get(data_at..).unwrap_or(&[]);
    let data = &data[..data_size.min(data.len())];
    let pixels = if bpp8 { data.to_vec() } else { data.iter().flat_map(|b| [b & 15, b >> 4]).collect() };
    // Mode 2 = blocs de 128 octets
    Ok((Cells { cells, mapping: 2, sizes }, Tiles { bpp8, linear: false, pixels, width: None, height: None }))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    /// Ce que désigne x, y : 0 = le bord gauche ou haut de la cellule, 1 = son centre, 2 = son bord droit ou bas.
    pub anchor_x: u8,
    pub anchor_y: u8,
    pub cell: usize,
}

/// Un JNCL : où poser chaque cellule à l'écran (8 octets : x, y, n° ; x et y sur 12 bits, l'ancrage au-dessus).
pub fn read_jlayout(file: &[u8]) -> Result<Vec<Placement>> {
    if file.get(0..4) != Some(b"JNCL") {
        bail!("pas un fichier JNCL");
    }
    let count = u16_at(file, 6)? as usize;
    let mut placements = Vec::with_capacity(count);
    for i in 0..count {
        let at = 8 + i * 8;
        let coordinate = |v: u16| ((v & 0xFFF) as i32) << 20 >> 20;
        let (x, y) = (u16_at(file, at)?, u16_at(file, at + 2)?);
        placements.push(Placement { x: coordinate(x), y: coordinate(y), anchor_x: (x >> 12) as u8, anchor_y: (y >> 12) as u8, cell: u32_at(file, at + 4)? as usize });
    }
    Ok(placements)
}

/// La mise en page sur un écran DS ; la première cellule de la liste au-dessus.
pub fn render_layout(placements: &[Placement], cells: &Cells, tiles: &Tiles, palette: &Palette) -> Rgba {
    let mut out = Rgba { width: 256, height: 192, data: vec![0; 256 * 192 * 4] };
    for p in placements.iter().rev() {
        let image = render_cell(cells, p.cell, tiles, palette);
        let half = image.width as i64 / 2;
        let (w, h) = cells.sizes.get(p.cell).copied().unwrap_or((0, 0));
        let start = |at: i32, size: u32, anchor: u8| at as i64 - size as i64 * anchor as i64 / 2;
        blit(&mut out, &image, start(p.x, w, p.anchor_x) - half, start(p.y, h, p.anchor_y) - half);
    }
    out
}

// --------------------------------------------------------------------------------------------
//  Tuiles, palette et écran bruts (sans en-tête : .nbfc, .nbfp, .nbfs)
// --------------------------------------------------------------------------------------------

pub fn raw_palette(file: &[u8]) -> Palette {
    Palette { colors: file.chunks_exact(2).map(|c| bgr555(u16::from_le_bytes([c[0], c[1]]))).collect() }
}

/// Plus de 16 couleurs dans la palette : 8 bits par pixel ; sinon 4.
pub fn raw_tiles(file: &[u8], palette: Option<&Palette>) -> Tiles {
    let bpp8 = palette.is_some_and(|p| p.colors.len() > 16);
    let pixels = if bpp8 { file.to_vec() } else { file.iter().flat_map(|b| [b & 15, b >> 4]).collect() };
    Tiles { bpp8, linear: false, pixels, width: None, height: None }
}

/// Le fichier ne dit pas sa taille : 32x24 tuiles (l'écran DS) ou le plus proche d'un carré de 32 de large.
pub fn raw_screen(file: &[u8]) -> Screen {
    let entries: Vec<u16> = file.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let width = if entries.len() >= 64 * 32 { 64 } else { 32 };
    let height = (entries.len() as u32 / width).max(1);
    Screen { width: width * 8, height: height * 8, entries }
}

// --------------------------------------------------------------------------------------------
//  Polices (NFTR)
// --------------------------------------------------------------------------------------------

/// Tous les caractères de la police, 32 par ligne (blancs, l'alpha donne le tracé). Bloc CGLP : largeur et
/// hauteur de la case, taille d'une case en octets, ligne de base, largeur maximale, bits par pixel, sens.
pub fn render_font(file: &[u8]) -> Result<(Rgba, usize)> {
    let b = block(file, &[b"PLGC", b"CGLP"])?;
    let (Some(&width), Some(&height), Some(&bpp)) = (b.first(), b.get(1), b.get(6)) else { bail!("police trop courte") };
    let (width, height, bpp) = (width as usize, height as usize, bpp as usize);
    let cell = u16_at(b, 2)? as usize;
    if width == 0 || height == 0 || cell == 0 || bpp == 0 || bpp > 8 {
        bail!("police : case {}x{}, {} bits", width, height, bpp);
    }
    let glyphs = b.get(8..).unwrap_or(&[]);
    let count = glyphs.len() / cell;
    const PER_LINE: usize = 32;
    let rows = count.div_ceil(PER_LINE).max(1);
    let (out_w, out_h) = (PER_LINE * (width + 1), rows * (height + 1));
    let mut out = Rgba { width: out_w as u32, height: out_h as u32, data: vec![0; out_w * out_h * 4] };
    let top = (1usize << bpp) - 1;
    for (g, glyph) in glyphs.chunks_exact(cell).enumerate() {
        let (ox, oy) = ((g % PER_LINE) * (width + 1), (g / PER_LINE) * (height + 1));
        for y in 0..height {
            for x in 0..width {
                let bit = (y * width + x) * bpp;
                let Some(byte) = glyph.get(bit / 8) else { continue };
                let next = glyph.get(bit / 8 + 1).copied().unwrap_or(0);
                let value = ((((*byte as usize) << 8) | next as usize) >> (16 - bpp - bit % 8)) & top;
                if value != 0 {
                    put(&mut out, (ox + x) as i64, (oy + y) as i64, [255, 255, 255, (value * 255 / top) as u8]);
                }
            }
        }
    }
    Ok((out, count))
}

// --------------------------------------------------------------------------------------------
//  Textures et images du moteur maison (LXET, " PMB")
// --------------------------------------------------------------------------------------------

/// Une texture `LXET` : taille, pixels, puis format de texture de la DS (u32), largeur et hauteur (u16).
pub struct Texel<'a> {
    pub format: u32,
    pub width: u32,
    pub height: u32,
    data: &'a [u8],
}

pub fn read_texel(file: &[u8]) -> Result<Texel<'_>> {
    if file.get(0..4) != Some(b"LXET") {
        bail!("pas une texture LXET");
    }
    let size = u32_at(file, 4)? as usize;
    let Some(data) = file.get(8..8 + size) else { bail!("texture coupee") };
    let (format, width, height) = (u32_at(file, 8 + size)?, u16_at(file, 12 + size)? as u32, u16_at(file, 14 + size)? as u32);
    if width == 0 || height == 0 || width > 1024 || height > 1024 {
        bail!("texture de {}x{}", width, height);
    }
    Ok(Texel { format, width, height, data })
}

/// La texture en RGBA. Couleur 0 transparente. Formats de la DS : 1 A3I5, 2 4 couleurs, 3 16 couleurs,
/// 4 256 couleurs, 6 A5I3, 7 couleurs directes (5, le 4x4 compressé, n'est pas lu).
pub fn render_texel(texel: &Texel, palette: &Palette) -> Result<Rgba> {
    let (width, height) = (texel.width as usize, texel.height as usize);
    let count = width * height;
    let d = texel.data;
    let mut out = Rgba { width: texel.width, height: texel.height, data: vec![0; count * 4] };
    // Le magenta (0x7C1F) est la couleur transparente de ces moteurs
    const COLOR_KEY: [u8; 4] = [255, 0, 255, 255];
    let pal = |index: u8| {
        let color = palette.color(0, index, true);
        if color == COLOR_KEY { [0; 4] } else { color }
    };
    let mut set = |i: usize, c: [u8; 4]| out.data[i * 4..i * 4 + 4].copy_from_slice(&c);
    match texel.format {
        1 | 6 => {
            let (index_bits, scale) = if texel.format == 1 { (5, 7) } else { (3, 31) };
            for (i, b) in d.iter().take(count).enumerate() {
                let mut c = palette.colors.get((*b & ((1 << index_bits) - 1)) as usize).copied().unwrap_or([255, 0, 255, 255]);
                c[3] = ((*b >> index_bits) as u32 * 255 / scale) as u8;
                set(i, c);
            }
        }
        2 => {
            for i in 0..count {
                let Some(b) = d.get(i / 4) else { break };
                set(i, pal((b >> ((i % 4) * 2)) & 3));
            }
        }
        3 => {
            for i in 0..count {
                let Some(b) = d.get(i / 2) else { break };
                set(i, pal(if i % 2 == 0 { b & 15 } else { b >> 4 }));
            }
        }
        4 => {
            for (i, b) in d.iter().take(count).enumerate() {
                set(i, pal(*b));
            }
        }
        7 => {
            for (i, c) in d.chunks_exact(2).take(count).enumerate() {
                let v = u16::from_le_bytes([c[0], c[1]]);
                let mut color = bgr555(v);
                color[3] = if v & 0x8000 != 0 { 255 } else { 0 };
                set(i, color);
            }
        }
        other => bail!("format de texture {} non lu", other),
    }
    Ok(out)
}

/// Une image ` PMB` : taille, puis une image 8 bits compressée en LZ77. 49 152 octets = l'écran de la DS,
/// sinon un carré, sinon 256 pixels de large.
pub fn render_engine_bitmap(file: &[u8], palette: &Palette) -> Result<Rgba> {
    if file.get(0..4) != Some(b" PMB") {
        bail!("pas une image PMB");
    }
    let Some(pixels) = crate::decompress_any(file.get(8..).unwrap_or(&[])) else { bail!("image PMB non compressee") };
    let count = pixels.len() as u32;
    let root = (count as f64).sqrt() as u32;
    let (width, height) = if count == 256 * 192 {
        (256, 192)
    } else if root * root == count {
        (root, root)
    } else {
        (256, count.div_ceil(256).max(1))
    };
    let mut out = Rgba { width, height, data: vec![0; (width * height * 4) as usize] };
    for (i, index) in pixels.iter().enumerate() {
        out.data[i * 4..i * 4 + 4].copy_from_slice(&palette.color(0, *index, true));
    }
    Ok(out)
}

impl<'a> Texel<'a> {
    pub fn new(format: u32, width: u32, height: u32, data: &'a [u8]) -> Self {
        Texel { format, width, height, data }
    }
}

/// Une texture `.bstga` (déjà décompressée) : format de texture de la DS, puis à 16 largeur et hauteur (u16), à 20
/// position et taille des pixels, à 36 position et taille de la palette.
pub fn read_bstga(file: &[u8]) -> Result<(Texel<'_>, Palette)> {
    let (format, width, height) = (u32_at(file, 0)?, u16_at(file, 16)? as u32, u16_at(file, 18)? as u32);
    let (data_at, data_size, pal_at, pal_size) = (u32_at(file, 20)? as usize, u32_at(file, 24)? as usize, u32_at(file, 36)? as usize, u32_at(file, 40)? as usize);
    let Some(data) = file.get(data_at..data_at + data_size) else { bail!("pixels hors du fichier") };
    if width == 0 || height == 0 || width > 1024 || height > 1024 {
        bail!("texture de {}x{}", width, height);
    }
    Ok((Texel::new(format, width, height, data), raw_palette(file.get(pal_at..pal_at + pal_size).unwrap_or(&[]))))
}

/// La taille et le format d'une texture sans en-tête `.ntft` : on la suppose carrée (sinon deux fois plus large),
/// et sa palette dit le nombre de bits : 4 couleurs = 2 bits, 16 = 4 bits, sinon 8.
pub fn raw_texture<'a>(file: &'a [u8], palette: &Palette) -> Texel<'a> {
    let (format, bits) = match palette.colors.len() {
        0..=4 => (2, 2),
        5..=16 => (3, 4),
        _ => (4, 8),
    };
    let pixels = (file.len() * 8 / bits).max(1) as u32;
    let mut width = 1;
    while width * width < pixels {
        width *= 2;
    }
    Texel::new(format, width, (pixels / width).max(1), file)
}

/// Une image `.bmp` sans compression : 4, 8 ou 24 bits, les lignes de bas en haut.
pub fn read_bmp(file: &[u8]) -> Result<Rgba> {
    if file.get(0..2) != Some(b"BM") {
        bail!("pas une image BMP");
    }
    let data_at = u32_at(file, 10)? as usize;
    let header = u32_at(file, 14)? as usize;
    let (width, raw_height) = (u32_at(file, 18)? as usize, u32_at(file, 22)? as i32);
    let (bits, compression) = (u16_at(file, 28)? as usize, u32_at(file, 30)?);
    let height = raw_height.unsigned_abs() as usize;
    if compression != 0 || !matches!(bits, 4 | 8 | 24) || width == 0 || height == 0 || width > 4096 || height > 4096 {
        bail!("BMP : {} bits, compression {}, {}x{}", bits, compression, width, height);
    }
    let colors: Vec<[u8; 4]> = file.get(14 + header..data_at).unwrap_or(&[]).chunks_exact(4).map(|c| [c[2], c[1], c[0], 255]).collect();
    let stride = (width * bits).div_ceil(32) * 4;
    let mut out = Rgba { width: width as u32, height: height as u32, data: vec![0; width * height * 4] };
    for y in 0..height {
        let row = if raw_height > 0 { height - 1 - y } else { y };
        let Some(line) = file.get(data_at + row * stride..data_at + (row + 1) * stride) else { continue };
        for x in 0..width {
            let color = match bits {
                24 => [line[x * 3 + 2], line[x * 3 + 1], line[x * 3], 255],
                8 => colors.get(line[x] as usize).copied().unwrap_or([255, 0, 255, 255]),
                _ => {
                    let byte = line[x / 2];
                    colors.get(if x % 2 == 0 { byte >> 4 } else { byte & 15 } as usize).copied().unwrap_or([255, 0, 255, 255])
                }
            };
            out.data[(y * width + x) * 4..(y * width + x) * 4 + 4].copy_from_slice(&color);
        }
    }
    Ok(out)
}

/// Une planche de tuiles avec sa palette au début : 256 couleurs et des tuiles de 64 octets, sinon 16 couleurs et des
/// tuiles de 32 octets (Heroes of Mana). `None` si la taille ne tombe pas juste ou si le début n'a rien d'une palette.
pub fn read_palette_tiles(file: &[u8]) -> Option<(Palette, Tiles)> {
    const MIN_TILES: usize = 4;
    let colors_in = |bytes: &[u8]| {
        let mut seen: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        seen.sort_unstable();
        seen.dedup();
        seen.len()
    };
    let (size, bpp8) = if file.len() >= 512 + MIN_TILES * 64 && (file.len() - 512) % 64 == 0 && colors_in(&file[..512]) >= 8 {
        (512, true)
    } else if file.len() >= 32 + MIN_TILES * 32 && (file.len() - 32) % 32 == 0 && colors_in(&file[..32]) >= 4 {
        (32, false)
    } else {
        return None;
    };
    let data = &file[size..];
    let pixels = if bpp8 { data.to_vec() } else { data.iter().flat_map(|p| [p & 15, p >> 4]).collect() };
    Some((raw_palette(&file[..size]), Tiles { bpp8, linear: false, pixels, width: None, height: None }))
}

/// Le contenu décompressé d'un `.z` de Tom and Jerry. Trois formes :
/// - image : 0, largeur et hauteur (u16), attributs, taille, pixels en tuiles, puis un u32 et sa palette ;
/// - écran plein format : 1, 256 couleurs, 8 octets, 1024 entrées d'écran, tuiles de 64 octets ;
/// - texture : format de texture de la DS (u16), tailles (u8 : 8 << n), pixels, puis 1 (u16), taille de la palette
///   (u16), 0 (u32), palette.
/// `None` pour le reste (polices, maillages, palettes seules...).
pub fn read_z_image(file: &[u8]) -> Option<Rgba> {
    let first = u32_at(file, 0).ok()?;
    match first {
        0 => {
            let (width, height) = (u16_at(file, 4).ok()? as u32, u16_at(file, 6).ok()? as u32);
            let size = u32_at(file, 12).ok()? as usize;
            let data = file.get(16..16 + size)?;
            if width == 0 || height == 0 || width > 1024 || height > 1024 || width % 8 != 0 {
                return None;
            }
            let palette = raw_palette(file.get(16 + size + 4..)?);
            let bpp8 = size as u32 * 8 / (width * height) >= 8;
            let pixels = if bpp8 { data.to_vec() } else { data.iter().flat_map(|p| [p & 15, p >> 4]).collect() };
            let tiles = Tiles { bpp8, linear: false, pixels, width: None, height: None };
            Some(render_tiles(&tiles, &palette, 0, width / 8))
        }
        1 => {
            const MAP_AT: usize = 4 + 512 + 8;
            const MAP_SIZE: usize = 2048;
            let palette = raw_palette(file.get(4..4 + 512)?);
            let entries = file.get(MAP_AT..MAP_AT + MAP_SIZE)?.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            let tiles = Tiles { bpp8: true, linear: false, pixels: file.get(MAP_AT + MAP_SIZE..)?.to_vec(), width: None, height: None };
            Some(render_screen(&Screen { width: 256, height: 192, entries }, &tiles, &palette))
        }
        _ => {
            let format = u16_at(file, 0).ok()? as u32;
            let (Some(&sw), Some(&sh)) = (file.get(2), file.get(3)) else { return None };
            if !matches!(format, 1 | 2 | 3 | 4 | 6 | 7) || sw > 7 || sh > 7 {
                return None;
            }
            let (width, height) = (8u32 << sw, 8u32 << sh);
            let bits = [0, 8, 2, 4, 8, 0, 8, 16][format as usize];
            let size = (width * height) as usize * bits / 8;
            let data = file.get(4..4 + size)?;
            let palette = if format == 7 {
                Palette { colors: Vec::new() }
            } else {
                let palette_size = u16_at(file, 4 + size + 2).ok()? as usize;
                raw_palette(file.get(4 + size + 8..4 + size + 8 + palette_size)?)
            };
            render_texel(&Texel::new(format, width, height, data), &palette).ok()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magenta_is_transparent_in_textures() {
        let palette = Palette { colors: vec![[0; 4], [255, 0, 255, 255], [9, 9, 9, 255]] };
        let data = [1u8, 2];
        let image = render_texel(&Texel::new(4, 2, 1, &data), &palette).unwrap();
        assert_eq!(image.data, [0, 0, 0, 0, 9, 9, 9, 255]);
    }

    #[test]
    fn small_tile_sets_are_square() {
        let tiles = Tiles { bpp8: false, linear: false, pixels: vec![0; 16 * 64], width: None, height: None };
        assert_eq!(tiles.default_width(), 4);
    }

    #[test]
    fn reads_fonts_of_any_depth() {
        // 3 bits : le pixel (2, 0) est à cheval sur deux octets
        let mut file = vec![0u8; 16];
        file[0..4].copy_from_slice(b"RTFN");
        file[12] = 16;
        file[14] = 1;
        file.extend(b"PLGC");
        file.extend(20u32.to_le_bytes());
        file.extend([4, 1, 4, 0, 0, 0, 3, 0]);
        file.extend([0b0000_0011, 0b1000_0000, 0, 0]);
        let (image, count) = render_font(&file).unwrap();
        assert_eq!(count, 1);
        let alpha = |x: usize| image.data[x * 4 + 3];
        assert_eq!((alpha(0), alpha(1), alpha(2)), (0, 0, 255));
    }

    #[test]
    fn reads_palette_tile_sheets() {
        let mut file: Vec<u8> = (0..256u16).flat_map(|c| c.to_le_bytes()).collect();
        file.extend([1u8; 64 * 4]);
        let (palette, tiles) = read_palette_tiles(&file).unwrap();
        assert_eq!((palette.colors.len(), tiles.bpp8, tiles.count()), (256, true, 4));
        assert!(read_palette_tiles(&[0u8; 512 + 256]).is_none(), "que du noir : pas une palette");
    }

    #[test]
    fn reads_tom_and_jerry_textures() {
        // 8x8 en 4 bits : pixels 32 octets, puis palette de 2 couleurs
        let mut file = vec![3, 0, 0, 0];
        file.extend([0x11u8; 32]);
        file.extend([1, 0, 4, 0, 0, 0, 0, 0]);
        file.extend([0, 0, 0x1F, 0]);
        let image = read_z_image(&file).unwrap();
        assert_eq!((image.width, image.height), (8, 8));
        assert_eq!(&image.data[0..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn reads_tom_and_jerry_images() {
        let mut file = vec![0, 0, 0, 0, 8, 0, 8, 0, 0, 0, 0, 0x40, 32, 0, 0, 0];
        file.extend([0x11u8; 32]);
        file.extend([2, 0, 0, 0, 0, 0, 0x1F, 0]);
        let image = read_z_image(&file).unwrap();
        assert_eq!(&image.data[0..4], &[255, 0, 0, 255]);
        assert!(read_z_image(&[0x80, 0, 0, 0]).is_none());
    }

    #[test]
    fn guesses_raw_texture_sizes() {
        let palette = Palette { colors: vec![[0; 4]; 16] };
        let data = [0u8; 2048];
        let texel = raw_texture(&data, &palette);
        assert_eq!((texel.format, texel.width, texel.height), (3, 64, 64));
        let wide = [0u8; 4096];
        let texel = raw_texture(&wide, &palette);
        assert_eq!((texel.width, texel.height), (128, 64));
    }

    #[test]
    fn reads_a_small_bmp() {
        let mut file = vec![0u8; 54];
        file[0..2].copy_from_slice(b"BM");
        file[10] = 54;
        file[14] = 40;
        file[18] = 1;
        file[22] = 1;
        file[28] = 24;
        file.extend([1, 2, 3, 0]);
        assert_eq!(read_bmp(&file).unwrap().data, [3, 2, 1, 255]);
    }

    #[test]
    fn reads_an_engine_texture() {
        let mut file = b"LXET\x04\0\0\0".to_vec();
        file.extend([0, 1, 1, 0]);
        file.extend(4u32.to_le_bytes());
        file.extend(2u16.to_le_bytes());
        file.extend(2u16.to_le_bytes());
        let texel = read_texel(&file).unwrap();
        assert_eq!((texel.format, texel.width, texel.height), (4, 2, 2));
        let palette = Palette { colors: vec![[1, 2, 3, 255], [9, 9, 9, 255]] };
        let image = render_texel(&texel, &palette).unwrap();
        assert_eq!(&image.data[0..8], &[0, 0, 0, 0, 9, 9, 9, 255], "couleur 0 transparente");
    }

    #[test]
    fn reads_engine_palettes() {
        let mut file = b"TTLP\x04\0\0\0".to_vec();
        file.extend([0x1F, 0, 0xE0, 3, 0, 0]);
        assert_eq!(read_palette(&file).unwrap().colors.len(), 2);
    }
}
