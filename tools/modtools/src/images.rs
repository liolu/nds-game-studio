//! Le chemin du retour des images : PNG -> palette, tuiles, écran, cellules, textures de la DS, écrits au même format que
//! le fichier d'origine. Les couleurs viennent de la palette du jeu, souvent partagée entre plusieurs images : on la garde,
//! et un pixel qui n'y est pas prend la couleur la plus proche. Pour de vraies nouvelles couleurs, on modifie la palette.

use std::collections::HashMap;

use nds_nitro::{g2d, Rgba};

use crate::container::{rd16, rd32, wr16, wr32};

/// Couleur RGBA 8 bits -> BGR555, l'inverse de ce que fait le lecteur (`v * 255 / 31`).
pub fn to_bgr555(c: [u8; 4]) -> u16 {
    let five = |v: u8| ((v as u32 * 31 + 127) / 255) as u16;
    five(c[0]) | (five(c[1]) << 5) | (five(c[2]) << 10)
}

fn distance(a: [u8; 4], b: [u8; 4]) -> i32 {
    let d = |i: usize| (a[i] as i32 - b[i] as i32).pow(2);
    d(0) * 3 + d(1) * 4 + d(2) * 2
}

/// Le n° de la couleur la plus proche dans `colors[from..to]`, compté depuis `from`. Le 0 est le transparent, sauf si `use0`.
fn nearest(colors: &[[u8; 4]], from: usize, to: usize, pixel: [u8; 4], use0: bool, cache: &mut HashMap<(usize, [u8; 4]), u8>) -> u8 {
    const VISIBLE: u8 = 128;
    if pixel[3] < VISIBLE && !use0 {
        return 0;
    }
    *cache.entry((from, pixel)).or_insert_with(|| {
        let first = if use0 { 0 } else { 1 };
        let end = to.min(colors.len());
        (from + first..end).min_by_key(|&i| distance(colors[i], pixel)).map_or(0, |i| (i - from) as u8)
    })
}

// --------------------------------------------------------------------------------------------
//  Où sont les données dans chaque variante de fichier
// --------------------------------------------------------------------------------------------

/// Position des données (après ses 8 octets) d'un bloc, et position du bloc.
fn block(file: &[u8], names: &[&[u8; 4]]) -> Option<(usize, usize)> {
    let mut at = rd16(file, 12).max(16);
    for _ in 0..rd16(file, 14).max(1) {
        let magic = file.get(at..at + 4)?;
        let size = rd32(file, at + 4);
        if names.iter().any(|n| n.as_slice() == magic) {
            return Some((at + 8, at));
        }
        if size < 8 {
            return None;
        }
        at += size;
    }
    None
}

/// Les octets des couleurs d'un fichier de palette (le fichier entier s'il n'a pas d'en-tête).
fn palette_span(file: &[u8]) -> Option<(usize, usize)> {
    let span = match file.get(0..4).unwrap_or(&[]) {
        b"TTLP" => (8, rd32(file, 4)),
        b"NCL\0" => (8, rd32(file, 4) * 2),
        b"NCCL" => {
            let (b, at) = block(file, &[b"PALT"])?;
            (b + 8, at + rd32(file, at + 4) - b - 8)
        }
        b"RLCN" | b"NCLR" | b"RPCN" => {
            let (b, at) = block(file, &[b"TTLP", b"PLTT"])?;
            let data = b + rd32(file, b + 12);
            let room = at + rd32(file, at + 4) - data;
            let size = rd32(file, b + 8);
            (data, if size == 0 || size > room { room } else { size })
        }
        _ => (0, file.len()),
    };
    (span.0 + span.1 <= file.len()).then_some(span)
}

/// Nouvelle palette, même taille que l'ancienne.
pub fn write_palette(file: &[u8], colors: &[[u8; 4]]) -> Result<Vec<u8>, String> {
    let (at, size) = palette_span(file).ok_or("palette illisible")?;
    let mut out = file.to_vec();
    for (i, color) in colors.iter().take(size / 2).enumerate() {
        wr16(&mut out, at + i * 2, to_bgr555(*color) as usize);
    }
    Ok(out)
}

/// La palette en petits carrés de `SWATCH` pixels, 16 par ligne : chaque carré se peint directement.
pub const SWATCH: u32 = 16;

pub fn palette_image(palette: &g2d::Palette) -> Rgba {
    let rows = palette.banks() as u32;
    let (width, height) = (16 * SWATCH, rows * SWATCH);
    let mut data = vec![0; (width * height * 4) as usize];
    for (i, color) in palette.colors.iter().enumerate() {
        let (cx, cy) = (i as u32 % 16, i as u32 / 16);
        for y in 0..SWATCH {
            for x in 0..SWATCH {
                let p = (((cy * SWATCH + y) * width + cx * SWATCH + x) * 4) as usize;
                data[p..p + 4].copy_from_slice(&[color[0], color[1], color[2], 255]);
            }
        }
    }
    Rgba { width, height, data }
}

/// Les couleurs d'une image de `palette_image` : on lit le centre de chaque carré.
pub fn colors_from_image(image: &Rgba, count: usize) -> Result<Vec<[u8; 4]>, String> {
    if image.width != 16 * SWATCH || image.height < (count as u32).div_ceil(16) * SWATCH {
        return Err(format!("palette : image de {}x{}, la taille d'origine est attendue", image.width, image.height));
    }
    Ok((0..count as u32)
        .map(|i| {
            let (x, y) = (i % 16 * SWATCH + SWATCH / 2, i / 16 * SWATCH + SWATCH / 2);
            let p = ((y * image.width + x) * 4) as usize;
            [image.data[p], image.data[p + 1], image.data[p + 2], 255]
        })
        .collect())
}

fn pack_pixels(pixels: &[u8], bpp8: bool) -> Vec<u8> {
    if bpp8 { pixels.to_vec() } else { pixels.chunks(2).map(|p| (p[0] & 15) | (p.get(1).copied().unwrap_or(0) << 4)).collect() }
}

/// De nouveaux pixels de tuiles dans le fichier, éventuellement plus nombreux que les anciens.
pub fn write_tiles(file: &[u8], tiles: &g2d::Tiles) -> Result<Vec<u8>, String> {
    let data = pack_pixels(&tiles.pixels, tiles.bpp8);
    let magic = file.get(0..4).unwrap_or(&[]);
    let replace = |at: usize, old: usize| {
        let mut out = file[..at].to_vec();
        out.extend(&data);
        out.extend(file.get(at + old..).unwrap_or(&[]));
        out
    };
    match magic {
        b"NCG\0" => {
            let mut out = replace(8, file.len() - 8);
            wr16(&mut out, 4, tiles.count());
            Ok(out)
        }
        b"NCCG" => {
            let (b, at) = block(file, &[b"CHAR"]).ok_or("tuiles illisibles")?;
            let old = at + rd32(file, at + 4) - b - 12;
            if data.len() != old {
                return Err("NCCG : le nombre de tuiles ne peut pas changer".into());
            }
            Ok(replace(b + 12, old))
        }
        b"RGCN" | b"NCGR" | b"RCBN" => {
            let (b, at) = block(file, &[b"RAHC", b"CHAR"]).ok_or("tuiles illisibles")?;
            let (old, offset) = (rd32(file, b + 16), rd32(file, b + 20));
            let room = at + rd32(file, at + 4) - b - offset;
            let old = if old == 0 || old > room { room } else { old };
            let mut out = replace(b + offset, old);
            if data.len() != old {
                wr32(&mut out, b + 16, data.len());
                let block_size = rd32(file, at + 4) + data.len() - old;
                wr32(&mut out, at + 4, block_size);
                let total = out.len();
                wr32(&mut out, 8, total);
                let width = rd16(file, b + 2);
                if width != 0xFFFF && width != 0 {
                    wr16(&mut out, b, tiles.count() / width);
                }
            }
            Ok(out)
        }
        _ => Ok(data),
    }
}

/// De nouvelles entrées d'écran dans le fichier, en même nombre.
pub fn write_screen(file: &[u8], entries: &[u16]) -> Result<Vec<u8>, String> {
    let (at, wide) = match file.get(0..4).unwrap_or(&[]) {
        b"NSC\0" => (12, true),
        b"NCSC" => (block(file, &[b"SCRN"]).ok_or("ecran illisible")?.0 + 16, true),
        b"RCSN" | b"NSCR" => {
            let b = block(file, &[b"NRCS", b"SCRN"]).ok_or("ecran illisible")?.0;
            let (width, height, size) = (rd16(file, b) as u32, rd16(file, b + 2) as u32, rd32(file, b + 8));
            (b + 12, size >= (width / 8 * (height / 8)) as usize * 2)
        }
        _ => (0, true),
    };
    let mut out = file.to_vec();
    for (i, entry) in entries.iter().enumerate() {
        if wide {
            wr16(&mut out, at + i * 2, *entry as usize);
        } else {
            out[at + i] = *entry as u8;
        }
    }
    Ok(out)
}

// --------------------------------------------------------------------------------------------
//  Planche, écran, cellule
// --------------------------------------------------------------------------------------------

fn pixel_at(image: &Rgba, x: u32, y: u32) -> [u8; 4] {
    let p = ((y * image.width + x) * 4) as usize;
    [image.data[p], image.data[p + 1], image.data[p + 2], image.data[p + 3]]
}

/// La couleur que montre un n° de la palette : comme le lecteur, 0 = transparent.
fn shown(palette: &g2d::Palette, bank: usize, index: u8, bpp8: bool) -> [u8; 4] {
    if index == 0 {
        return [0; 4];
    }
    let at = if bpp8 { index as usize } else { bank * 16 + index as usize };
    palette.colors.get(at).copied().unwrap_or([255, 0, 255, 255])
}

/// Même couleur à l'oeil : deux pixels transparents se valent, quelle que soit leur couleur.
fn same(a: [u8; 4], b: [u8; 4]) -> bool {
    (a[3] < 128 && b[3] < 128) || (a[3] >= 128 && b[3] >= 128 && a[..3] == b[..3])
}

fn index_for(palette: &g2d::Palette, bank: usize, bpp8: bool, pixel: [u8; 4], cache: &mut HashMap<(usize, [u8; 4]), u8>) -> u8 {
    let (from, to) = if bpp8 { (0, 256) } else { (bank * 16, bank * 16 + 16) };
    nearest(&palette.colors, from, to, pixel, false, cache)
}

/// Comme `render_tiles` à l'envers : l'image doit avoir la taille exportée.
pub fn sheet_from_image(tiles: &mut g2d::Tiles, palette: &g2d::Palette, bank: usize, width: u32, image: &Rgba) -> Result<(), String> {
    let rows = (tiles.count() as u32).div_ceil(width.max(1)).max(1);
    if image.width != width * 8 || image.height != rows * 8 {
        return Err(format!("image de {}x{}, {}x{} attendu", image.width, image.height, width * 8, rows * 8));
    }
    let mut cache = HashMap::new();
    let bpp8 = tiles.bpp8;
    // Une fin de tuile incomplète n'est pas dessinée : elle ne change pas
    let drawn = if tiles.linear { tiles.pixels.len() } else { tiles.count() * 64 };
    for (i, slot) in tiles.pixels.iter_mut().take(drawn).enumerate() {
        let i = i as u32;
        let (x, y, tile) = if tiles.linear {
            (i % image.width, i / image.width, 0)
        } else {
            let tile = i / 64;
            ((tile % width) * 8 + (i % 64) % 8, (tile / width) * 8 + (i % 64) / 8, tile as usize)
        };
        if y >= image.height {
            continue;
        }
        let tile_bank = tiles.attrs.get(tile).map_or(bank, |attr| *attr as usize);
        let wanted = pixel_at(image, x, y);
        if !same(wanted, shown(palette, tile_bank, *slot, bpp8)) {
            *slot = index_for(palette, tile_bank, bpp8, wanted, &mut cache);
        }
    }
    Ok(())
}

/// Le n° d'entrée de la tuile (`tx`, `ty`), comme `render_screen`.
fn screen_entry(screen: &g2d::Screen, tx: u32, ty: u32) -> usize {
    let (wt, ht) = (screen.width / 8, screen.height / 8);
    let blocks = wt > 32 && screen.entries.len() as u32 >= wt * ht;
    (if blocks { ((ty / 32) * (wt / 32) + tx / 32) * 1024 + (ty % 32) * 32 + tx % 32 } else { ty * wt + tx }) as usize
}

const MAX_TILE: usize = 1024;

/// Comme `render_screen` à l'envers. Une tuile dont le dessin change est réécrite si cet écran seul s'en sert à cet endroit,
/// sinon une nouvelle tuile est ajoutée à la fin. Rend le nombre de tuiles ajoutées.
pub fn screen_from_image(screen: &mut g2d::Screen, tiles: &mut g2d::Tiles, palette: &g2d::Palette, image: &Rgba) -> Result<usize, String> {
    if image.width != screen.width || image.height != screen.height {
        return Err(format!("image de {}x{}, {}x{} attendu", image.width, image.height, screen.width, screen.height));
    }
    if tiles.linear {
        return Err("tuiles en bitmap sous un ecran : non gere".into());
    }
    let mut uses = HashMap::<usize, usize>::new();
    for e in &screen.entries {
        *uses.entry((e & 0x3FF) as usize).or_default() += 1;
    }
    let bpp8 = tiles.bpp8;
    let mut added = 0;
    let mut known = HashMap::<[u8; 64], usize>::new();
    let mut cache = HashMap::new();
    for ty in 0..screen.height / 8 {
        for tx in 0..screen.width / 8 {
            let at = screen_entry(screen, tx, ty);
            let Some(&entry) = screen.entries.get(at) else { continue };
            let (tile, bank) = ((entry & 0x3FF) as usize, (entry >> 12) as usize);
            let (hflip, vflip) = (entry & 0x400 != 0, entry & 0x800 != 0);
            let mut drawn = [0u8; 64];
            let mut changed = false;
            for y in 0..8u32 {
                for x in 0..8u32 {
                    let (sx, sy) = (if hflip { 7 - x } else { x }, if vflip { 7 - y } else { y });
                    let old = tiles.pixels.get(tile * 64 + (sy * 8 + sx) as usize).copied().unwrap_or(0);
                    let wanted = pixel_at(image, tx * 8 + x, ty * 8 + y);
                    drawn[(sy * 8 + sx) as usize] = if same(wanted, shown(palette, bank, old, bpp8)) {
                        old
                    } else {
                        changed = true;
                        index_for(palette, bank, bpp8, wanted, &mut cache)
                    };
                }
            }
            if !changed {
                continue;
            }
            let target = if let Some(&again) = known.get(&drawn) {
                again
            } else if uses.get(&tile) == Some(&1) && (tile + 1) * 64 <= tiles.pixels.len() {
                tile
            } else {
                if tiles.count() >= MAX_TILE {
                    return Err(t_too_many());
                }
                added += 1;
                let new = tiles.count();
                tiles.pixels.resize((new + 1) * 64, 0);
                new
            };
            tiles.pixels[target * 64..(target + 1) * 64].copy_from_slice(&drawn);
            known.insert(drawn, target);
            screen.entries[at] = (entry & !0x3FF) | target as u16;
        }
    }
    Ok(added)
}

fn t_too_many() -> String {
    "plus de 1024 tuiles differentes : image trop detaillee pour un ecran de la DS".to_string()
}

/// L'octet d'un pixel d'OBJ dans les tuiles, comme `render_cell`.
fn obj_pixel(cells: &g2d::Cells, tiles: &g2d::Tiles, obj: &g2d::Obj, x: usize, y: usize) -> usize {
    let units = if tiles.bpp8 { 2 } else { 1 };
    let sheet = tiles.width.unwrap_or(32 / units as u32) as usize;
    let w = obj.width as usize;
    if cells.mapping >= 4 {
        let t = obj.tile as usize / units;
        let (col, row) = (t % sheet + x / 8, t / sheet + y / 8);
        if tiles.linear { (row * 8 + y % 8) * sheet * 8 + col * 8 + x % 8 } else { (row * sheet + col) * 64 + (y % 8) * 8 + x % 8 }
    } else {
        let bytes = obj.tile as usize * (32usize << cells.mapping);
        let first = bytes * 2 / units;
        if tiles.linear { first + y * w + x } else { first + ((y / 8) * (w / 8) + x / 8) * 64 + (y % 8) * 8 + x % 8 }
    }
}

/// Comme `render_cell` à l'envers. Seuls les pixels qui changent sont écrits, dans l'OBJ du dessus à cet endroit : deux OBJ peuvent se
/// recouvrir, et celui du dessous ne doit pas prendre le dessin de l'autre. Ne change que les tuiles : si deux cellules partagent une
/// tuile, la dernière a raison.
pub fn cell_from_image(cells: &g2d::Cells, index: usize, tiles: &mut g2d::Tiles, palette: &g2d::Palette, image: &Rgba) -> Result<(), String> {
    let objs = cells.cells.get(index).ok_or("cellule introuvable")?;
    let reach = objs.iter().map(|o| o.x.abs().max((o.x + o.width as i32).abs()).max(o.y.abs()).max((o.y + o.height as i32).abs())).max().unwrap_or(8).max(8) as u32;
    if image.width != reach * 2 || image.height != reach * 2 {
        return Err(format!("image de {}x{}, {}x{} attendu", image.width, image.height, reach * 2, reach * 2));
    }
    let current = g2d::render_cell(cells, index, tiles, palette);
    let mut cache = HashMap::new();
    let bpp8 = tiles.bpp8;
    for py in 0..image.height {
        for px in 0..image.width {
            let wanted = pixel_at(image, px, py);
            if same(wanted, pixel_at(&current, px, py)) {
                continue;
            }
            let clear = wanted[3] < 128;
            for obj in objs {
                let (x, y) = (px as i64 - reach as i64 - obj.x as i64, py as i64 - reach as i64 - obj.y as i64);
                if x < 0 || y < 0 || x >= obj.width as i64 || y >= obj.height as i64 {
                    continue;
                }
                let sx = if obj.hflip { obj.width as usize - 1 - x as usize } else { x as usize };
                let sy = if obj.vflip { obj.height as usize - 1 - y as usize } else { y as usize };
                let at = obj_pixel(cells, tiles, obj, sx, sy);
                let Some(slot) = tiles.pixels.get_mut(at) else { continue };
                // Effacer vaut pour tous les OBJ à cet endroit, sinon celui du dessous réapparaît
                if clear {
                    *slot = 0;
                } else {
                    *slot = index_for(palette, obj.bank as usize, bpp8, wanted, &mut cache);
                    break;
                }
            }
        }
    }
    Ok(())
}

// --------------------------------------------------------------------------------------------
//  Textures 3D
// --------------------------------------------------------------------------------------------

/// Les couleurs d'une palette de texture, lues à la position `at`.
pub fn read_colors(bytes: &[u8], at: usize, count: usize) -> Vec<[u8; 4]> {
    let ch = |v: u16| {
        let five = (v & 31) as u8;
        (five << 3) | (five >> 2)
    };
    (0..count)
        .map(|i| {
            let c = rd16(bytes, at + i * 2) as u16;
            [ch(c), ch(c >> 5), ch(c >> 10), 255]
        })
        .collect()
}

/// Le nombre de couleurs d'une palette pour un format de texture de la DS.
pub fn palette_size(format: u8) -> usize {
    match format {
        1 => 32,
        2 => 4,
        3 => 16,
        4 => 256,
        6 => 8,
        _ => 0,
    }
}

/// Les pixels d'une texture au format de la DS `format` (pas le 5, voir `encode_4x4`).
pub fn encode_texture(format: u8, image: &Rgba, colors: &[[u8; 4]], color0_transparent: bool) -> Result<Vec<u8>, String> {
    let count = (image.width * image.height) as usize;
    let mut cache = HashMap::new();
    let pixel = |i: usize| {
        let p = i * 4;
        [image.data[p], image.data[p + 1], image.data[p + 2], image.data[p + 3]]
    };
    let mut index = |c: [u8; 4], use0: bool| {
        let opaque = if use0 { [c[0], c[1], c[2], 255] } else { c };
        nearest(colors, 0, colors.len(), opaque, use0, &mut cache)
    };
    let use0 = !color0_transparent;
    Ok(match format {
        1 => (0..count).map(|i| (index(pixel(i), true) & 31) | (((pixel(i)[3] as u32 * 7 + 127) / 255) as u8) << 5).collect(),
        6 => (0..count).map(|i| (index(pixel(i), true) & 7) | (((pixel(i)[3] as u32 * 31 + 127) / 255) as u8) << 3).collect(),
        2 => (0..count).collect::<Vec<_>>().chunks(4).map(|b| b.iter().enumerate().fold(0u8, |v, (n, i)| v | (index(pixel(*i), use0) & 3) << (n * 2))).collect(),
        3 => (0..count).collect::<Vec<_>>().chunks(2).map(|b| b.iter().enumerate().fold(0u8, |v, (n, i)| v | (index(pixel(*i), use0) & 15) << (n * 4))).collect(),
        4 => (0..count).map(|i| index(pixel(i), use0)).collect(),
        7 => (0..count)
            .flat_map(|i| {
                let c = pixel(i);
                (to_bgr555(c) | if c[3] >= 128 { 0x8000 } else { 0 }).to_le_bytes()
            })
            .collect(),
        other => return Err(format!("format de texture {other} non gere")),
    })
}

/// Une texture compressée 4x4 (format 5) : chaque bloc garde sa palette et son mode, seuls ses pixels sont rechoisis. Un bloc dont le
/// dessin n'a pas changé (d'après `before`, l'image d'origine) garde ses octets.
pub fn encode_4x4(image: &Rgba, before: &Rgba, data1: &[u8], data2: &[u8], palette: &[u8], base: usize) -> Vec<u8> {
    let (w, h) = (image.width as usize, image.height as usize);
    let (blocks_x, blocks_y) = (w / 4, h / 4);
    let mut out = Vec::with_capacity(blocks_x * blocks_y * 4);
    let blend = |a: [u8; 4], b: [u8; 4], wa: u32, wb: u32| {
        let m = |i: usize| ((wa * a[i] as u32 + wb * b[i] as u32) / (wa + wb)) as u8;
        [m(0), m(1), m(2), 255]
    };
    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let block = by * blocks_x + bx;
            let unchanged = (0..16).all(|i: u32| pixel_at(image, bx as u32 * 4 + i % 4, by as u32 * 4 + i / 4) == pixel_at(before, bx as u32 * 4 + i % 4, by as u32 * 4 + i / 4));
            if unchanged {
                out.extend(data1.get(block * 4..block * 4 + 4).unwrap_or(&[0; 4]));
                continue;
            }
            let extra = rd16(data2, block * 2);
            let (mode, address) = (extra >> 14, (extra & 0x3FFF) << 1);
            let colors = read_colors(palette, base + address * 2, 4);
            let choices: [Option<[u8; 4]>; 4] = match mode {
                0 => [Some(colors[0]), Some(colors[1]), Some(colors[2]), None],
                1 => [Some(colors[0]), Some(colors[1]), Some(blend(colors[0], colors[1], 1, 1)), None],
                2 => [Some(colors[0]), Some(colors[1]), Some(colors[2]), Some(colors[3])],
                _ => [Some(colors[0]), Some(colors[1]), Some(blend(colors[1], colors[0], 3, 5)), Some(blend(colors[0], colors[1], 3, 5))],
            };
            let mut word = 0u32;
            for y in 0..4 {
                for x in 0..4 {
                    let c = pixel_at(image, (bx * 4 + x) as u32, (by * 4 + y) as u32);
                    let texel = if c[3] < 128 && choices[3].is_none() {
                        3
                    } else {
                        (0..4).filter(|n| choices[*n].is_some()).min_by_key(|n| distance(choices[*n].unwrap(), c)).unwrap_or(0)
                    };
                    word |= (texel as u32) << ((y * 4 + x) * 2);
                }
            }
            out.extend(word.to_le_bytes());
        }
    }
    out
}

// --------------------------------------------------------------------------------------------
//  Images des moteurs maison : `.z`, BMP, PMB, polices
// --------------------------------------------------------------------------------------------

/// Deux pixels que le jeu montre pareil. Au bit près : un demi-transparent de texture n'est pas un opaque.
fn identical(a: [u8; 4], b: [u8; 4]) -> bool {
    a == b || (a[3] == 0 && b[3] == 0)
}

fn check_size(image: &Rgba, width: u32, height: u32) -> Result<(), String> {
    if image.width != width || image.height != height {
        return Err(format!("image de {}x{}, {}x{} attendu", image.width, image.height, width, height));
    }
    Ok(())
}

/// Le magenta est la couleur transparente de `render_texel`.
const COLOR_KEY: [u8; 4] = [255, 0, 255, 255];

/// Les pixels d'une texture : ceux qui ne changent pas gardent leur n° de couleur (une palette peut avoir deux fois la même, ou un
/// magenta transparent), les autres prennent la couleur la plus proche.
pub fn texel_from_image(texel: &g2d::Texel, palette: &g2d::Palette, image: &Rgba) -> Result<Vec<u8>, String> {
    check_size(image, texel.width, texel.height)?;
    let before = g2d::render_texel(texel, palette).map_err(|e| e.to_string())?;
    let (format, old) = (texel.format as u8, texel.pixels());
    let mut painted = image.data.clone();
    if texel.opaque && palette.colors.contains(&COLOR_KEY) {
        for pixel in painted.chunks_exact_mut(4).filter(|p| p[3] < 128) {
            pixel.copy_from_slice(&COLOR_KEY);
        }
    }
    let painted = Rgba { width: image.width, height: image.height, data: painted };
    let mut out = encode_texture(format, &painted, &palette.colors, !texel.opaque)?;
    keep_unchanged(format, &mut out, old, image, &before);
    Ok(out)
}

/// Remet les n° de couleur d'origine des pixels que l'image ne change pas.
fn keep_unchanged(format: u8, out: &mut [u8], old: &[u8], image: &Rgba, before: &Rgba) {
    let bits = [0, 8, 2, 4, 8, 0, 8, 16][format as usize];
    for i in 0..(image.width * image.height) as usize {
        let (x, y) = (i as u32 % image.width, i as u32 / image.width);
        if !identical(pixel_at(image, x, y), pixel_at(before, x, y)) {
            continue;
        }
        let at = i * bits / 8;
        if bits >= 8 {
            out[at..at + bits / 8].copy_from_slice(&old[at..at + bits / 8]);
        } else {
            let mask = ((1u16 << bits) - 1) as u8;
            let shift = (i * bits % 8) as u8;
            out[at] = (out[at] & !(mask << shift)) | (old[at] & (mask << shift));
        }
    }
}

/// Le `.z` décompressé avec cette image, et le nombre de tuiles ajoutées à un fond.
pub fn z_from_image(file: &[u8], image: &Rgba) -> Result<(Vec<u8>, usize), String> {
    match g2d::read_z_shape(file).ok_or("image illisible")? {
        g2d::ZShape::Sprite { width, mut tiles, palette, .. } => {
            sheet_from_image(&mut tiles, &palette, 0, width / 8, image)?;
            let pixels = pack_pixels(&tiles.pixels, tiles.bpp8);
            let mut out = file.to_vec();
            out[16..16 + pixels.len()].copy_from_slice(&pixels);
            Ok((out, 0))
        }
        g2d::ZShape::Screen { mut screen, mut tiles, palette, map_at, count_at, .. } => {
            let added = screen_from_image(&mut screen, &mut tiles, &palette, image)?;
            let mut out = file[..map_at].to_vec();
            out.extend(screen.entries.iter().flat_map(|e| e.to_le_bytes()));
            debug_assert_eq!(out.len(), count_at);
            out.extend((tiles.count() as u32).to_le_bytes());
            out.extend(&tiles.pixels);
            Ok((out, added))
        }
        g2d::ZShape::Texture { texel, palette, .. } => {
            let pixels = texel_from_image(&texel, &palette, image)?;
            let mut out = file.to_vec();
            out[4..4 + pixels.len()].copy_from_slice(&pixels);
            Ok((out, 0))
        }
    }
}

/// La palette d'un `.z` image, s'il en a une.
pub fn z_palette(file: &[u8]) -> Option<g2d::Palette> {
    match g2d::read_z_shape(file)? {
        g2d::ZShape::Sprite { palette, .. } | g2d::ZShape::Screen { palette, .. } | g2d::ZShape::Texture { palette, .. } => (!palette.colors.is_empty()).then_some(palette),
    }
}

/// Le `.z` décompressé avec cette palette. Une couleur qui ne change pas garde ses octets.
pub fn z_with_palette(file: &[u8], colors: &[[u8; 4]]) -> Result<Vec<u8>, String> {
    let (palette, at) = match g2d::read_z_shape(file).ok_or("image illisible")? {
        g2d::ZShape::Sprite { palette, palette_at, .. } | g2d::ZShape::Screen { palette, palette_at, .. } | g2d::ZShape::Texture { palette, palette_at, .. } => (palette, palette_at),
    };
    let mut out = file.to_vec();
    for (i, (old, new)) in palette.colors.iter().zip(colors).enumerate() {
        if old[..3] != new[..3] {
            wr16(&mut out, at + i * 2, to_bgr555(*new) as usize);
        }
    }
    Ok(out)
}

/// Une image de `render_engine_bitmap` remise dans ses octets 8 bits : un pixel qui ne change pas garde son n°.
pub fn bitmap_pixels(pixels: &[u8], before: &Rgba, palette: &g2d::Palette, image: &Rgba) -> Result<Vec<u8>, String> {
    check_size(image, before.width, before.height)?;
    let mut cache = HashMap::new();
    let mut out = pixels.to_vec();
    for (i, slot) in out.iter_mut().enumerate() {
        let (x, y) = (i as u32 % image.width, i as u32 / image.width);
        if y < image.height && !same(pixel_at(image, x, y), pixel_at(before, x, y)) {
            *slot = index_for(palette, 0, true, pixel_at(image, x, y), &mut cache);
        }
    }
    Ok(out)
}

/// Une image `.bmp` sans compression, avec les pixels de `image`.
pub fn bmp_from_image(file: &[u8], image: &Rgba) -> Result<Vec<u8>, String> {
    let before = g2d::read_bmp(file).map_err(|e| e.to_string())?;
    check_size(image, before.width, before.height)?;
    let (data_at, header, bits) = (rd32(file, 10), rd32(file, 14), rd16(file, 28));
    let (width, height) = (before.width as usize, before.height as usize);
    let up = (rd32(file, 22) as u32 as i32) > 0;
    let colors: Vec<[u8; 4]> = file.get(14 + header..data_at).unwrap_or(&[]).chunks_exact(4).map(|c| [c[2], c[1], c[0], 255]).collect();
    let stride = (width * bits).div_ceil(32) * 4;
    let mut out = file.to_vec();
    let mut cache = HashMap::new();
    for y in 0..height {
        let row = data_at + (if up { height - 1 - y } else { y }) * stride;
        for x in 0..width {
            let wanted = pixel_at(image, x as u32, y as u32);
            if same(wanted, pixel_at(&before, x as u32, y as u32)) || row + stride > file.len() {
                continue;
            }
            let opaque = [wanted[0], wanted[1], wanted[2], 255];
            match bits {
                24 => out[row + x * 3..row + x * 3 + 3].copy_from_slice(&[opaque[2], opaque[1], opaque[0]]),
                8 => out[row + x] = nearest(&colors, 0, colors.len(), opaque, true, &mut cache),
                _ => {
                    let index = nearest(&colors, 0, colors.len(), opaque, true, &mut cache);
                    let byte = &mut out[row + x / 2];
                    *byte = if x % 2 == 0 { (*byte & 15) | (index << 4) } else { (*byte & 0xF0) | (index & 15) };
                }
            }
        }
    }
    Ok(out)
}

/// La police avec les caractères de cette image de `render_font` : l'alpha donne le tracé, la couleur ne compte pas.
pub fn font_from_image(file: &[u8], image: &Rgba) -> Result<Vec<u8>, String> {
    const PER_LINE: usize = 32;
    let (b, at) = block(file, &[b"PLGC", b"CGLP"]).ok_or("police illisible")?;
    let (width, height, cell, bpp) = (file[b] as usize, file[b + 1] as usize, rd16(file, b + 2), file[b + 6] as usize);
    let glyphs_at = b + 8;
    let end = (at + rd32(file, at + 4)).min(file.len());
    if width == 0 || height == 0 || cell == 0 || bpp == 0 || bpp > 8 {
        return Err("police illisible".into());
    }
    let count = (end - glyphs_at) / cell;
    let rows = count.div_ceil(PER_LINE).max(1);
    check_size(image, (PER_LINE * (width + 1)) as u32, (rows * (height + 1)) as u32)?;
    let top = (1u32 << bpp) - 1;
    let mut out = file.to_vec();
    for g in 0..count {
        let (ox, oy) = ((g % PER_LINE) * (width + 1), (g / PER_LINE) * (height + 1));
        let glyph = glyphs_at + g * cell;
        for y in 0..height {
            for x in 0..width {
                let bit = (y * width + x) * bpp;
                if bit / 8 >= cell {
                    continue;
                }
                let value = (pixel_at(image, (ox + x) as u32, (oy + y) as u32)[3] as u32 * top + 127) / 255;
                let shift = (16 - bpp - bit % 8) as u32;
                let next = if bit / 8 + 1 < cell { out[glyph + bit / 8 + 1] } else { 0 };
                let word = ((out[glyph + bit / 8] as u32) << 8) | next as u32;
                let word = (word & !(top << shift)) | (value << shift);
                out[glyph + bit / 8] = (word >> 8) as u8;
                if bit / 8 + 1 < cell {
                    out[glyph + bit / 8 + 1] = word as u8;
                }
            }
        }
    }
    Ok(out)
}

/// Où sont les textures d'un fichier Nitro 3D.
pub struct Tex0 {
    pub texels: usize,
    pub compressed1: usize,
    pub palettes: usize,
}

pub fn find_tex0(file: &[u8]) -> Option<Tex0> {
    (0..rd16(file, 14)).find_map(|i| {
        let at = rd32(file, 16 + i * 4);
        (file.get(at..at + 4) == Some(b"TEX0")).then(|| Tex0 { texels: at + rd32(file, at + 0x14), compressed1: at + rd32(file, at + 0x24), palettes: at + rd32(file, at + 0x38) })
    })
}

/// Remet une texture modifiée dans le fichier ; `palette_at` = position de sa palette dans le bloc des palettes.
pub fn write_texture(file: &[u8], texture: &nds_nitro::nitro::Texture, palette: Option<&nds_nitro::nitro::Palette>, image: &Rgba) -> Result<Vec<u8>, String> {
    let palette_at = palette.map(|p| p.off as usize);
    let (width, height) = texture.params.dim();
    if image.width != width || image.height != height {
        return Err(format!("image de {}x{}, {}x{} attendu : la DS ne garde que ces tailles", image.width, image.height, width, height));
    }
    let tex0 = find_tex0(file).ok_or("bloc TEX0 absent")?;
    let format = texture.params.format().0;
    let offset = texture.params.offset() as usize;
    let mut out = file.to_vec();
    let before = nds_nitro::nds::decode_texture(texture, palette).map_err(|e| e.to_string())?;
    let before = Rgba { width, height, data: before.0 };
    if format == 5 {
        let at = tex0.palettes + palette_at.ok_or("texture sans palette")?;
        let pixels = encode_4x4(image, &before, &texture.data1, &texture.data2, file, at);
        out[tex0.compressed1 + offset..tex0.compressed1 + offset + pixels.len()].copy_from_slice(&pixels);
    } else {
        let colors = palette_at.map_or_else(Vec::new, |at| read_colors(file, tex0.palettes + at, palette_size(format)));
        // Deux textures peuvent partager leurs pixels avec des palettes différentes : ce qui ne change pas ne bouge pas
        let mut pixels = encode_texture(format, image, &colors, texture.params.is_color0_transparent())?;
        let size = pixels.len();
        keep_unchanged(format, &mut pixels, &file[tex0.texels + offset..tex0.texels + offset + size], image, &before);
        out[tex0.texels + offset..tex0.texels + offset + pixels.len()].copy_from_slice(&pixels);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> g2d::Palette {
        let mut colors = vec![[0, 0, 0, 255]; 32];
        colors[1] = [255, 0, 0, 255];
        colors[2] = [0, 255, 0, 255];
        colors[17] = [0, 0, 255, 255];
        g2d::Palette { colors }
    }

    /// Une texture 8x8 à 4 couleurs dont la 3 est le magenta transparent.
    fn z_texture(key: u8) -> Vec<u8> {
        let mut pixels = vec![0xE4u8; 16];
        pixels[1] = 0xFF;
        [&[2, key, 0, 0], &pixels[..], &[1, 0, 8, 0, 0, 0, 0, 0], &[0x9C, 0x73, 0x1F, 0, 0xE0, 3, 0x1F, 0x7C]].concat()
    }

    #[test]
    fn z_images_come_back_identical() {
        // Texture, image en tuiles, fond : sans changement, les mêmes octets
        let mut sprite = vec![0, 0, 0, 0, 8, 0, 8, 0, 0, 0, 0, 0x40, 32, 0, 0, 0];
        sprite.extend((0..32).map(|i| (i % 3) as u8 | ((i % 2) as u8) << 4));
        sprite.extend([3, 0, 0, 0, 0, 0, 0x1F, 0, 0xE0, 3]);
        let mut screen = vec![0u8; 4 + 512 + 4 + 2048 + 4 + 128];
        screen[0] = 1;
        screen[4 + 2] = 0x1F;
        screen[520 + 2] = 1;
        screen[520 + 2048] = 2;
        screen[520 + 2048 + 4 + 64] = 1;
        for file in [z_texture(0), z_texture(1), sprite, screen] {
            let image = g2d::read_z_image(&file).unwrap();
            assert_eq!(z_from_image(&file, &image).unwrap(), (file.clone(), 0));
            let colors = z_palette(&file).unwrap().colors;
            assert_eq!(z_with_palette(&file, &colors).unwrap(), file);
        }
    }

    #[test]
    fn a_painted_z_shows_what_was_painted() {
        for key in [0, 1] {
            let file = z_texture(key);
            let mut image = g2d::read_z_image(&file).unwrap();
            image.data[0..4].copy_from_slice(&[0, 255, 0, 255]);
            image.data[4..8].copy_from_slice(&[0, 0, 0, 0]);
            let (out, _) = z_from_image(&file, &image).unwrap();
            let again = g2d::read_z_image(&out).unwrap();
            assert_eq!(&again.data[0..4], &[0, 255, 0, 255]);
            assert_eq!(again.data[7], 0, "transparent, cle {key}");
            assert_eq!(again.data[8..], image.data[8..], "le reste ne bouge pas");
        }
    }

    #[test]
    fn a_grown_z_screen_says_its_tile_count() {
        let mut screen = vec![0u8; 4 + 512 + 4 + 2048 + 4 + 64];
        screen[0] = 1;
        screen[4 + 2] = 0x1F;
        screen[8] = 0xE0;
        screen[9] = 3;
        screen[520 + 2048] = 1;
        screen[520 + 2048 + 4] = 1;
        let mut image = g2d::read_z_image(&screen).unwrap();
        image.data[0..4].copy_from_slice(&[0, 255, 0, 255]);
        let (out, added) = z_from_image(&screen, &image).unwrap();
        assert_eq!(added, 1);
        assert_eq!(g2d::read_z_image(&out).unwrap().data, image.data);
    }

    #[test]
    fn a_z_palette_changes_the_image() {
        let file = z_texture(1);
        let mut colors = z_palette(&file).unwrap().colors;
        colors[1] = [0, 0, 255, 255];
        let out = z_with_palette(&file, &colors).unwrap();
        assert_eq!(z_palette(&out).unwrap().colors[1], [0, 0, 255, 255]);
        assert_eq!(z_palette(&out).unwrap().colors[2], colors[2]);
    }

    #[test]
    fn fonts_come_back_at_any_depth() {
        for bpp in [1u8, 2, 3, 4] {
            let (width, height) = (5u8, 3u8);
            let cell = (width as usize * height as usize * bpp as usize).div_ceil(8) as u16;
            let mut file = vec![0u8; 16];
            file[0..4].copy_from_slice(b"RTFN");
            file[12] = 16;
            file[14] = 1;
            file.extend(b"PLGC");
            file.extend((16 + cell as u32 * 2).to_le_bytes());
            file.extend([width, height, cell as u8, 0, 0, 0, bpp, 0]);
            file.extend((0..cell * 2).map(|i| (i as u8).wrapping_mul(73) ^ 0x5A));
            // Le reste de la dernière case n'est pas dessiné : on le met à 0 pour comparer
            let (mut image, _) = g2d::render_font(&file).unwrap();
            let out = font_from_image(&file, &image).unwrap();
            assert_eq!(g2d::render_font(&out).unwrap().0.data, image.data, "{bpp} bits");
            image.data[0..4].copy_from_slice(&[255; 4]);
            let out = font_from_image(&file, &image).unwrap();
            assert_eq!(g2d::render_font(&out).unwrap().0.data, image.data, "{bpp} bits, un pixel peint");
        }
    }

    #[test]
    fn bmps_come_back_in_every_depth() {
        let make = |bits: u16, colors: &[[u8; 4]], row: &[u8]| {
            let data_at = 54 + colors.len() * 4;
            let mut file = vec![0u8; 54];
            file[0..2].copy_from_slice(b"BM");
            file[10] = data_at as u8;
            file[14] = 40;
            file[18] = 2;
            file[22] = 2;
            file[28..30].copy_from_slice(&bits.to_le_bytes());
            file.extend(colors.iter().flatten());
            file.extend(row);
            file.extend(row);
            file
        };
        let colors = [[0, 0, 255, 0], [0, 255, 0, 0]];
        for file in [make(24, &[], &[1, 2, 3, 4, 5, 6, 0, 0]), make(8, &colors, &[0, 1, 0, 0]), make(4, &colors, &[0x10, 0, 0, 0])] {
            let mut image = g2d::read_bmp(&file).unwrap();
            assert_eq!(bmp_from_image(&file, &image).unwrap(), file);
            image.data[0..4].copy_from_slice(&[0, 255, 0, 255]);
            let out = bmp_from_image(&file, &image).unwrap();
            assert_eq!(g2d::read_bmp(&out).unwrap().data, image.data);
        }
    }

    #[test]
    fn colors_survive_the_trip_to_555_and_back() {
        for v in 0..32u16 {
            let c = ((v as u32) * 255 / 31) as u8;
            assert_eq!(to_bgr555([c, c, c, 255]), v | (v << 5) | (v << 10));
        }
    }

    #[test]
    fn a_sheet_comes_back_with_the_same_indexes() {
        let mut tiles = g2d::Tiles { bpp8: false, linear: false, pixels: (0..128).map(|i| (i % 3) as u8).collect(), width: Some(2), height: Some(1), attrs: Vec::new() };
        let before = tiles.pixels.clone();
        let image = g2d::render_tiles(&tiles, &palette(), 0, 2);
        sheet_from_image(&mut tiles, &palette(), 0, 2, &image).unwrap();
        assert_eq!(tiles.pixels, before);
    }

    #[test]
    fn a_painted_pixel_takes_the_nearest_color() {
        let mut tiles = g2d::Tiles { bpp8: false, linear: false, pixels: vec![1; 64], width: Some(1), height: Some(1), attrs: Vec::new() };
        let mut image = g2d::render_tiles(&tiles, &palette(), 0, 1);
        image.data[0..4].copy_from_slice(&[10, 240, 10, 255]);
        image.data[4..8].copy_from_slice(&[0, 0, 0, 0]);
        sheet_from_image(&mut tiles, &palette(), 0, 1, &image).unwrap();
        assert_eq!((tiles.pixels[0], tiles.pixels[1], tiles.pixels[2]), (2, 0, 1));
    }

    #[test]
    fn a_shared_tile_is_split_and_the_others_stay() {
        let mut tiles = g2d::Tiles { bpp8: false, linear: false, pixels: vec![1; 64], width: Some(1), height: Some(1), attrs: Vec::new() };
        let mut screen = g2d::Screen { width: 16, height: 8, entries: vec![0, 0] };
        let mut image = g2d::render_screen(&screen, &tiles, &palette());
        image.data[(8 * 4)..(8 * 4 + 4)].copy_from_slice(&[0, 255, 0, 255]);
        let added = screen_from_image(&mut screen, &mut tiles, &palette(), &image).unwrap();
        assert_eq!(added, 1);
        assert_eq!(screen.entries, [0, 1]);
        assert_eq!(tiles.pixels[0], 1, "la 1re case garde sa tuile");
        assert_eq!(tiles.pixels[64], 2);
        let again = g2d::render_screen(&screen, &tiles, &palette());
        assert_eq!(again.data, image.data);
    }

    #[test]
    fn flipped_entries_are_written_the_right_way() {
        let mut pixels = vec![1u8; 64];
        pixels[7] = 2;
        let mut tiles = g2d::Tiles { bpp8: false, linear: false, pixels, width: Some(1), height: Some(1), attrs: Vec::new() };
        let mut screen = g2d::Screen { width: 8, height: 8, entries: vec![0x400] };
        let mut image = g2d::render_screen(&screen, &tiles, &palette());
        assert_eq!(&image.data[0..4], &[0, 255, 0, 255], "miroir : le pixel 7 est affiche en 0");
        image.data[4..8].copy_from_slice(&[0, 255, 0, 255]);
        screen_from_image(&mut screen, &mut tiles, &palette(), &image).unwrap();
        assert_eq!(tiles.pixels[6], 2);
        assert_eq!(g2d::render_screen(&screen, &tiles, &palette()).data, image.data);
    }

    #[test]
    fn a_cell_comes_back_into_its_tiles() {
        let mut tiles = g2d::Tiles { bpp8: false, linear: false, pixels: vec![1; 64], width: None, height: None, attrs: Vec::new() };
        let obj = g2d::Obj { x: -4, y: -4, width: 8, height: 8, tile: 0, bank: 1, hflip: true, vflip: false, bpp8: false };
        let cells = g2d::Cells { cells: vec![vec![obj]], mapping: 0, sizes: Vec::new() };
        let mut image = g2d::render_cell(&cells, 0, &tiles, &palette());
        let size = image.width as usize;
        // Le coin haut gauche de l'OBJ
        let at = (4 * size + 4) * 4;
        image.data[at..at + 4].copy_from_slice(&[0, 0, 0, 0]);
        cell_from_image(&cells, 0, &mut tiles, &palette(), &image).unwrap();
        assert_eq!(tiles.pixels[7], 0, "miroir horizontal");
        assert_eq!(g2d::render_cell(&cells, 0, &tiles, &palette()).data, image.data);
    }

    #[test]
    fn palette_images_read_back_their_colors() {
        let palette = palette();
        let image = palette_image(&palette);
        let colors = colors_from_image(&image, palette.colors.len()).unwrap();
        assert_eq!(colors[17], [0, 0, 255, 255]);
    }

    #[test]
    fn palette_files_keep_their_header() {
        let mut file = b"TTLP\x04\0\0\0".to_vec();
        file.extend([0x1F, 0, 0xE0, 3]);
        let out = write_palette(&file, &[[0, 0, 255, 255], [255, 255, 255, 255]]).unwrap();
        assert_eq!(&out[8..], &[0x00, 0x7C, 0xFF, 0x7F]);
        assert_eq!(&out[..8], &file[..8]);
    }

    #[test]
    fn grown_tile_files_say_their_new_size() {
        let mut file = b"RGCN\xFF\xFE\0\x01\0\0\0\0\x10\0\x01\0".to_vec();
        file.extend(b"RAHC");
        file.extend(((32 + 32) as u32).to_le_bytes());
        file.extend([1, 0, 1, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        file.extend(32u32.to_le_bytes());
        file.extend(24u32.to_le_bytes());
        file.extend([0x11u8; 32]);
        let size = file.len();
        wr32(&mut file, 8, size);
        let mut tiles = g2d::read_tiles(&file).unwrap();
        tiles.pixels.resize(128, 2);
        let out = write_tiles(&file, &tiles).unwrap();
        let read = g2d::read_tiles(&out).unwrap();
        assert_eq!(read.count(), 2);
        assert_eq!(read.height, Some(2));
        assert_eq!(read.pixels, tiles.pixels);
        assert_eq!(rd32(&out, 8), out.len());
    }

    #[test]
    fn a_hidden_obj_keeps_its_drawing() {
        // Deux OBJ l'un sur l'autre : celui du dessus (le premier) a du rouge, celui du dessous du vert
        let mut pixels = vec![1u8; 64];
        pixels.extend(vec![2u8; 64]);
        let mut tiles = g2d::Tiles { bpp8: false, linear: false, pixels, width: None, height: None, attrs: Vec::new() };
        let obj = |tile| g2d::Obj { x: 0, y: 0, width: 8, height: 8, tile, bank: 0, hflip: false, vflip: false, bpp8: false };
        let cells = g2d::Cells { cells: vec![vec![obj(0), obj(1)]], mapping: 0, sizes: Vec::new() };
        let image = g2d::render_cell(&cells, 0, &tiles, &palette());
        cell_from_image(&cells, 0, &mut tiles, &palette(), &image).unwrap();
        assert_eq!(tiles.pixels[64], 2, "image sans changement : rien n'est ecrit");
    }

    #[test]
    fn texture_colors_match_the_texture_decoder() {
        let bytes = 0x7FFFu16.to_le_bytes();
        assert_eq!(read_colors(&bytes, 0, 1)[0], [255, 255, 255, 255]);
        let one = 1u16.to_le_bytes();
        assert_eq!(read_colors(&one, 0, 1)[0][0], 8);
        for v in 0..32u8 {
            let c = (v << 3) | (v >> 2);
            assert_eq!(to_bgr555([c, 0, 0, 255]), v as u16);
        }
    }

    #[test]
    fn textures_keep_their_format() {
        let colors = vec![[0, 0, 0, 255], [255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]];
        let image = Rgba { width: 4, height: 1, data: [[0, 0, 0, 0], [255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]].concat() };
        assert_eq!(encode_texture(2, &image, &colors, true).unwrap(), [0b11100100]);
        assert_eq!(encode_texture(3, &image, &colors, true).unwrap(), [0x10, 0x32]);
        assert_eq!(encode_texture(4, &image, &colors, true).unwrap(), [0, 1, 2, 3]);
        assert_eq!(encode_texture(7, &image, &colors, true).unwrap()[..4], [0, 0, 0x1F, 0x80]);
        // A3I5 : l'alpha dans les 3 bits forts
        assert_eq!(encode_texture(1, &image, &colors, true).unwrap()[1], 1 | (7 << 5));
    }
}
