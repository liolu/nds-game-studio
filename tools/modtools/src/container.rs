//! Remet des fichiers modifiés dans ce qui les contient : la rom, une archive NARC, une archive de son SDAT, une table
//! `rom.bin`, un fichier maison à table de positions. L'original n'est jamais modifié : chaque fonction rend une copie.

use std::collections::{BTreeMap, HashMap};

use nds_nitro::{nitrofs, romarc};

use crate::compress;

pub fn rd16(b: &[u8], at: usize) -> usize {
    b.get(at..at + 2).map_or(0, |s| u16::from_le_bytes([s[0], s[1]]) as usize)
}

pub fn rd32(b: &[u8], at: usize) -> usize {
    b.get(at..at + 4).map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize)
}

pub fn wr16(b: &mut [u8], at: usize, value: usize) {
    b[at..at + 2].copy_from_slice(&(value as u16).to_le_bytes());
}

pub fn wr32(b: &mut [u8], at: usize, value: usize) {
    b[at..at + 4].copy_from_slice(&(value as u32).to_le_bytes());
}

/// CRC de l'en-tête d'une rom.
fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0xFFFFu16;
    for byte in data {
        crc ^= *byte as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xA001 } else { crc >> 1 };
        }
    }
    crc
}

/// Une copie de la rom avec ces fichiers (chemin NitroFS -> contenu). Un fichier qui tient reste à sa place, sinon il va
/// à la fin de la rom et sa place d'origine reste telle quelle. Les jeux trouvent leurs fichiers par la table FAT.
pub fn patch_rom(rom: &[u8], files: &BTreeMap<String, Vec<u8>>) -> Result<Vec<u8>, String> {
    const ROM_ALIGN: usize = 0x200;
    let table = nitrofs::read(rom).map_err(|e| e.to_string())?;
    let fat = rd32(rom, 0x48);
    let mut out = rom.to_vec();
    for (path, bytes) in files {
        let file = table.iter().find(|f| f.path.eq_ignore_ascii_case(path)).ok_or_else(|| format!("{path} : pas dans la rom"))?;
        let start = if bytes.len() <= file.end - file.start {
            file.start
        } else {
            out.resize(out.len().next_multiple_of(ROM_ALIGN), 0xFF);
            out.resize(out.len() + bytes.len(), 0xFF);
            out.len() - bytes.len()
        };
        out[start..start + bytes.len()].copy_from_slice(bytes);
        wr32(&mut out, fat + file.id * 8, start);
        wr32(&mut out, fat + file.id * 8 + 4, start + bytes.len());
    }
    // Taille utilisée, puis CRC de l'en-tête
    if out.len() != rom.len() {
        let used = out.len();
        wr32(&mut out, 0x80, used);
        let crc = crc16(&out[..0x15E]);
        out[0x15E..0x160].copy_from_slice(&crc.to_le_bytes());
    }
    Ok(out)
}

fn align(out: &mut Vec<u8>, to: usize, fill: u8) {
    out.resize(out.len().next_multiple_of(to), fill);
}

/// Une archive NARC avec ces fichiers (n° -> contenu).
pub fn rebuild_narc(file: &[u8], new: &HashMap<usize, Vec<u8>>) -> Result<Vec<u8>, String> {
    let mut at = rd16(file, 12).max(16);
    let (mut fat, mut img) = (None, None);
    while at + 8 <= file.len() {
        let size = rd32(file, at + 4);
        if size < 8 {
            break;
        }
        match &file[at..at + 4] {
            b"BTAF" => fat = Some(at),
            b"GMIF" => img = Some((at, size)),
            _ => {}
        }
        at += size;
    }
    let (Some(fat), Some((img, img_size))) = (fat, img) else { return Err("NARC sans table".into()) };
    let count = rd16(file, fat + 8);
    let base = img + 8;
    let mut data = Vec::new();
    let mut table = Vec::with_capacity(count);
    for i in 0..count {
        let (start, end) = (rd32(file, fat + 12 + i * 8), rd32(file, fat + 16 + i * 8));
        let bytes: &[u8] = match new.get(&i) {
            Some(bytes) => bytes,
            None => file.get(base + start..base + end).ok_or("NARC : fichier hors de l'archive")?,
        };
        align(&mut data, 4, 0xFF);
        table.push((data.len(), data.len() + bytes.len()));
        data.extend(bytes);
    }
    align(&mut data, 4, 0xFF);
    let mut out = file[..img].to_vec();
    for (i, (start, end)) in table.into_iter().enumerate() {
        wr32(&mut out, fat + 12 + i * 8, start);
        wr32(&mut out, fat + 16 + i * 8, end);
    }
    out.extend(b"GMIF");
    out.extend(((data.len() + 8) as u32).to_le_bytes());
    out.extend(data);
    out.extend(file.get(img + img_size..).unwrap_or(&[]));
    let size = out.len();
    wr32(&mut out, 8, size);
    Ok(out)
}

/// Une archive de son avec ces fichiers (position d'origine -> contenu).
pub fn rebuild_sdat(file: &[u8], new: &HashMap<usize, Vec<u8>>) -> Result<Vec<u8>, String> {
    const SDAT_ALIGN: usize = 0x20;
    let (fat, block) = (rd32(file, 0x20), rd32(file, 0x28));
    if file.get(fat..fat + 4) != Some(b"FAT ") || file.get(block..block + 4) != Some(b"FILE") {
        return Err("SDAT sans table".into());
    }
    let count = rd32(file, fat + 8);
    let block_end = block + rd32(file, block + 4);
    let mut out = file[..block + 12].to_vec();
    // Le bloc de fichiers commence souvent plus loin que son en-tête : on garde cet écart
    let first = (0..count).map(|i| rd32(file, fat + 12 + i * 16)).filter(|s| *s > 0).min().unwrap_or(block + 12);
    out.extend(file.get(block + 12..first).unwrap_or(&[]));
    // Deux entrées de la table peuvent montrer le même fichier
    let mut placed: HashMap<usize, (usize, usize)> = HashMap::new();
    let mut slots: Vec<usize> = (0..count).collect();
    slots.sort_by_key(|i| rd32(file, fat + 12 + i * 16));
    for slot in slots {
        let (start, size) = (rd32(file, fat + 12 + slot * 16), rd32(file, fat + 16 + slot * 16));
        let (at, length) = *match placed.get(&start) {
            Some(place) => place,
            None => {
                let bytes: &[u8] = match new.get(&start) {
                    Some(bytes) => bytes,
                    None => file.get(start..start + size).ok_or("SDAT : fichier hors de l'archive")?,
                };
                align(&mut out, SDAT_ALIGN, 0);
                let place = (out.len(), bytes.len());
                out.extend(bytes);
                placed.insert(start, place);
                placed.get(&start).unwrap()
            }
        };
        wr32(&mut out, fat + 12 + slot * 16, at);
        wr32(&mut out, fat + 16 + slot * 16, length);
    }
    align(&mut out, SDAT_ALIGN, 0);
    let new_block_end = out.len();
    wr32(&mut out, block + 4, new_block_end - block);
    wr32(&mut out, 0x2C, new_block_end - block);
    out.extend(file.get(block_end..).unwrap_or(&[]));
    let size = out.len();
    wr32(&mut out, 8, size);
    Ok(out)
}

/// Une table `rom.bin` avec ces fichiers (n° -> contenu dépaqueté).
pub fn rebuild_rom_bin(file: &[u8], new: &HashMap<usize, Vec<u8>>) -> Result<Vec<u8>, String> {
    let entries = romarc::entries(file);
    let table_size = rd32(file, 0);
    if entries.is_empty() || (entries.len() + 1) * 4 != table_size {
        return Err("rom.bin : table inattendue".into());
    }
    let mut out = vec![0u8; table_size];
    for (i, (at, size)) in entries.iter().enumerate() {
        let at_now = out.len();
        wr32(&mut out, i * 4, at_now);
        let blob = &file[*at..*at + *size];
        match new.get(&i) {
            Some(bytes) => out.extend(pack_rom_bin(blob, bytes)?),
            None => out.extend(blob),
        }
        // Quelques tout petits fichiers ne tombent pas juste : sans quoi la table leur donnerait une autre taille
        if (*at + *size) % 4 == 0 {
            align(&mut out, 4, 0);
        }
    }
    let end = out.len();
    wr32(&mut out, entries.len() * 4, end);
    Ok(out)
}

/// Un fichier de `rom.bin`, empaqueté comme l'original : tous les jeux n'ont pas toutes les routines (Lego n'a pas celle du
/// LZ77 du SDK, le jeu plante). Le Huffman, que ces jeux utilisent peu, passe par leur codeur propre.
fn pack_rom_bin(original: &[u8], new: &[u8]) -> Result<Vec<u8>, String> {
    let inner = romarc::unpack(original);
    if inner == original {
        return compress::like(original, new);
    }
    let unpacked = nds_nitro::unpack(inner.clone());
    let plain = if unpacked != inner { compress::like(&inner, new)? } else { new.to_vec() };
    let kind = (original[0] >> 4) & 0xF;
    let filtered = kind & 8 != 0;
    let mut data = plain.clone();
    if filtered {
        let mut previous = 0u16;
        for pair in data.chunks_exact_mut(2) {
            let value = u16::from_le_bytes([pair[0], pair[1]]);
            pair.copy_from_slice(&value.wrapping_sub(previous).to_le_bytes());
            previous = value;
        }
    }
    let filter_bit = if filtered { 0x80 } else { 0 };
    let packed = match kind & 7 {
        1 => Some(compress::lz10(&data)?),
        3 => Some(compress::rle(&data)?),
        2 | 6 => Some(compress::game_coder(&data)?),
        _ => None,
    };
    // L'en-tête refuse un fichier empaqueté plus long que l'original : dans ce cas, une simple copie
    if let Some(mut out) = packed.filter(|p| p.len() <= data.len() + 8 && data.len() >= 4) {
        out[0] |= filter_bit;
        return Ok(out);
    }
    let mut out = vec![filter_bit, data.len() as u8, (data.len() >> 8) as u8, (data.len() >> 16) as u8];
    out.extend(&data);
    align(&mut out, 4, 0);
    Ok(out)
}

/// Un fichier maison commençant par une table de positions. `None` si `starts` n'en sont pas toutes.
pub fn rebuild_table(file: &[u8], new: &HashMap<usize, Vec<u8>>) -> Option<Vec<u8>> {
    let table = rd32(file, 0);
    if table < 8 || table % 4 != 0 || table > file.len() {
        return None;
    }
    let offsets: Vec<usize> = (0..table / 4).map(|i| rd32(file, i * 4)).collect();
    if !new.keys().all(|start| offsets.contains(start)) {
        return None;
    }
    let mut out = file[..table].to_vec();
    for (i, start) in offsets.iter().enumerate() {
        let end = offsets.get(i + 1).copied().unwrap_or(file.len());
        let at_now = out.len();
        wr32(&mut out, i * 4, at_now);
        match new.get(start) {
            Some(bytes) => out.extend(bytes),
            None => out.extend(file.get(*start..end)?),
        }
        if i + 1 != offsets.len() {
            align(&mut out, 4, 0);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nds_nitro::{narc, sdat};

    fn narc_file() -> Vec<u8> {
        let mut file = b"NARC\xFE\xFF\0\x01\0\0\0\0\x10\0\x03\0".to_vec();
        file.extend(b"BTAF\x1C\0\0\0\x02\0\0\0");
        file.extend([0, 0, 0, 0, 4, 0, 0, 0, 4, 0, 0, 0, 8, 0, 0, 0]);
        file.extend(b"BTNF\x10\0\0\0\x04\0\0\0\0\0\x01\0");
        file.extend(b"GMIF\x10\0\0\0AAAABBBB");
        let size = file.len();
        wr32(&mut file, 8, size);
        file
    }

    #[test]
    fn narc_files_can_grow() {
        let new = HashMap::from([(0, b"CCCCCCCCC".to_vec())]);
        let out = rebuild_narc(&narc_file(), &new).unwrap();
        let entries = narc::read(&out).unwrap();
        assert_eq!(&out[entries[0].start..entries[0].end], b"CCCCCCCCC");
        assert_eq!(&out[entries[1].start..entries[1].end], b"BBBB");
        assert_eq!(rd32(&out, 8), out.len());
    }

    #[test]
    fn sdat_files_can_grow() {
        let mut file = vec![0u8; 0x40];
        file[0..4].copy_from_slice(b"SDAT");
        wr32(&mut file, 0x20, 0x40);
        file.extend(b"FAT \x2c\0\0\0\x02\0\0\0");
        for (start, size) in [(0x80usize, 4usize), (0x84, 4)] {
            file.extend((start as u32).to_le_bytes());
            file.extend((size as u32).to_le_bytes());
            file.extend([0u8; 8]);
        }
        let size = file.len();
        wr32(&mut file, 0x28, size);
        let block = file.len();
        file.extend(b"FILE\0\0\0\0\x02\0\0\0");
        file.resize(0x80, 0);
        file.extend(b"SWARSTRM");
        let size = file.len();
        wr32(&mut file, block + 4, size - block);
        let new = HashMap::from([(0x80, b"SWARSWARSWAR".to_vec())]);
        let out = rebuild_sdat(&file, &new).unwrap();
        let found = sdat::files(&out).unwrap();
        assert_eq!(&out[found[0].start..found[0].end], b"SWARSWARSWAR");
        assert_eq!(&out[found[1].start..found[1].end], b"STRM");
    }

    #[test]
    fn rom_bin_keeps_the_other_files_and_its_filter() {
        // Sorte 0 + filtre delta (0x80) : valeurs 1, 2 -> deltas 1, 1
        let packed = [0x80u8, 4, 0, 0, 1, 0, 1, 0];
        let mut file = vec![0u8; 12];
        wr32(&mut file, 0, 12);
        wr32(&mut file, 4, 20);
        wr32(&mut file, 8, 24);
        file.extend(packed);
        file.extend(b"KEEP");
        let new = HashMap::from([(0, vec![5u8, 0, 9, 0, 20, 0])]);
        let out = rebuild_rom_bin(&file, &new).unwrap();
        let entries = romarc::entries(&out);
        assert_eq!(romarc::unpack(&out[entries[0].0..entries[0].0 + entries[0].1]), [5, 0, 9, 0, 20, 0]);
        assert_eq!(&out[entries[1].0..entries[1].0 + entries[1].1], b"KEEP");
    }

    #[test]
    fn the_rom_itself_stays_intact_and_bigger_files_move_to_the_end() {
        let rom = nitrofs::tests_rom(b"AAAA", b"BBBB");
        let before = rom.clone();
        let small = BTreeMap::from([("data/a.bin".to_string(), b"CC".to_vec())]);
        let out = patch_rom(&rom, &small).unwrap();
        assert_eq!(out.len(), rom.len(), "ca tient : pas de deplacement");
        let big = BTreeMap::from([("data/a.bin".to_string(), vec![7u8; 1000])]);
        let out = patch_rom(&rom, &big).unwrap();
        let files = nitrofs::read(&out).unwrap();
        let moved = files.iter().find(|f| f.path == "data/a.bin").unwrap();
        assert_eq!(&out[moved.start..moved.end], &[7u8; 1000][..]);
        assert_eq!(moved.start % 0x200, 0);
        let kept = files.iter().find(|f| f.path == "rom.bin").unwrap();
        assert_eq!(&out[kept.start..kept.end], b"BBBB");
        assert_eq!(rd32(&out, 0x80), out.len());
        assert_eq!(rom, before);
    }

    #[test]
    fn offset_tables_are_rewritten() {
        let mut file = vec![8u8, 0, 0, 0, 12, 0, 0, 0];
        file.extend(b"AAAABBBB");
        let new = HashMap::from([(8, b"CCCCCCCC".to_vec())]);
        let out = rebuild_table(&file, &new).unwrap();
        assert_eq!(rd32(&out, 4), 16);
        assert_eq!(&out[8..], b"CCCCCCCCBBBB");
        assert!(rebuild_table(&file, &HashMap::from([(9, vec![])])).is_none());
    }
}
