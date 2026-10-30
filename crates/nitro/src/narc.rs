//! Archives NARC (un petit système de fichiers Nitro : blocs BTAF, BTNF, GMIF).

use crate::errors::Result;

pub struct Entry {
    /// Chemin dans l'archive (« dossier/nom »), ou le n° du fichier s'il n'a pas de nom.
    pub name: String,
    pub index: usize,
    pub start: usize,
    pub end: usize,
}

fn u16_at(b: &[u8], off: usize) -> Option<u16> {
    b.get(off..off + 2).map(|s| u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    b.get(off..off + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Les fichiers de l'archive (positions dans `file`).
pub fn read(file: &[u8]) -> Result<Vec<Entry>> {
    if file.get(0..4) != Some(b"NARC") {
        bail!("pas une archive NARC");
    }
    let header = u16_at(file, 12).unwrap_or(16) as usize;
    let mut off = header.max(16);
    let (mut fat, mut fnt, mut img) = (None, None, None);
    while let (Some(magic), Some(size)) = (file.get(off..off + 4), u32_at(file, off + 4)) {
        let size = size as usize;
        if size < 8 {
            break;
        }
        let data = off + 8..(off + size).min(file.len());
        match magic {
            b"BTAF" => fat = Some(data),
            b"BTNF" => fnt = Some(data),
            b"GMIF" => img = Some(data),
            _ => {}
        }
        off += size;
    }
    let (Some(fat), Some(img)) = (fat, img) else { bail!("archive NARC sans table ou sans donnees") };
    let fat = &file[fat];
    let count = u16_at(fat, 0).unwrap_or(0) as usize;
    let names = fnt.map(|r| names(&file[r], count)).unwrap_or_default();
    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        let (Some(s), Some(e)) = (u32_at(fat, 4 + i * 8), u32_at(fat, 8 + i * 8)) else { break };
        let (start, end) = (img.start + s as usize, (img.start + e as usize).min(img.end));
        if start > end {
            continue;
        }
        let name = names.get(i).cloned().flatten().unwrap_or_else(|| format!("{i:04}"));
        entries.push(Entry { name, index: i, start, end });
    }
    Ok(entries)
}

/// Noms des fichiers d'après la table des noms (FNT), s'il y en a.
fn names(fnt: &[u8], count: usize) -> Vec<Option<String>> {
    let mut out = vec![None; count];
    fn walk(fnt: &[u8], dir: usize, prefix: &str, out: &mut Vec<Option<String>>, depth: usize) {
        if depth > 32 {
            return;
        }
        let (Some(sub), Some(first)) = (u32_at(fnt, dir * 8), u16_at(fnt, dir * 8 + 4)) else { return };
        let (mut p, mut id) = (sub as usize, first as usize);
        while let Some(&b) = fnt.get(p) {
            if b == 0 {
                break;
            }
            let len = (b & 0x7F) as usize;
            let Some(name) = fnt.get(p + 1..p + 1 + len) else { return };
            let name = format!("{prefix}{}", String::from_utf8_lossy(name));
            p += 1 + len;
            if b & 0x80 != 0 {
                let Some(sub_dir) = u16_at(fnt, p) else { return };
                p += 2;
                walk(fnt, (sub_dir & 0xFFF) as usize, &format!("{name}/"), out, depth + 1);
            } else {
                if let Some(slot) = out.get_mut(id) {
                    *slot = Some(name);
                }
                id += 1;
            }
        }
    }
    walk(fnt, 0, "", &mut out, 0);
    out
}
