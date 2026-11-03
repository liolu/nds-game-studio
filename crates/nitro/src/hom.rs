//! Formats maison de Heroes of Mana : sprites en blocs (`faceshap`, `monstershap`, `sallyshap`, `fmap`), archive 3D
//! textures des modèles 3D (`model00.bin`...), `Archive.sar` et sa table `.mat`, scripts d'événements.

use crate::g2d::{self, AnimFrame, Cells, Obj, Palette, Tiles};
use crate::{bgr555, Rgba};

fn u32_at(b: &[u8], off: usize) -> Option<usize> {
    b.get(off..off + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize)
}

/// Taille (largeur, hauteur) d'un OBJ : forme puis taille, comme dans la mémoire des OBJ de la DS.
const OBJ_SIZES: [[(u32, u32); 4]; 3] = [
    [(8, 8), (16, 16), (32, 32), (64, 64)],
    [(16, 8), (32, 8), (32, 16), (64, 32)],
    [(8, 16), (8, 32), (16, 32), (32, 64)],
];

/// Un sprite de Heroes of Mana : six blocs précédés de leur taille (u32) qui finissent pile avec le fichier.
/// 1. les animations : (première étape, nombre d'étapes) en u16 ;
/// 2. les étapes : (n° d'image, durée) en u16 ;
/// 3. les images : (premier OBJ, nombre d'OBJ) en u16 ;
/// 4. les OBJ, 8 octets : x, y (i16), n° de tuile (u16), groupe de couleurs, puis taille << 4 | forme << 2 | retournements ;
/// 5. les tuiles 4 bpp ;
/// 6. la palette (16 couleurs par groupe).
pub struct Shape {
    pub animations: Vec<Vec<AnimFrame>>,
    pub cells: Cells,
    pub tiles: Tiles,
    pub palette: Palette,
}

/// Les six blocs, ou `None` si le fichier n'est pas fait ainsi.
fn shape_blocks(file: &[u8]) -> Option<[&[u8]; 6]> {
    let mut blocks = [&file[..0]; 6];
    let mut at = 0;
    for block in &mut blocks {
        let size = u32_at(file, at)?;
        *block = file.get(at + 4..at + 4 + size)?;
        at += 4 + size;
    }
    let [anims, steps, frames, objs, tiles, palette] = blocks;
    let fits = at == file.len() && !anims.is_empty() && anims.len() % 4 == 0 && steps.len() % 4 == 0 && frames.len() % 4 == 0;
    (fits && objs.len() % 8 == 0 && tiles.len() % 32 == 0 && !palette.is_empty() && palette.len() % 32 == 0).then_some(blocks)
}

pub fn read_shape(file: &[u8]) -> Option<Shape> {
    let [anims, steps, frames, objs, tiles, palette] = shape_blocks(file)?;
    let pairs = |b: &[u8]| -> Vec<(usize, usize)> { b.chunks_exact(4).map(|c| (u16::from_le_bytes([c[0], c[1]]) as usize, u16::from_le_bytes([c[2], c[3]]) as usize)).collect() };
    let objs: Vec<Obj> = objs
        .chunks_exact(8)
        .map(|o| {
            let flags = o[7];
            let (width, height) = OBJ_SIZES[((flags >> 2) & 3).min(2) as usize][((flags >> 4) & 3) as usize];
            Obj {
                x: i16::from_le_bytes([o[0], o[1]]) as i32,
                y: i16::from_le_bytes([o[2], o[3]]) as i32,
                width,
                height,
                tile: u16::from_le_bytes([o[4], o[5]]) as u32,
                bank: o[6] as u32,
                hflip: flags & 1 != 0,
                vflip: flags & 2 != 0,
                bpp8: false,
            }
        })
        .collect();
    let cells = pairs(frames).into_iter().map(|(first, count)| objs.get(first..first + count).map(<[Obj]>::to_vec).unwrap_or_default()).collect();
    let steps = pairs(steps);
    let animations = pairs(anims)
        .into_iter()
        .map(|(first, count)| steps.get(first..first + count).unwrap_or_default().iter().map(|&(cell, duration)| AnimFrame { cell, x: 0, y: 0, duration: duration as u16 }).collect())
        .collect();
    let tiles = tiles_4bpp(tiles);
    // Tuiles comptées par blocs de 32 octets : la mémoire des OBJ en 1D 32 Ko
    Some(Shape { animations, cells: Cells { cells, mapping: 0, sizes: Vec::new() }, tiles, palette: g2d::raw_palette(palette) })
}

impl Shape {
    /// Les images d'une animation ; une animation sans étape montre la première image.
    pub fn render(&self, animation: usize) -> Vec<(Rgba, u16)> {
        let fallback = [AnimFrame { cell: 0, x: 0, y: 0, duration: 1 }];
        let frames = self.animations.get(animation).filter(|a| !a.is_empty()).map_or(&fallback[..], |a| &a[..]);
        g2d::render_sequence(&self.cells, frames, &self.tiles, &self.palette)
    }
}

/// Les blocs d'en-tête d'une archive `sar ` (`sar `, `mbr `, `cmp `, `lz7 `...) et le mot qui suit chacun.
pub fn sar_summary(file: &[u8]) -> Option<Vec<String>> {
    if file.get(0..4) != Some(b"sar ") {
        return None;
    }
    let is_tag = |t: &[u8]| t.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b' ');
    let head = &file[..file.len().min(64)];
    Some((0..head.len().saturating_sub(7)).step_by(4).filter(|&at| is_tag(&head[at..at + 4])).map(|at| format!("{}  {:#x}", String::from_utf8_lossy(&head[at..at + 4]), u32_at(file, at + 4).unwrap_or(0))).collect())
}

/// La table `.mat` d'une archive : (position, taille) de chaque morceau, qui se suivent sans trou.
pub fn mat_entries(file: &[u8]) -> Option<Vec<(usize, usize)>> {
    let pairs: Vec<(usize, usize)> = file.chunks_exact(8).map(|c| (u32_at(c, 0).unwrap_or(0), u32_at(c, 4).unwrap_or(0))).collect();
    let follow = pairs.windows(2).all(|w| w[0].0 + w[0].1 <= w[1].0 && w[1].0 - (w[0].0 + w[0].1) < 4);
    (file.len() % 8 == 0 && pairs.len() >= 2 && follow).then_some(pairs)
}

/// Un emplacement vide d'une table : quelques octets à zéro, compressés ou non. Compressés (LZ77 0x10), ce sont des
/// octets d'en-tête puis des zéros (drapeaux et octets copiés tels quels) : le décompresseur ne prend pas des flux si
/// courts.
pub fn is_empty_slot(file: &[u8]) -> bool {
    const MAX: usize = 16;
    let zeros = |b: &[u8]| b.iter().all(|b| *b == 0);
    match file {
        [0x10, size, 0, 0, rest @ ..] => (1..=MAX as u8).contains(size) && rest.len() <= 2 * MAX && zeros(rest),
        _ => file.len() <= MAX && zeros(file),
    }
}

/// Les commandes d'un script d'événement : mots de 32 bits, huit par ligne.
pub fn script_lines(file: &[u8]) -> Vec<String> {
    let words: Vec<String> = file.chunks(4).map(|c| {
        let mut w = [0u8; 4];
        w[..c.len()].copy_from_slice(c);
        format!("{:08x}", u32::from_le_bytes(w))
    }).collect();
    words.chunks(8).enumerate().map(|(row, chunk)| format!("{:06x}  {}", row * 32, chunk.join(" "))).collect()
}

/// Les textures d'un modèle (`model00.bin`...) : 16 octets d'en-tête, une liste de n° (nombre puis n°), les textures
/// (nombre, puis taille u32, format u16 1 = 4 bpp / 2 = 8 bpp, u16, largeur et hauteur u16), leurs pixels, puis
/// une palette par texture (nombre, tailles, couleurs). La géométrie qui suit n'est pas lue.
pub fn model_textures(file: &[u8]) -> Option<Vec<Rgba>> {
    const MAX: usize = 1024;
    let ids = u32_at(file, 16)?;
    let mut at = 20 + ids.checked_mul(4).filter(|n| *n < MAX)?;
    let count = u32_at(file, at)?;
    if count == 0 || count > MAX {
        return None;
    }
    at += 4;
    let mut heads = Vec::with_capacity(count);
    for _ in 0..count {
        let head = file.get(at..at + 12)?;
        let (size, format) = (u32_at(head, 0)?, head[4]);
        let (width, height) = (u16::from_le_bytes([head[8], head[9]]) as usize, u16::from_le_bytes([head[10], head[11]]) as usize);
        let bits = match format {
            1 => 4,
            2 => 8,
            _ => return None,
        };
        if width == 0 || height == 0 || width * height * bits / 8 != size {
            return None;
        }
        heads.push((size, bits, width, height));
        at += 12;
    }
    let mut pixels = Vec::with_capacity(count);
    for &(size, ..) in &heads {
        pixels.push(file.get(at..at + size)?);
        at += size;
    }
    if u32_at(file, at)? != count {
        return None;
    }
    let sizes: Vec<usize> = (0..count).map(|i| u32_at(file, at + 4 + i * 4)).collect::<Option<_>>()?;
    at += 4 + count * 4;
    let mut images = Vec::with_capacity(count);
    for ((&(_, bits, width, height), data), size) in heads.iter().zip(pixels).zip(sizes) {
        let colors: Vec<[u8; 4]> = file.get(at..at + size)?.chunks_exact(2).map(|c| bgr555(u16::from_le_bytes([c[0], c[1]]))).collect();
        at += size;
        let mut out = Rgba { width: width as u32, height: height as u32, data: Vec::with_capacity(width * height * 4) };
        for i in 0..width * height {
            let index = if bits == 8 { data[i] } else if i % 2 == 0 { data[i / 2] & 15 } else { data[i / 2] >> 4 };
            out.data.extend(colors.get(index as usize).copied().unwrap_or([255, 0, 255, 255]));
        }
        images.push(out);
    }
    Some(images)
}

/// Une planche de Heroes of Mana (`story.dat`, `title.dat`...) : 256 couleurs (16 groupes) puis des tuiles 4 bpp ; la
/// fin, qui n'est pas faite de tuiles (fonds, sprites), est montrée comme des tuiles aussi.
pub fn read_sheet(file: &[u8]) -> Option<(Palette, Tiles)> {
    const PALETTE: usize = 512;
    const MIN_TILES: usize = 16;
    let palette = file.get(..PALETTE)?;
    let mut colors: Vec<&[u8]> = palette.chunks_exact(2).collect();
    colors.sort_unstable();
    colors.dedup();
    let data = &file[PALETTE..];
    if colors.len() < 16 || data.len() < MIN_TILES * 32 {
        return None;
    }
    Some((g2d::raw_palette(palette), tiles_4bpp(&data[..data.len() / 32 * 32])))
}

/// Une animation 3D maison (`modelanim01.bin`) : nombre d'images, puis 1.0 en flottant.
pub fn is_model_anim(file: &[u8]) -> bool {
    u32_at(file, 0).is_some_and(|frames| (1..=10_000).contains(&frames)) && file.get(4..8) == Some(&1.0f32.to_le_bytes()[..])
}

/// Des tuiles 4 bpp sans en-tête.
pub fn tiles_4bpp(data: &[u8]) -> Tiles {
    Tiles { bpp8: false, linear: false, pixels: data.iter().flat_map(|b| [b & 15, b >> 4]).collect(), width: None, height: None, attrs: Vec::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_slots_compressed_or_not() {
        assert!(is_empty_slot(&[0, 0, 0, 0]));
        assert!(is_empty_slot(&[0x10, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert!(!is_empty_slot(&[0x10, 4, 0, 0, 0, 1, 0, 0, 0]));
        assert!(!is_empty_slot(&[0, 0, 1, 0]));
    }

    #[test]
    fn reads_a_sprite_of_six_blocks() {
        let block = |data: &[u8]| [&(data.len() as u32).to_le_bytes()[..], data].concat();
        // Une animation d'une étape : l'image 0 pendant 5, faite d'un OBJ 8x8 en (-4, -4), tuile 0, couleur 1 partout
        let file = [block(&[0, 0, 1, 0]), block(&[0, 0, 5, 0]), block(&[0, 0, 1, 0]), block(&[0xFC, 0xFF, 0xFC, 0xFF, 0, 0, 0, 0]), block(&[0x11; 32]), block(&[0x1F, 0, 0xE0, 0x03].repeat(8))].concat();
        let shape = read_shape(&file).unwrap();
        assert_eq!(shape.animations[0][0].duration, 5);
        let frames = shape.render(0);
        let (image, _) = &frames[0];
        // Le centre de l'image est le (0, 0) du sprite : l'OBJ le couvre, en vert (couleur 1)
        let at = ((image.height / 2 * image.width + image.width / 2) * 4) as usize;
        assert_eq!(&image.data[at..at + 4], &[0, 255, 0, 255]);
        assert!(read_shape(&file[..file.len() - 1]).is_none());
    }
}
