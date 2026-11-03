//! Formats du moteur Amaze dans `rom.bin` (The Simpsons Game), sans signature : chacun est reconnu à sa structure.
//!
//! - Sprite : un descripteur, ses tuiles et une palette, dans des fichiers voisins. Les tuiles sont une suite de blocs du
//!   codeur du jeu, un par image (`romarc::own_chain`) ; une image seule est donc dépaquetée d'office.
//! - Planche de niveau : les tuiles 8 bpp d'un niveau, en Huffman canonique.

use crate::g2d::{Palette, Tiles};
use crate::romarc::own_chain;
use crate::Rgba;

/// Un OBJ de la DS : position dans l'image, taille en pixels.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Obj {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Obj {
    fn tiles(&self) -> usize {
        (self.width * self.height / 64) as usize
    }
}

pub struct Frame {
    /// Position de ses tuiles : celle du bloc dans le fichier de tuiles compressé, sinon dans les tuiles elles-mêmes.
    pub tiles_at: usize,
    pub objs: Vec<Obj>,
}

impl Frame {
    fn tile_count(&self) -> usize {
        self.objs.iter().map(Obj::tiles).sum()
    }
}

pub struct Sprite {
    pub bpp8: bool,
    pub frames: Vec<Frame>,
}

impl Sprite {
    fn tile_bytes(&self) -> usize {
        if self.bpp8 {
            64
        } else {
            32
        }
    }
}

const OBJ_SIZES: [(u32, u32); 12] = [(8, 8), (16, 16), (32, 32), (64, 64), (16, 8), (32, 8), (32, 16), (64, 32), (8, 16), (8, 32), (16, 32), (32, 64)];

fn u16_at(b: &[u8], at: usize) -> Option<usize> {
    b.get(at..at + 2).map(|s| u16::from_le_bytes([s[0], s[1]]) as usize)
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn signed14(v: u32) -> i32 {
    (v as i32) << 18 >> 18
}

/// En-tête de 16 octets : largeur, hauteur (u8), boîte (4 i16), nombre d'images, octets par tuile / 32 (u16), points et boîtes
/// de chaque image (u8, u8). Puis une position u16 par image, comptée depuis la fin de l'en-tête. Une image : nombre d'OBJ (u8),
/// position de ses tuiles / 4 (u24), largeur, hauteur (u8), x, y (i16), les points (2 octets chacun) et les boîtes (8 octets), puis
/// un u32 par OBJ : x sur 14 bits, y sur 14 bits, code de taille sur 4 bits (forme * 4 + taille, comme sur la DS).
pub fn read_sprite(file: &[u8]) -> Option<Sprite> {
    let (count, depth) = (u16_at(file, 10)?, u16_at(file, 12)?);
    let (points, boxes) = (*file.get(14)? as usize, *file.get(15)? as usize);
    if count == 0 || !matches!(depth, 1 | 2) || u16_at(file, 16)? != count * 2 {
        return None;
    }
    let mut frames = Vec::with_capacity(count);
    for i in 0..count {
        let at = 16 + u16_at(file, 16 + i * 2)?;
        let end = if i + 1 < count { 16 + u16_at(file, 18 + i * 2)? } else { file.len() };
        let head = u32_at(file, at)?;
        let objs_at = at + 10 + points * 2 + boxes * 8;
        let obj_count = (head & 0xFF) as usize;
        let objs_end = objs_at + obj_count * 4;
        // La dernière image va jusqu'à la fin du fichier, à l'alignement près
        if end > file.len() || objs_end > end || (i + 1 < count && objs_end != end) || end - objs_end >= 4 {
            return None;
        }
        let mut objs = Vec::with_capacity(obj_count);
        for k in 0..obj_count {
            let v = u32_at(file, objs_at + k * 4)?;
            let (width, height) = *OBJ_SIZES.get((v >> 28) as usize)?;
            objs.push(Obj { x: signed14(v & 0x3FFF), y: signed14(v >> 14 & 0x3FFF), width, height });
        }
        frames.push(Frame { tiles_at: (head >> 8) as usize * 4, objs });
    }
    Some(Sprite { bpp8: depth == 2, frames })
}

/// Ces tuiles vont-elles avec ce descripteur ? Compressées : un bloc par image, à la position et à la taille annoncées.
/// Sinon, une seule image dont les tuiles font tout le fichier.
pub fn tiles_match(sprite: &Sprite, file: &[u8]) -> bool {
    let size = sprite.tile_bytes();
    if let Some(blocks) = own_chain(file) {
        return blocks.len() == sprite.frames.len() && blocks.iter().zip(&sprite.frames).all(|((at, data), frame)| *at == frame.tiles_at && data.len() == frame.tile_count() * size);
    }
    let needed = sprite.frames.iter().map(|f| f.tiles_at + f.tile_count() * size).max().unwrap_or(0);
    needed > 0 && sprite.frames[0].tiles_at == 0 && needed.div_ceil(4) == file.len().div_ceil(4)
}

/// Une image par image du descripteur, sur un même cadre pour que l'animation ne saute pas.
pub fn render_sprite(sprite: &Sprite, tiles: &[u8], palette: &Palette) -> Vec<Rgba> {
    let blocks = own_chain(tiles);
    let objs = || sprite.frames.iter().flat_map(|f| &f.objs);
    let (left, top) = (objs().map(|o| o.x).min().unwrap_or(0), objs().map(|o| o.y).min().unwrap_or(0));
    let width = (objs().map(|o| o.x + o.width as i32).max().unwrap_or(1) - left).clamp(1, 2048) as u32;
    let height = (objs().map(|o| o.y + o.height as i32).max().unwrap_or(1) - top).clamp(1, 2048) as u32;
    let size = sprite.tile_bytes();
    sprite
        .frames
        .iter()
        .enumerate()
        .map(|(i, frame)| {
            let data: &[u8] = match &blocks {
                Some(list) => list.get(i).map_or(&[], |b| b.1.as_slice()),
                None => tiles.get(frame.tiles_at..).unwrap_or(&[]),
            };
            let mut out = Rgba { width, height, data: vec![0; (width * height * 4) as usize] };
            let mut first = frame.tile_count();
            // Le premier OBJ passe devant : on dessine en partant du dernier, ses tuiles partent de la fin
            for obj in frame.objs.iter().rev() {
                first -= obj.tiles();
                let per_row = (obj.width / 8) as usize;
                for t in 0..obj.tiles() {
                    let Some(tile) = data.get((first + t) * size..(first + t + 1) * size) else { continue };
                    let (ox, oy) = (obj.x - left + (t % per_row) as i32 * 8, obj.y - top + (t / per_row) as i32 * 8);
                    for p in 0..64usize {
                        let index = if sprite.bpp8 { tile[p] as usize } else { (tile[p / 2] as usize >> (p % 2 * 4)) & 15 };
                        if index == 0 {
                            continue;
                        }
                        let (x, y) = (ox + (p % 8) as i32, oy + (p / 8) as i32);
                        if let (Some(color), true) = (palette.colors.get(index), x >= 0 && y >= 0 && (x as u32) < width && (y as u32) < height) {
                            let at = (y as usize * width as usize + x as usize) * 4;
                            out.data[at..at + 4].copy_from_slice(color);
                        }
                    }
                }
            }
            out
        })
        .collect()
}

/// L'en-tête d'une planche de niveau. Son premier u16 ressemble parfois à un en-tête de `rom.bin` (la taille 0x2360 = sorte 6) :
/// `romarc::unpack` le demande pour ne pas décoder ce qui n'est pas compressé.
pub(crate) fn is_level_tiles(file: &[u8]) -> bool {
    let (Some(first), Some(count)) = (u16_at(file, 0), u16_at(file, 2)) else { return false };
    let Some(head) = u32_at(file, first + 4) else { return false };
    count != 0 && file[4..].first() == Some(&0x60) && head & 0xFF == 0x20 && (head >> 8) as usize == count * 64
}

/// Planche d'un niveau : taille du 1er bloc (u16), nombre de tuiles (u16), ce bloc (codeur du jeu), puis les tuiles :
/// u32 (0x20 + taille << 8), 19 nombres de codes par longueur, 21 octets (un vide, puis leurs totaux cumulés), les symboles, et
/// le flux, lu du bit de poids faible au bit de poids fort de chaque octet. Il contient deux tuiles de plus que le compte annoncé,
/// quelques bits de bourrage après.
pub fn read_level_tiles(file: &[u8]) -> Option<Tiles> {
    if !is_level_tiles(file) {
        return None;
    }
    let (first, count) = (u16_at(file, 0)?, u16_at(file, 2)?);
    let lengths = file.get(first + 8..first + 27)?;
    let symbol_count: usize = lengths.iter().map(|c| *c as usize).sum();
    let symbols = file.get(first + 48..first + 48 + symbol_count)?;
    // Méthode canonique : premier code, nombre de codes et premier symbole de chaque longueur
    let (mut code, mut index, mut table) = (0u32, 0u32, [(0u32, 0u32, 0u32); 19]);
    for (l, c) in lengths.iter().enumerate() {
        table[l] = (code, *c as u32, index);
        code = (code + *c as u32) << 1;
        index += *c as u32;
    }
    let stream = file.get((first + 48 + symbol_count).next_multiple_of(4)..)?;
    let total = (count + 2) * 64;
    let mut pixels = Vec::with_capacity(total);
    let (mut value, mut length) = (0u32, 0usize);
    'read: for byte in stream {
        for bit in 0..8 {
            value = value << 1 | (*byte as u32 >> bit & 1);
            let (start, number, base) = table[length];
            length += 1;
            if value >= start && value - start < number {
                pixels.push(*symbols.get((base + value - start) as usize)?);
                (value, length) = (0, 0);
                if pixels.len() == total {
                    break 'read;
                }
            } else if length == table.len() {
                return None;
            }
        }
    }
    pixels.truncate(pixels.len() / 64 * 64);
    (pixels.len() >= count * 64).then_some(Tiles { bpp8: true, linear: false, pixels, width: None, height: None, attrs: Vec::new() })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Une image de deux OBJ : 8x8 en (0, 0), 16x8 en (8, 0).
    fn descriptor() -> Vec<u8> {
        let mut d = vec![24, 8];
        d.extend([0u8; 8]); // boîte
        d.extend(1u16.to_le_bytes()); // 1 image
        d.extend(1u16.to_le_bytes());
        d.extend([0, 0]); // sans points ni boîtes
        d.extend(2u16.to_le_bytes());
        d.push(2); // 2 OBJ
        d.extend([0, 0, 0]);
        d.extend([24, 8, 0, 0, 0, 0]);
        d.extend(0u32.to_le_bytes());
        d.extend((4u32 << 28 | 8).to_le_bytes());
        d
    }

    #[test]
    fn reads_a_descriptor() {
        let s = read_sprite(&descriptor()).unwrap();
        assert_eq!(s.frames.len(), 1);
        assert_eq!(s.frames[0].objs[1], Obj { x: 8, y: 0, width: 16, height: 8 });
        assert_eq!(s.frames[0].tile_count(), 3);
        assert!(read_sprite(&[0; 32]).is_none());
    }

    #[test]
    fn negative_positions_on_14_bits() {
        assert_eq!(signed14(0x3FEB), -21);
        assert_eq!(signed14(0x000B), 11);
    }

    #[test]
    fn draws_each_obj_at_its_place_with_colour_0_transparent() {
        let s = read_sprite(&descriptor()).unwrap();
        // Tuile 0 : couleur 1 ; tuile 1 : couleur 0 ; tuile 2 : couleur 2
        let mut tiles = vec![0x11; 32];
        tiles.extend([0; 32]);
        tiles.extend([0x22; 32]);
        assert!(tiles_match(&s, &tiles));
        assert!(!tiles_match(&s, &tiles[..64]));
        let palette = Palette { colors: vec![[0, 0, 0, 255], [255, 0, 0, 255], [0, 255, 0, 255]] };
        let frames = render_sprite(&s, &tiles, &palette);
        assert_eq!((frames[0].width, frames[0].height), (24, 8));
        let at = |x: usize| &frames[0].data[x * 4..x * 4 + 4];
        assert_eq!(at(0), [255, 0, 0, 255]);
        assert_eq!(at(8), [0, 0, 0, 0]);
        assert_eq!(at(16), [0, 255, 0, 255]);
    }

    #[test]
    fn reads_level_tiles_low_bit_first() {
        // Deux codes de 1 bit : 0 = 7, 1 = 9 ; 1 tuile annoncée + 2 de plus
        let mut file = vec![4, 0, 1, 0, 0x60, 0, 0, 0];
        file.extend(((64u32 << 8) | 0x20).to_le_bytes());
        let mut lengths = [0u8; 16];
        lengths[0] = 2;
        file.extend(lengths);
        file.extend([0u8; 24]);
        file.extend([7, 9]);
        while file.len() % 4 != 0 {
            file.push(0);
        }
        file.push(0b0000_0010);
        file.extend([0u8; 23]);
        let tiles = read_level_tiles(&file).unwrap();
        assert_eq!(tiles.pixels.len(), 3 * 64);
        assert_eq!(tiles.pixels[0..3], [7, 9, 7]);
    }
}
