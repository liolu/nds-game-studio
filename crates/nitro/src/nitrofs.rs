//! NitroFS : les fichiers d'une rom DS, lus directement dans la rom (table des noms FNT et table des
//! positions FAT, dont les adresses sont dans l'en-tête).

use crate::errors::Result;

pub struct File {
    /// Chemin dans NitroFS, sans « / » au début.
    pub path: String,
    /// Position dans la rom.
    pub start: usize,
    pub end: usize,
}

fn u16_at(b: &[u8], off: usize) -> Option<usize> {
    b.get(off..off + 2).map(|s| u16::from_le_bytes([s[0], s[1]]) as usize)
}

fn u32_at(b: &[u8], off: usize) -> Option<usize> {
    b.get(off..off + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize)
}

/// Les fichiers de la rom, triés par chemin (sans tenir compte de la casse).
pub fn read(rom: &[u8]) -> Result<Vec<File>> {
    let (Some(fnt_at), Some(fnt_size), Some(fat_at), Some(fat_size)) = (u32_at(rom, 0x40), u32_at(rom, 0x44), u32_at(rom, 0x48), u32_at(rom, 0x4C)) else { bail!("en-tete de rom trop court") };
    let (Some(fnt), Some(fat)) = (rom.get(fnt_at..fnt_at + fnt_size), rom.get(fat_at..fat_at + fat_size)) else { bail!("tables NitroFS hors de la rom") };
    let mut files = Vec::new();
    walk(rom, fnt, fat, 0, "", &mut files, 0);
    files.sort_by_key(|f| f.path.to_lowercase());
    Ok(files)
}

fn walk(rom: &[u8], fnt: &[u8], fat: &[u8], dir: usize, prefix: &str, out: &mut Vec<File>, depth: usize) {
    if depth > 32 {
        return;
    }
    let (Some(sub), Some(first)) = (u32_at(fnt, dir * 8), u16_at(fnt, dir * 8 + 4)) else { return };
    let (mut p, mut id) = (sub, first);
    while let Some(&b) = fnt.get(p) {
        if b == 0 {
            break;
        }
        let len = (b & 0x7F) as usize;
        let Some(name) = fnt.get(p + 1..p + 1 + len) else { return };
        let path = format!("{prefix}{}", String::from_utf8_lossy(name));
        p += 1 + len;
        if b & 0x80 != 0 {
            let Some(sub_dir) = u16_at(fnt, p) else { return };
            p += 2;
            walk(rom, fnt, fat, sub_dir & 0xFFF, &format!("{path}/"), out, depth + 1);
        } else {
            if let (Some(start), Some(end)) = (u32_at(fat, id * 8), u32_at(fat, id * 8 + 4)) {
                if start <= end && end <= rom.len() {
                    out.push(File { path, start, end });
                }
            }
            id += 1;
        }
    }
}

/// Pour les tests : une petite rom avec `data/a.bin` et `rom.bin`, sans bannière.
#[doc(hidden)]
pub fn tests_rom(a: &[u8], rom_bin: &[u8]) -> Vec<u8> {
    fake_rom(a, rom_bin, b"rom.bin")
}

fn fake_rom(a: &[u8], b: &[u8], b_name: &[u8]) -> Vec<u8> {
    let mut fnt = Vec::new();
    // Racine : sous-table à 16, premier fichier 0 ; dossier data : sous-table plus loin, fichier 1
    let root_table: Vec<u8> = [&[0x80 | 4][..], b"data", &[0x01, 0xF0], &[b_name.len() as u8], b_name, &[0]].concat();
    let data_table: Vec<u8> = [&[5][..], b"a.bin", &[0]].concat();
    for (sub, first, parent) in [(16u32, 0u16, 2u16), (16 + root_table.len() as u32, 1, 0xF000)] {
        fnt.extend(sub.to_le_bytes());
        fnt.extend(first.to_le_bytes());
        fnt.extend(parent.to_le_bytes());
    }
    fnt.extend(&root_table);
    fnt.extend(&data_table);
    let mut rom = vec![0u8; 0x200];
    let fnt_at = rom.len();
    rom.extend(&fnt);
    let fat_at = rom.len();
    rom.extend([0u8; 16]);
    let (b_at, a_at) = (rom.len(), rom.len() + b.len());
    rom.extend(b);
    rom.extend(a);
    let put = |rom: &mut Vec<u8>, at: usize, value: usize| rom[at..at + 4].copy_from_slice(&(value as u32).to_le_bytes());
    put(&mut rom, 0x40, fnt_at);
    put(&mut rom, 0x44, fnt.len());
    put(&mut rom, 0x48, fat_at);
    put(&mut rom, 0x4C, 16);
    // Fichier 0 = b (racine), fichier 1 = data/a.bin
    for (i, value) in [b_at, b_at + b.len(), a_at, a_at + a.len()].into_iter().enumerate() {
        put(&mut rom, fat_at + i * 4, value);
    }
    rom
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_files_with_folders() {
        let rom = fake_rom(b"AAAA", b"BB", b"b.bin");
        let files = read(&rom).unwrap();
        let found: Vec<(&str, &[u8])> = files.iter().map(|f| (f.path.as_str(), &rom[f.start..f.end])).collect();
        assert_eq!(found, [("b.bin", &b"BB"[..]), ("data/a.bin", &b"AAAA"[..])]);
    }

    #[test]
    fn broken_tables_are_an_error_or_nothing() {
        assert!(read(&[0u8; 16]).is_err());
        let mut rom = fake_rom(b"A", b"B", b"b.bin");
        rom[0x44..0x48].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(read(&rom).is_err());
    }
}
