//! Formats 2D du SDK Nitro : palettes NCLR, tuiles NCGR, écrans NSCR, cellules NCER.
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

    /// Largeur en tuiles pour l'aperçu de la planche.
    pub fn default_width(&self) -> u32 {
        match self.width {
            Some(w) if w > 0 && w as usize <= self.count() => w,
            _ => (self.count() as u32).clamp(1, 32),
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
    let b = block(file, &[b"NRCS", b"SCRN"])?;
    let width = u16_at(b, 0)? as u32;
    let height = u16_at(b, 2)? as u32;
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
    Ok(Cells { cells, mapping })
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
