//! Archives de son `SDAT` du SDK Nitro : la table FAT donne les fichiers (musiques en flux `STRM`, banques
//! de sons `SWAR`, séquences `SSEQ`...), les blocs SYMB et INFO donnent leurs noms.

use crate::errors::Result;
use crate::sadl::{ima, Sound};

pub struct File {
    pub name: Option<String>,
    pub start: usize,
    pub end: usize,
    pub ext: &'static str,
}

fn u16_at(b: &[u8], off: usize) -> Option<usize> {
    b.get(off..off + 2).map(|s| u16::from_le_bytes([s[0], s[1]]) as usize)
}

fn u32_at(b: &[u8], off: usize) -> Option<usize> {
    b.get(off..off + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize)
}

/// Les noms des fichiers : les catégories d'INFO et de SYMB sont dans le même ordre, chaque entrée d'INFO
/// commence par le n° de son fichier. Séquences (0), archives de séquences (1), banques (2), ondes (3), flux (7).
fn names(file: &[u8], count: usize) -> Vec<Option<String>> {
    let mut out = vec![None; count];
    let (Some(symb), Some(info)) = (u32_at(file, 0x10), u32_at(file, 0x18)) else { return out };
    if symb == 0 || info == 0 {
        return out;
    }
    for category in [0, 1, 2, 3, 7] {
        let (Some(names_at), Some(infos_at)) = (u32_at(file, symb + 8 + category * 4), u32_at(file, info + 8 + category * 4)) else { continue };
        // Les archives de séquences ont deux décalages par entrée
        let name_step = if category == 1 { 8 } else { 4 };
        let Some(entries) = u32_at(file, info + infos_at) else { continue };
        for i in 0..entries.min(65536) {
            let Some(entry) = u32_at(file, info + infos_at + 4 + i * 4) else { break };
            let Some(id) = u16_at(file, info + entry) else { continue };
            let Some(name_at) = u32_at(file, symb + names_at + 4 + i * name_step) else { continue };
            if entry == 0 || name_at == 0 {
                continue;
            }
            let Some(rest) = file.get(symb + name_at..) else { continue };
            let text = String::from_utf8_lossy(&rest[..rest.iter().position(|b| *b == 0).unwrap_or(rest.len())]).into_owned();
            if let Some(slot) = out.get_mut(id) {
                *slot = Some(text);
            }
        }
    }
    out
}

/// Les fichiers de l'archive (positions dans `file`).
pub fn files(file: &[u8]) -> Result<Vec<File>> {
    if file.get(0..4) != Some(b"SDAT") {
        bail!("pas une archive SDAT");
    }
    let Some(fat) = u32_at(file, 0x20) else { bail!("SDAT sans table") };
    if file.get(fat..fat + 4) != Some(b"FAT ") {
        bail!("SDAT : table FAT introuvable");
    }
    let Some(count) = u32_at(file, fat + 8) else { bail!("SDAT : table FAT coupee") };
    let names = names(file, count.min(65536));
    let mut out = Vec::new();
    for i in 0..count.min(65536) {
        let (Some(start), Some(size)) = (u32_at(file, fat + 12 + i * 16), u32_at(file, fat + 16 + i * 16)) else { break };
        let Some(data) = file.get(start..start + size) else { continue };
        let ext = match data.get(0..4) {
            Some(b"SSEQ") => "sseq",
            Some(b"SSAR") => "ssar",
            Some(b"SBNK") => "sbnk",
            Some(b"SWAR") => "swar",
            Some(b"STRM") => "strm",
            _ => "bin",
        };
        out.push(File { name: names.get(i).cloned().flatten(), start, end: start + size, ext });
    }
    Ok(out)
}

const PCM8: u8 = 0;
const PCM16: u8 = 1;
const IMA: u8 = 2;

/// Une voie : `bytes` au codage `kind`. En IMA, 4 octets d'état avant les données (échantillon, index).
fn voice(bytes: &[u8], kind: u8) -> Vec<i16> {
    match kind {
        PCM8 => bytes.iter().map(|b| (*b as i8 as i16) << 8).collect(),
        PCM16 => bytes.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect(),
        IMA if bytes.len() >= 4 => {
            let sample = i16::from_le_bytes([bytes[0], bytes[1]]) as i32;
            ima(bytes[4..].iter().copied(), sample, (bytes[2] as i32).clamp(0, 88))
        }
        _ => Vec::new(),
    }
}

/// Une musique `STRM` : des blocs entrelacés par voie, l'état IMA est au début de chaque bloc.
pub fn read_strm(file: &[u8]) -> Result<Sound> {
    if file.get(0..4) != Some(b"STRM") {
        bail!("pas un fichier STRM");
    }
    let head = 0x18;
    let (Some(&kind), Some(&channels)) = (file.get(head), file.get(head + 2)) else { bail!("STRM trop court") };
    let channels = channels as usize;
    let (Some(rate), Some(data_at), Some(blocks), Some(block_size), Some(last_size)) = (u16_at(file, head + 4), u32_at(file, head + 0x10), u32_at(file, head + 0x14), u32_at(file, head + 0x18), u32_at(file, head + 0x20)) else { bail!("STRM trop court") };
    if channels == 0 || channels > 2 || rate == 0 || block_size == 0 {
        bail!("STRM : {} voies, {} Hz", channels, rate);
    }
    let mut voices = vec![Vec::new(); channels];
    let mut at = data_at;
    for b in 0..blocks {
        let size = if b + 1 == blocks { last_size } else { block_size };
        for out in voices.iter_mut() {
            let Some(bytes) = file.get(at..at + size) else { break };
            out.extend(voice(bytes, kind));
            at += size;
            // Les blocs sont alignés sur 4 octets
            at = at.next_multiple_of(4);
        }
    }
    let length = voices.iter().map(Vec::len).min().unwrap_or(0);
    let mut samples = Vec::with_capacity(length * channels);
    for i in 0..length {
        samples.extend(voices.iter().map(|v| v[i]));
    }
    Ok(Sound { rate: rate as u32, channels: channels as u16, samples })
}

/// Le son à la position `at` : type, boucle, fréquence (u16), durée (u16), début de boucle et longueur en
/// mots de 32 bits, puis les données.
fn read_wave(file: &[u8], at: usize) -> Result<Sound> {
    let (Some(&kind), Some(rate), Some(loop_at), Some(rest)) = (file.get(at), u16_at(file, at + 2), u16_at(file, at + 6), u32_at(file, at + 8)) else { bail!("son trop court") };
    if rate == 0 {
        bail!("son a 0 Hz");
    }
    let size = (loop_at + rest) * 4;
    let data = file.get(at + 12..).unwrap_or(&[]);
    let data = &data[..size.min(data.len())];
    Ok(Sound { rate: rate as u32, channels: 1, samples: voice(data, kind) })
}

/// Le nombre de sons d'une banque `SWAR`.
pub fn swar_count(file: &[u8]) -> Result<usize> {
    if file.get(0..4) != Some(b"SWAR") {
        bail!("pas un fichier SWAR");
    }
    match u32_at(file, 0x38) {
        Some(count) => Ok(count),
        None => bail!("SWAR trop court"),
    }
}

/// Le son `index` d'une banque `SWAR`.
pub fn read_swar(file: &[u8], index: usize) -> Result<Sound> {
    if index >= swar_count(file)? {
        bail!("SWAR : pas de son {}", index);
    }
    match u32_at(file, 0x3C + index * 4) {
        Some(at) => read_wave(file, at),
        None => bail!("SWAR trop court"),
    }
}

/// Un son seul `SWAV`.
pub fn read_swav(file: &[u8]) -> Result<Sound> {
    if file.get(0..4) != Some(b"SWAV") {
        bail!("pas un fichier SWAV");
    }
    read_wave(file, 0x18)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_files_with_the_fat() {
        let mut file = vec![0u8; 0x40];
        file[0..4].copy_from_slice(b"SDAT");
        file[0x20..0x24].copy_from_slice(&0x40u32.to_le_bytes());
        file.extend(b"FAT \x1c\0\0\0\x01\0\0\0");
        file.extend(0x60u32.to_le_bytes());
        file.extend(4u32.to_le_bytes());
        file.extend([0u8; 8]);
        file.resize(0x60, 0);
        file.extend(b"SWAR");
        let found = files(&file).unwrap();
        assert_eq!((found[0].start, found[0].end, found[0].ext), (0x60, 0x64, "swar"));
        assert!(files(b"pas ca").is_err());
    }

    #[test]
    fn reads_pcm_waves() {
        let mut file = vec![0u8; 0x18];
        file[0..4].copy_from_slice(b"SWAV");
        file.extend([PCM8, 0, 0x10, 0x27, 0, 0, 0, 0, 1, 0, 0, 0]);
        file.extend([1, 0xFF, 0, 0]);
        let sound = read_swav(&file).unwrap();
        assert_eq!(sound.rate, 10000);
        assert_eq!(sound.samples, [256, -256, 0, 0]);
    }
}
