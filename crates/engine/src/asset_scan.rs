//! Fichiers des roms pour la fenêtre Assets, lus dans la rom elle-même : liste, décodage d'un aperçu,
//! et index de toutes les roms (pourcentages, formats, essai d'un fichier de chaque format).
//!
//! L'index est calculé en arrière-plan et gardé dans `assets_index.json` (dossier des données).
//! Il est refait pour une rom quand son fichier change.
//!
//! - **extrait** = part des octets de données lisibles fichier par fichier. Ne comptent pas comme
//!   extraits : un `rom.bin` qui n'est pas une table de fichiers, un `.narc` illisible, et
//!   tout gros fichier (4 Mo et plus) de signature inconnue, comme `Glob.bin`. Les banques de son
//!   et les vidéos ne comptent pas dans le total.
//! - **regardable** = part des fichiers (ceux des archives compris) que la fenêtre sait montrer.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek, SeekFrom};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use nds_nitro::{adx, decompress_any, g2d, gx_mesh, looks_compressed, model3d, narc, nitrofs, romarc, sadl, sdat, sniff, unpack, Kind, Rgba};
use serde::{Deserialize, Serialize};

use crate::i18n::{t, tr};

/// À changer quand le calcul change : l'index gardé est alors refait.
const INDEX_VERSION: u32 = 9;
const BIG_FILE: usize = 4 << 20;

/// Une rom du dossier `rom` du launcher.
pub struct GameDir {
    pub label: String,
    pub rom: PathBuf,
}

pub struct Entry {
    /// Chemin dans NitroFS (sans « / » au début) ; `archive.narc#n` dans une archive.
    pub path: String,
    /// Nom dans l'archive, s'il y en a un.
    pub member_name: Option<String>,
    /// La rom, et la position du fichier dedans.
    pub rom: PathBuf,
    pub span: (usize, usize),
    pub member: Option<(usize, usize)>,
    pub kind: Kind,
    pub size: usize,
    /// Dossier (ou archive) : les fichiers qui vont ensemble y sont cherchés d'abord.
    pub dir: String,
    pub stem: String,
    /// N° dans l'archive rom.bin.
    pub rom_index: Option<u32>,
}

impl Entry {
    /// Extension en minuscules, sans le point (« (sans nom) » pour un fichier d'archive anonyme).
    pub fn format(&self) -> String {
        if self.member.is_some() && self.member_name.is_none() {
            return "(sans nom)".into();
        }
        let name = self.member_name.as_deref().unwrap_or(&self.path);
        let name = name.rsplit('/').next().unwrap_or(name);
        match name.rsplit_once('.') {
            Some((_, ext)) if !ext.is_empty() => ext.to_lowercase(),
            _ => "(sans extension)".into(),
        }
    }
}

/// Les roms du dossier : « Titre (CODE) ».
pub fn find_games(dir: &Path) -> Vec<GameDir> {
    crate::library::scan_roms(dir).into_iter().map(|rom| GameDir { label: format!("{} ({})", plain_title(&rom.title, &rom.internal_name), rom.code), rom: rom.path }).collect()
}

/// Le titre avec les seuls caractères de la police de cette fenêtre : sans accents ni « ™ » ; le nom
/// interne de la rom si le titre est dans une autre écriture (japonais...).
fn plain_title(title: &str, internal_name: &str) -> String {
    let mut plain = String::new();
    for c in title.chars() {
        match c {
            'à' | 'â' | 'ä' | 'á' | 'ã' => plain.push('a'),
            'é' | 'è' | 'ê' | 'ë' => plain.push('e'),
            'î' | 'ï' | 'í' | 'ì' => plain.push('i'),
            'ô' | 'ö' | 'ó' | 'ò' | 'õ' => plain.push('o'),
            'û' | 'ü' | 'ú' | 'ù' => plain.push('u'),
            'ç' => plain.push('c'),
            'ñ' => plain.push('n'),
            'À' | 'Â' | 'Ä' | 'Á' => plain.push('A'),
            'É' | 'È' | 'Ê' | 'Ë' => plain.push('E'),
            '™' | '®' | '©' => {}
            c if c.is_ascii() => plain.push(c),
            _ => return internal_name.to_string(),
        }
    }
    plain
}

fn stem_of(path: &str) -> String {
    let name = path.rsplit(['/', '#']).next().unwrap_or(path);
    name.split('.').next().unwrap_or(name).to_lowercase()
}

/// Sorte d'un fichier : signature, sinon signature une fois décompressé (LZ77).
fn kind_of(head: &[u8], whole: impl FnOnce() -> Option<Vec<u8>>) -> Kind {
    let kind = sniff(head);
    if kind == Kind::Other && looks_compressed(head.first()) {
        if let Some(bytes) = whole() {
            return sniff(&unpack(bytes));
        }
    }
    kind
}

/// Tous les fichiers de la rom, ceux des archives NARC et ceux de rom.bin.
pub fn scan(rom_path: &Path) -> Vec<Entry> {
    let Ok(rom) = std::fs::read(rom_path) else { return Vec::new() };
    let Ok(files) = nitrofs::read(&rom) else { return Vec::new() };
    let mut entries = Vec::new();
    let mut rom_bin = Vec::new();
    for file in files {
        let span = (file.start, file.end);
        let data = &rom[file.start..file.end];
        let path = file.path;
        let size = data.len();
        let small = size < BIG_FILE;
        let kind = guess_house_format(by_extension(kind_of(&data[..size.min(4)], || small.then(|| data.to_vec())), &path), &path, data);
        let dir = path.rsplit_once('/').map_or(String::new(), |(d, _)| d.to_string());
        let stem = stem_of(&path);
        entries.push(Entry { path: path.clone(), member_name: None, rom: rom_path.to_path_buf(), span, member: None, kind, size, dir, stem, rom_index: None });
        if path.rsplit('/').next().is_some_and(|n| n.eq_ignore_ascii_case("rom.bin")) {
            rom_bin = scan_rom_bin(rom_path, data, file.start);
        }
        if kind == Kind::NestedRom {
            // Une mini-rom a son propre NitroFS ; ses positions comptent depuis son début
            for (n, f) in nitrofs::read(data).unwrap_or_default().into_iter().enumerate() {
                let inner = &data[f.start..f.end];
                let name = f.path.rsplit('/').next().unwrap_or(&f.path).to_string();
                entries.push(Entry { path: format!("{path}#{n}"), member_name: Some(f.path.clone()), rom: rom_path.to_path_buf(), span, member: Some((f.start, f.end)), kind: by_extension(sniff(inner), &name), size: f.end - f.start, dir: path.clone(), stem: stem_of(&name), rom_index: None });
            }
            continue;
        }
        if kind == Kind::SoundArchive {
            for (n, f) in sdat::files(data).unwrap_or_default().into_iter().enumerate() {
                let inner = &data[f.start..f.end];
                let name = format!("{}.{}", f.name.unwrap_or_else(|| format!("{n:04}")), f.ext);
                entries.push(Entry { path: format!("{path}#{n}"), member_name: Some(name), rom: rom_path.to_path_buf(), span, member: Some((f.start, f.end)), kind: sniff(inner), size: f.end - f.start, dir: path.clone(), stem: String::new(), rom_index: None });
            }
            continue;
        }
        if kind == Kind::Other && !path.to_lowercase().ends_with("rom.bin") {
            let bytes = unpack(data.to_vec());
            let embedded = embedded_files(&bytes);
            for (n, (start, end, ext)) in embedded.iter().copied().enumerate() {
                let inner = &bytes[start..end];
                entries.push(Entry { path: format!("{path}#{n}"), member_name: Some(format!("{n:03}.{ext}")), rom: rom_path.to_path_buf(), span, member: Some((start, end)), kind: sniff(inner), size: end - start, dir: path.clone(), stem: String::new(), rom_index: None });
            }
            if embedded.is_empty() && has_house_extension(&path) {
                // Une table de positions au début : chaque morceau est un fichier, souvent compressé
                for (n, (start, end)) in table_members(&bytes).into_iter().enumerate() {
                    let member = &bytes[start..end];
                    let kind = guess_house_format(sniff_inner(member), &path, member);
                    let ext = if kind == Kind::Other { "bin" } else { "dat" };
                    entries.push(Entry { path: format!("{path}#{n}"), member_name: Some(format!("{n:03}.{ext}")), rom: rom_path.to_path_buf(), span, member: Some((start, end)), kind, size: end - start, dir: path.clone(), stem: String::new(), rom_index: None });
                }
            }
            continue;
        }
        if kind != Kind::Archive {
            continue;
        }
        let bytes = unpack(data.to_vec());
        let Ok(members) = narc::read(&bytes) else { continue };
        for m in members {
            let data = &bytes[m.start..m.end];
            let kind = by_extension(kind_of(data, || Some(data.to_vec())), &m.name);
            let member_path = format!("{path}#{}", m.index);
            let named = !m.name.chars().all(|c| c.is_ascii_digit());
            let stem = if named { stem_of(&m.name) } else { String::new() };
            entries.push(Entry { path: member_path, member_name: named.then(|| m.name.clone()), rom: rom_path.to_path_buf(), span, member: Some((m.start, m.end)), kind, size: m.end - m.start, dir: path.clone(), stem, rom_index: None });
        }
    }
    entries.extend(rom_bin);
    entries
}

fn has_house_extension(path: &str) -> bool {
    matches!(path.rsplit_once('.').map(|(_, ext)| ext.to_lowercase()).as_deref(), Some("dat" | "bin"))
}

/// La sorte d'un morceau, une fois décompressé s'il l'est.
fn sniff_inner(member: &[u8]) -> Kind {
    match decompress_any(member) {
        Some(inner) => sniff(&inner),
        None => sniff(member),
    }
}

/// Un `.dat` ou `.bin` sans signature : une planche de tuiles se reconnaît à sa taille et à sa palette.
fn guess_house_format(kind: Kind, path: &str, data: &[u8]) -> Kind {
    if kind != Kind::Other || !has_house_extension(path) {
        return kind;
    }
    let inner = decompress_any(data);
    if g2d::read_palette_tiles(inner.as_deref().unwrap_or(data)).is_some() { Kind::PaletteTiles } else { kind }
}

/// Les morceaux d'un fichier qui commence par une table de positions : la première donne la taille de la table, elles
/// vont en croissant, la fin du dernier morceau est celle du fichier. Vide si ce n'est pas une telle table.
fn table_members(bytes: &[u8]) -> Vec<(usize, usize)> {
    const MAX_TABLE: usize = 256 * 1024;
    let u32_at = |at: usize| bytes.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize);
    let Some(first) = u32_at(0) else { return Vec::new() };
    if first % 4 != 0 || first < 8 || first > bytes.len() || first > MAX_TABLE {
        return Vec::new();
    }
    let offsets: Vec<usize> = (0..first / 4).filter_map(|i| u32_at(i * 4)).collect();
    if offsets.windows(2).any(|w| w[1] < w[0]) || offsets.last().is_none_or(|last| *last > bytes.len()) {
        return Vec::new();
    }
    offsets.iter().enumerate().map(|(i, start)| (*start, offsets.get(i + 1).copied().unwrap_or(bytes.len()))).collect()
}

/// Les fichiers bruts n'ont pas de signature : on se fie à leur extension.
fn by_extension(kind: Kind, name: &str) -> Kind {
    if kind != Kind::Other {
        return kind;
    }
    match name.rsplit_once('.').map(|(_, ext)| ext.to_lowercase()).as_deref() {
        Some("nbfc" | "char" | "ncg") => Kind::RawTiles,
        Some("nbfp" | "ntfp" | "plt" | "ncl" | "pal" | "plb") => Kind::RawPalette,
        Some("ntft") => Kind::RawTexture,
        Some("bstga") => Kind::BsTexture,
        Some("bmp") => Kind::Bitmap,
        Some("zip") => Kind::ZipFile,
        Some("z") => Kind::ZImage,
        Some("anm") => Kind::AnmFile,
        Some("nbfs" | "nsc" | "scr") => Kind::RawScreen,
        Some("adx") => Kind::Audio,
        Some("txt" | "ini" | "itx" | "csv" | "xml" | "cfg") => Kind::PlainText,
        Some("lz" | "cat") => Kind::Compressed,
        Some("srl") => Kind::NestedRom,
        _ => kind,
    }
}

/// Le chemin de la texture et celui de sa palette, dans un `TXET` (deux chaînes précédées de leur taille).
fn texture_ref(bytes: &[u8]) -> Option<(String, String)> {
    two_paths(bytes, 4)
}

/// Deux chaînes précédées de leur taille, à partir de `at`.
fn two_paths(bytes: &[u8], at: usize) -> Option<(String, String)> {
    let read = |at: usize| -> Option<(String, usize)> {
        let size = u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?) as usize;
        let text = bytes.get(at + 4..at + 4 + size)?;
        Some((String::from_utf8_lossy(text).trim_end_matches('\0').to_string(), at + 4 + size))
    };
    let (first, next) = read(at)?;
    let (second, _) = read(next)?;
    Some((first, second))
}

/// Les morceaux d'un `MESH` : géométrie, texture, et palette chez ceux qui la nomment ici. Le nombre de morceaux et ce qui
/// suit chaque morceau changent selon le jeu : on repère donc chaque chemin de géométrie (précédé de sa taille).
fn mesh_parts(bytes: &[u8]) -> Vec<(String, String, Option<String>)> {
    const GEOMETRY: &[u8] = b"3d/geometry/";
    let string_at = |at: usize| -> Option<String> {
        let size = u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?) as usize;
        Some(String::from_utf8_lossy(bytes.get(at + 4..at + 4 + size)?).trim_end_matches('\0').to_string())
    };
    let mut parts = Vec::new();
    let mut search = 4;
    while let Some(found) = bytes.get(search..).and_then(|rest| rest.windows(GEOMETRY.len()).position(|w| w == GEOMETRY)) {
        let path_at = search + found;
        search = path_at + GEOMETRY.len();
        let Some(geometry) = path_at.checked_sub(4).and_then(string_at) else { continue };
        if !geometry.starts_with("3d/geometry/") {
            continue;
        }
        // La taille écrite compte aussi les zéros de fin, que `geometry` n'a plus
        let Some(texture_at) = bytes.get(path_at - 4..path_at).map(|s| path_at + u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize) else { continue };
        let Some(texture) = string_at(texture_at) else { continue };
        let palette = string_at(texture_at + 4 + texture.len()).filter(|p| p.starts_with("3d/palette/"));
        parts.push((geometry, texture, palette));
    }
    parts
}

/// La texture d'un `MESH` : le fichier nommé est un `TXET` qui relie pixels et palette, ou les pixels eux-mêmes.
fn engine_texture(entries: &[Entry], index: usize, named_palette: Option<&str>) -> Option<Rgba> {
    let bytes = read_entry(&entries[index]).ok()?;
    let (texel_bytes, mut palettes) = match entries[index].kind {
        Kind::TextureRef => {
            let (texel, _) = texture_ref(&bytes)?;
            (read_entry(&entries[entry_at(entries, &texel)?]).ok()?, texture_palettes(entries, Some(index), index))
        }
        _ => (bytes, texture_palettes(entries, None, index)),
    };
    if let Some(named) = named_palette.and_then(|p| entry_at(entries, p)) {
        palettes.insert(0, named);
    }
    let texel = g2d::read_texel(&texel_bytes).ok()?;
    let palette = g2d::read_palette(&read_entry(&entries[*palettes.first()?]).ok()?).ok()?;
    let mut image = g2d::render_texel(&texel, &palette).ok()?;
    // Une texture entièrement transparente est une couleur unie : sa couleur 0 compte vraiment
    if image.data.chunks_exact(4).all(|p| p[3] == 0) {
        let first = palette.colors.first().copied().unwrap_or([255; 4]);
        for pixel in image.data.chunks_exact_mut(4) {
            pixel.copy_from_slice(&first);
        }
    }
    Some(image)
}

/// Ce qu'on sait d'une animation `.anm` : la table des animations de SimAnimals (nombre, 0, positions), sinon l'en-tête.
fn anm_lines(bytes: &[u8]) -> Vec<String> {
    let u32_at = |at: usize| bytes.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize);
    // La première position dit où finit la table
    let first = u32_at(8).unwrap_or(0);
    let count = if first > 8 && (first - 8) % 4 == 0 { (first - 8) / 4 } else { 0 };
    let offsets: Vec<usize> = (0..count.min(4096)).filter_map(|i| u32_at(8 + i * 4)).collect();
    let is_table = u32_at(4) == Some(0) && count > 0 && offsets.len() == count && offsets.windows(2).all(|w| w[0] < w[1]) && offsets.last().is_some_and(|last| *last <= bytes.len());
    let mut lines = if is_table {
        let mut list = vec![t!("{count} animations", "{count} animations")];
        for (i, at) in offsets.iter().enumerate() {
            let end = offsets.get(i + 1).copied().unwrap_or(bytes.len());
            let first_frame = u32_at(*at).unwrap_or(end);
            list.push(t!("{i:4}  {} frames", "{i:4}  {} images", first_frame.saturating_sub(*at).min(end - at) / 4));
        }
        list
    } else {
        vec![t!("Header: size {}, then {} and {}", "En-tete : taille {}, puis {} et {}", u32_at(0).unwrap_or(0), u32_at(4).unwrap_or(0), u32_at(8).unwrap_or(0))]
    };
    lines.push(String::new());
    lines.extend(text_lines(bytes));
    lines
}

/// Les caractéristiques d'une vidéo, d'après son en-tête.
fn video_lines(bytes: &[u8]) -> Vec<String> {
    let u32_at = |at: usize| bytes.get(at..at + 4).map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]));
    let (format, frames, width, height, fps) = match bytes.get(0..3) {
        Some(b"BIK") => ("Bink", u32_at(8), u32_at(0x14), u32_at(0x18), u32_at(0x1C) as f32 / u32_at(0x20).max(1) as f32),
        Some(b"VXD") => ("VX", u32_at(4), u32_at(8), u32_at(0xC), u32_at(0x10) as f32 / 65536.0),
        _ => ("Mobiclip", u32_at(8), u32_at(0xC), u32_at(0x10), u32_at(0x14) as f32 / 16777216.0),
    };
    vec![
        t!("Video {format}: {width}x{height}, {frames} frames, {fps:.1} per second", "Video {format} : {width}x{height}, {frames} images, {fps:.1} par seconde"),
        t!("{:.1} seconds. The video itself is not played.", "{:.1} secondes. La video elle-meme n'est pas lue.", frames as f32 / fps.max(0.01)),
    ]
}

/// Les fichiers d'une archive zip, d'après son répertoire de fin de fichier.
fn zip_lines(bytes: &[u8]) -> Vec<String> {
    let u16_at = |at: usize| bytes.get(at..at + 2).map(|s| u16::from_le_bytes([s[0], s[1]]) as usize);
    let u32_at = |at: usize| bytes.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize);
    let Some(end) = bytes.windows(4).rposition(|w| w == b"PK\x05\x06") else { return vec![tr("Not a readable zip.", "Zip illisible.").to_string()] };
    let count = u16_at(end + 10).unwrap_or(0);
    let mut at = u32_at(end + 16).unwrap_or(0);
    let mut lines = vec![t!("{count} files", "{count} fichiers")];
    for _ in 0..count {
        let (Some(size), Some(names), Some(extra), Some(comment)) = (u32_at(at + 24), u16_at(at + 28), u16_at(at + 30), u16_at(at + 32)) else { break };
        let Some(name) = bytes.get(at + 46..at + 46 + names) else { break };
        lines.push(format!("{}  {}", String::from_utf8_lossy(name), size_text(size)));
        at += 46 + names + extra + comment;
    }
    lines
}

/// Le n° du fichier à ce chemin (le moteur écrit parfois le dossier avec des `\\`).
fn entry_at(entries: &[Entry], path: &str) -> Option<usize> {
    let wanted = path.replace('\\', "/").to_lowercase();
    entries.iter().position(|e| e.path.to_lowercase() == wanted)
}

/// Les palettes d'une texture : celle que nomme son `TXET` d'abord, puis les plus proches par le nom.
fn texture_palettes(entries: &[Entry], reference: Option<usize>, near: usize) -> Vec<usize> {
    let named = reference.and_then(|r| read_entry(&entries[r]).ok()).and_then(|b| texture_ref(&b)).and_then(|(_, palette)| entry_at(entries, &palette));
    let mut list: Vec<usize> = named.into_iter().collect();
    list.extend(companions(entries, near, &[Kind::Palette]).into_iter().filter(|i| Some(*i) != named));
    list
}

/// Les textes d'un fichier `GNAL` : nombre de textes, puis pour chacun deux nombres, sa taille, ses octets.
fn lang_lines(bytes: &[u8]) -> Vec<String> {
    let u32_at = |at: usize| bytes.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize);
    let count = u32_at(4).unwrap_or(0);
    let mut lines = Vec::new();
    let mut at = 8;
    for i in 0..count {
        let Some(size) = u32_at(at + 8) else { break };
        let Some(text) = bytes.get(at + 12..at + 12 + size) else { break };
        lines.push(format!("{i:4}  {}", encoding_rs::WINDOWS_1252.decode(text).0));
        at += 12 + size;
    }
    lines
}

/// Les textes d'un fichier de messages BMG : INF1 donne où commence chaque texte dans DAT1.
fn bmg_lines(bytes: &[u8]) -> Vec<String> {
    let u32_at = |at: usize| bytes.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize);
    let u16_at = |at: usize| bytes.get(at..at + 2).map(|s| u16::from_le_bytes([s[0], s[1]]) as usize);
    let encoding = bytes.get(0x10).copied().unwrap_or(2);
    let (mut inf, mut dat) = (None, None);
    let mut at = 0x20;
    while let (Some(magic), Some(size)) = (bytes.get(at..at + 4), u32_at(at + 4)) {
        match magic {
            b"INF1" => inf = Some(at),
            b"DAT1" => dat = Some(at + 8),
            _ => {}
        }
        if size < 8 {
            break;
        }
        at += size;
    }
    let (Some(inf), Some(dat)) = (inf, dat) else { return Vec::new() };
    let (count, entry) = (u16_at(inf + 8).unwrap_or(0), u16_at(inf + 10).unwrap_or(4).max(4));
    let mut lines = Vec::with_capacity(count);
    for i in 0..count {
        let Some(offset) = u32_at(inf + 16 + i * entry) else { break };
        let Some(rest) = bytes.get(dat + offset..) else { continue };
        let text = if encoding == 2 {
            // UTF-16 ; 0x1A = une commande du jeu, l'octet suivant donne sa taille
            let (mut units, mut p) = (Vec::new(), 0);
            while let Some(c) = rest.get(p..p + 2).map(|s| u16::from_le_bytes([s[0], s[1]])) {
                if c == 0 {
                    break;
                }
                if c == 0x1A {
                    units.extend("[*]".encode_utf16());
                    p += (rest.get(p + 2).copied().unwrap_or(4) as usize).max(4);
                    continue;
                }
                units.push(c);
                p += 2;
            }
            String::from_utf16_lossy(&units)
        } else {
            let end = rest.iter().position(|b| *b == 0).unwrap_or(rest.len());
            let codec = if encoding == 3 { encoding_rs::SHIFT_JIS } else if encoding == 4 { encoding_rs::UTF_8 } else { encoding_rs::WINDOWS_1252 };
            codec.decode(&rest[..end]).0.into_owned()
        };
        lines.push(format!("{i:4}  {}", text.replace('\n', " / ")));
    }
    lines
}

/// Les fichiers Nitro collés dans un fichier maison (`.dat`, souvent) : leur en-tête donne leur taille.
fn embedded_files(bytes: &[u8]) -> Vec<(usize, usize, &'static str)> {
    const SIGNATURES: [(&[u8; 4], &str); 12] = [
        (b"RLCN", "nclr"),
        (b"RPCN", "nclr"),
        (b"RGCN", "ncgr"),
        (b"RCBN", "ncbr"),
        (b"RCSN", "nscr"),
        (b"RECN", "ncer"),
        (b"RNAN", "nanr"),
        (b"BMD0", "nsbmd"),
        (b"BTX0", "nsbtx"),
        (b"BCA0", "nsbca"),
        (b"JNCD", "bncd"),
        (b"JNCL", "bncl"),
    ];
    let mut found = Vec::new();
    let mut at = 0;
    while at + 16 <= bytes.len() {
        let size = u32::from_le_bytes([bytes[at + 8], bytes[at + 9], bytes[at + 10], bytes[at + 11]]) as usize;
        let known = SIGNATURES.iter().find(|(magic, _)| &bytes[at..at + 4] == magic.as_slice());
        match known {
            // En-tête complet : 0xFEFF, taille, taille de l'en-tête (16), nombre de blocs. Sans ça, ce sont
            // les premiers octets d'un fichier compressé (Livly Garden).
            Some((_, ext)) if bytes[at + 4..at + 6] == [0xFF, 0xFE] && bytes[at + 12..at + 14] == [16, 0] && matches!(bytes[at + 14..at + 16], [1..=8, 0]) && size >= 16 && at + size <= bytes.len() => {
                found.push((at, at + size, *ext));
                at += size.next_multiple_of(4);
            }
            _ => at += 4,
        }
    }
    found
}

/// Les fichiers de l'archive numérotée rom.bin : `rom.bin#n`, nommés `<n sur 5 chiffres>.<ext>`.
fn scan_rom_bin(rom_path: &Path, data: &[u8], base: usize) -> Vec<Entry> {
    romarc::entries(data)
        .into_iter()
        .enumerate()
        .map(|(n, (at, size))| {
            let bytes = unpack(romarc::unpack(&data[at..at + size]));
            let name = format!("{n:05}.{}", romarc::extension(&bytes));
            Entry { path: format!("rom.bin#{n}"), member_name: Some(name), rom: rom_path.to_path_buf(), span: (base + at, base + at + size), member: None, kind: sniff(&bytes), size: bytes.len(), dir: "rom.bin".into(), stem: n.to_string(), rom_index: Some(n as u32) }
        })
        .collect()
}

fn read_span(rom: &Path, (start, end): (usize, usize)) -> Result<Vec<u8>, String> {
    let mut file = std::fs::File::open(rom).map_err(|e| e.to_string())?;
    file.seek(SeekFrom::Start(start as u64)).map_err(|e| e.to_string())?;
    let mut bytes = vec![0u8; end - start];
    file.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes)
}

pub fn read_entry(entry: &Entry) -> Result<Vec<u8>, String> {
    let raw = read_span(&entry.rom, entry.span)?;
    if entry.rom_index.is_some() {
        return Ok(unpack(romarc::unpack(&raw)));
    }
    let bytes = unpack(raw);
    Ok(match entry.member {
        Some((start, end)) => unpack(bytes.get(start..end).ok_or(tr("archive changed since it was read", "archive changee depuis la lecture"))?.to_vec()),
        None => bytes,
    })
}

// --------------------------------------------------------------------------------------------
//  Aperçu d'un fichier
// --------------------------------------------------------------------------------------------

/// Choix de l'aperçu (fichiers qui vont avec, palette, cellule...).
#[derive(Default)]
pub struct Choices {
    pub palettes: Vec<usize>,
    pub palette: usize,
    pub tiles: Vec<usize>,
    pub tile: usize,
    /// Fichiers de textures pour un modèle ; `texture` 0 = aucun, n = `textures[n - 1]`.
    pub textures: Vec<usize>,
    pub texture: usize,
    pub bank: usize,
    pub banks: usize,
    pub width: u32,
    pub cell: usize,
    pub cells: usize,
    pub model: usize,
    pub models: usize,
    /// Fichier lié : le modèle d'une animation 3D, les cellules d'une animation 2D ou d'une mise en page.
    pub linked: Vec<usize>,
    pub link: usize,
    /// Séquence ou animation montrée, dans le fichier.
    pub anim: usize,
    pub anims: usize,
}

/// Les fichiers d'une sorte qui vont le mieux avec `sel` : même dossier d'abord, puis le nom
/// le plus proche ; tout le jeu s'il n'y en a aucun dans le dossier.
fn companions(entries: &[Entry], sel: usize, kinds: &[Kind]) -> Vec<usize> {
    let me = &entries[sel];
    let closeness = |e: &Entry| {
        // rom.bin : les fichiers qui vont ensemble ont des numéros proches
        if let (Some(a), Some(b)) = (me.rom_index, e.rom_index) {
            return (false, std::cmp::Reverse(usize::MAX - a.abs_diff(b) as usize));
        }
        let common = me.stem.chars().zip(e.stem.chars()).take_while(|(a, b)| a == b).count();
        (e.stem != me.stem, std::cmp::Reverse(common))
    };
    let mut near: Vec<usize> = (0..entries.len()).filter(|&i| i != sel && kinds.contains(&entries[i].kind) && entries[i].dir == me.dir).collect();
    if near.is_empty() {
        near = (0..entries.len()).filter(|&i| i != sel && kinds.contains(&entries[i].kind)).collect();
    }
    near.sort_by_key(|&i| (closeness(&entries[i]), entries[i].path.clone()));
    near.truncate(400);
    near
}

/// Les textures d'un modèle sont souvent dans un `.nsbtx` à part : on prend le fichier qui
/// donne le plus de textures trouvées (0 = aucun, le modèle a déjà les siennes).
fn best_textures(entries: &[Entry], model: usize, candidates: &[usize]) -> usize {
    let Ok(bytes) = read_entry(&entries[model]) else { return 0 };
    let Ok(own) = model3d::read(&bytes) else { return 0 };
    let missing = |others: &[nds_nitro::nitro::Container]| model3d::build_models(&own, others).iter().map(|m| m.warnings.len()).sum::<usize>();
    let mut best = (missing(&[]), 0);
    if best.0 == 0 {
        return 0;
    }
    for (n, &i) in candidates.iter().take(40).enumerate() {
        let Ok(other) = read_entry(&entries[i]).and_then(|b| model3d::read(&b).map_err(|e| e.to_string())) else { continue };
        let left = missing(std::slice::from_ref(&other));
        if left < best.0 {
            best = (left, n + 1);
        }
        if left == 0 {
            break;
        }
    }
    best.1
}

/// Les modèles qui ont le squelette de l'animation (le même nombre d'os), les plus proches d'abord.
fn models_for_animation(entries: &[Entry], sel: usize) -> Vec<usize> {
    let Some(bones) = read_entry(&entries[sel]).ok().and_then(|b| model3d::read(&b).ok()).and_then(|c| c.animations.first().map(|a| a.objects_curves.len())) else { return Vec::new() };
    // Écart entre le squelette du modèle et l'animation : un modèle peut avoir quelques os de plus
    let gap = |i: usize| read_entry(&entries[i]).ok().and_then(|b| model3d::read(&b).ok()).and_then(|c| c.models.iter().filter(|m| m.objects.len() >= bones).map(|m| m.objects.len() - bones).min()).filter(|gap| *gap <= MAX_EXTRA_BONES);
    let mut found: Vec<(usize, usize)> = companions(entries, sel, &[Kind::Model]).into_iter().take(150).filter_map(|i| gap(i).map(|g| (g, i))).collect();
    found.sort_by_key(|(g, _)| *g);
    found.into_iter().map(|(_, i)| i).take(20).collect()
}

const MAX_EXTRA_BONES: usize = 3;

/// Les fichiers devinés pour montrer `sel`.
pub fn choices_for(entries: &[Entry], sel: usize) -> Choices {
    let kind = entries[sel].kind;
    let mut c = Choices::default();
    match kind {
        Kind::Tiles | Kind::Screen | Kind::Cells => {
            c.palettes = companions(entries, sel, &[Kind::Palette]);
            if kind != Kind::Tiles {
                c.tiles = companions(entries, sel, &[Kind::Tiles]);
            }
        }
        Kind::CellAnim => {
            c.linked = companions(entries, sel, &[Kind::Cells]);
            c.palettes = companions(entries, sel, &[Kind::Palette]);
            c.tiles = companions(entries, sel, &[Kind::Tiles]);
        }
        Kind::JCells | Kind::EngineBitmap => c.palettes = companions(entries, sel, &[Kind::Palette]),
        Kind::Texel => {
            // `x_texel` : la texture ; `x` : ce qui la relie à sa palette
            let path = entries[sel].path.strip_suffix("_texel").map(str::to_string);
            let reference = path.and_then(|p| entry_at(entries, &p));
            c.palettes = texture_palettes(entries, reference, sel);
        }
        Kind::TextureRef => {
            let target = read_entry(&entries[sel]).ok().and_then(|b| texture_ref(&b)).and_then(|(texel, _)| entry_at(entries, &texel));
            c.linked = target.into_iter().collect();
            c.palettes = texture_palettes(entries, Some(sel), sel);
        }
        Kind::RawTiles | Kind::RawTexture => c.palettes = companions(entries, sel, &[Kind::RawPalette]),
        Kind::RawScreen => {
            c.tiles = companions(entries, sel, &[Kind::RawTiles]);
            c.palettes = companions(entries, sel, &[Kind::RawPalette]);
        }
        Kind::JLayout => {
            c.linked = companions(entries, sel, &[Kind::JCells]);
            // La palette va avec les cellules (`training_obj`), pas avec la mise en page (`training`)
            c.palettes = companions(entries, c.linked.first().copied().unwrap_or(sel), &[Kind::Palette]);
        }
        Kind::ModelAnim => {
            c.linked = catch_unwind(AssertUnwindSafe(|| models_for_animation(entries, sel))).unwrap_or_default();
        }
        Kind::Model => {
            c.textures = companions(entries, sel, &[Kind::Textures]);
            // Un fichier mal formé ne doit pas faire tomber le studio
            c.texture = catch_unwind(AssertUnwindSafe(|| best_textures(entries, sel, &c.textures))).unwrap_or(0);
        }
        _ => {}
    }
    c
}

pub enum Decoded {
    Image { rgba: Rgba, info: String },
    Model { model: model3d::ModelOut, own_textures: usize },
    /// Une animation de cellules : chaque image et sa durée (1/60 s).
    Frames { frames: Vec<(Rgba, u16)>, info: String },
    /// Un modèle et l'animation jouée dessus.
    Animated { model: model3d::ModelOut, animation: model3d::AnimOut },
    Audio { wav: Vec<u8>, waveform: Rgba, info: String },
    Text { lines: Vec<String>, info: String },
}

const TEXT_BYTES: usize = 16 * 1024;

/// Les suites de caractères d'un fichier binaire (ASCII et Shift-JIS).
fn strings_in(bytes: &[u8]) -> Vec<String> {
    let mut found = Vec::new();
    let mut start = None;
    let mut i = 0;
    let mut close = |start: &mut Option<usize>, end: usize| {
        if let Some(from) = start.take() {
            let run: &[u8] = &bytes[from..end];
            if run.len() >= 4 {
                let (text, _, bad) = encoding_rs::SHIFT_JIS.decode(run);
                // Du japonais a des kana (« 謎の男 ») ; des kanji seuls, c'est du binaire lu comme du texte
                let kana = |c: char| matches!(c, '\u{3040}'..='\u{30FF}');
                let readable = text.is_ascii() || text.chars().any(kana);
                if !bad && readable && text.chars().any(|c| c.is_alphanumeric()) {
                    found.push(text.into_owned());
                }
            }
        }
    };
    while i < bytes.len() {
        let b = bytes[i];
        let width = match b {
            0x20..=0x7E => 1,
            0x81..=0x9F | 0xE0..=0xFC if matches!(bytes.get(i + 1), Some(0x40..=0x7E | 0x80..=0xFC)) => 2,
            _ => 0,
        };
        if width == 0 {
            close(&mut start, i);
            i += 1;
        } else {
            start.get_or_insert(i);
            i += width;
        }
    }
    close(&mut start, bytes.len());
    found
}

/// Le contenu d'un fichier qu'on ne sait pas décoder : ses textes, puis ses octets.
pub fn text_lines(bytes: &[u8]) -> Vec<String> {
    let strings = strings_in(bytes);
    let in_text: usize = strings.iter().map(String::len).sum();
    let mut text = vec![t!("Text found: {}", "Textes trouves : {}", strings.len())];
    text.extend(strings.into_iter().take(2000));
    let mut hex = vec![t!("Bytes: {} shown of {}", "Octets : {} montres sur {}", bytes.len().min(TEXT_BYTES), bytes.len())];
    for (row, chunk) in bytes[..bytes.len().min(TEXT_BYTES)].chunks(16).enumerate() {
        let values: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
        let ascii: String = chunk.iter().map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { '.' }).collect();
        hex.push(format!("{:06x}  {:<47}  {ascii}", row * 16, values.join(" ")));
    }
    // Un fichier surtout fait de texte commence par ce texte ; sinon les quelques mots viennent après les octets
    let (first, second) = if in_text * 5 >= bytes.len() { (text, hex) } else { (hex, text) };
    first.into_iter().chain([String::new()]).chain(second).collect()
}

pub fn size_text(size: usize) -> String {
    if size >= 1 << 20 {
        t!("{:.1} MB", "{:.1} Mo", size as f64 / (1 << 20) as f64)
    } else if size >= 1024 {
        t!("{} KB", "{} Ko", size / 1024)
    } else {
        t!("{size} B", "{size} o")
    }
}

/// Le nom du type de fichier dans la langue choisie (`Kind::label` reste la clé en français).
pub fn kind_text(kind: Kind) -> &'static str {
    match kind.label() {
        "modele 3D" => tr("3D model", "modele 3D"),
        "textures 3D" => tr("3D textures", "textures 3D"),
        "tuiles" => tr("tiles", "tuiles"),
        "ecran" => tr("screen", "ecran"),
        "cellules" => tr("cells", "cellules"),
        "animation 3D" => tr("3D animation", "animation 3D"),
        "animation 2D" => tr("2D animation", "animation 2D"),
        "cellules J" => tr("cells", "cellules"),
        "mise en page" => tr("layout", "mise en page"),
        "son" => tr("sound", "son"),
        "banque de sons" => tr("sound bank", "banque de sons"),
        "archive de son" => tr("sound archive", "archive de son"),
        "police" => tr("font", "police"),
        "texte" => tr("text", "texte"),
        "video" => "video",
        "compresse" => tr("compressed", "compresse"),
        other => other,
    }
}

/// Les images en grille, avec 4 pixels entre elles.
fn sheet(images: &[Rgba]) -> Rgba {
    const GAP: u32 = 4;
    let max_w = images.iter().map(|i| i.width).max().unwrap_or(1).max(512);
    let (mut x, mut y, mut line_h, mut places) = (0, 0, 0, Vec::new());
    for img in images {
        if x > 0 && x + img.width > max_w {
            (x, y, line_h) = (0, y + line_h + GAP, 0);
        }
        places.push((x, y));
        x += img.width + GAP;
        line_h = line_h.max(img.height);
    }
    let (width, height) = (max_w, (y + line_h).max(1));
    let mut out = Rgba { width, height, data: vec![0; (width * height * 4) as usize] };
    for (img, (px, py)) in images.iter().zip(places) {
        for row in 0..img.height {
            let src = (row * img.width * 4) as usize;
            let dst = (((py + row) * width + px) * 4) as usize;
            out.data[dst..dst + (img.width * 4) as usize].copy_from_slice(&img.data[src..src + (img.width * 4) as usize]);
        }
    }
    out
}

fn audio(sound: sadl::Sound, extra: String) -> Decoded {
    let info = t!("{extra}{:.1} s, {} Hz, {} channels", "{extra}{:.1} s, {} Hz, {} voies", sound.seconds(), sound.rate, sound.channels);
    Decoded::Audio { wav: sound.wav(), waveform: sound.waveform(), info }
}

/// Sans palette : niveaux de gris.
fn grey_palette(tiles: &g2d::Tiles) -> g2d::Palette {
    g2d::Palette { colors: (0..256).map(|i| if tiles.bpp8 { [i as u8, i as u8, i as u8, 255] } else { let v = (i % 16 * 17) as u8; [v, v, v, 255] }).collect() }
}

fn decode_inner(entries: &[Entry], sel: usize, c: &mut Choices) -> Result<Decoded, String> {
    let bytes = read_entry(&entries[sel])?;
    let kind = entries[sel].kind;
    let load = |list: &[usize], i: usize| list.get(i).map(|&e| read_entry(&entries[e])).transpose();
    match kind {
        Kind::Palette => {
            let p = g2d::read_palette(&bytes).map_err(|e| e.to_string())?;
            Ok(Decoded::Image { rgba: p.swatches(), info: t!("{} colors", "{} couleurs", p.colors.len()) })
        }
        Kind::Tiles | Kind::Screen | Kind::Cells => {
            let tiles_bytes = if kind == Kind::Tiles { Some(bytes.clone()) } else { load(&c.tiles, c.tile)? };
            let tiles = g2d::read_tiles(tiles_bytes.as_deref().ok_or(tr("no tile file (.ncgr) found", "aucun fichier de tuiles (.ncgr) trouve"))?).map_err(|e| e.to_string())?;
            let palette = match load(&c.palettes, c.palette)? {
                Some(p) => g2d::read_palette(&p).map_err(|e| e.to_string())?,
                None => grey_palette(&tiles),
            };
            c.banks = if tiles.bpp8 { 1 } else { palette.banks() };
            c.bank = c.bank.min(c.banks.saturating_sub(1));
            let what = t!("{} tiles {} bpp{}", "{} tuiles {} bpp{}", tiles.count(), if tiles.bpp8 { 8 } else { 4 }, if tiles.linear { " (bitmap)" } else { "" });
            match kind {
                Kind::Tiles => {
                    if c.width == 0 {
                        c.width = tiles.default_width();
                    }
                    Ok(Decoded::Image { rgba: g2d::render_tiles(&tiles, &palette, c.bank, c.width), info: what })
                }
                Kind::Screen => {
                    let screen = g2d::read_screen(&bytes).map_err(|e| e.to_string())?;
                    Ok(Decoded::Image { rgba: g2d::render_screen(&screen, &tiles, &palette), info: t!("screen {}x{}, {what}", "ecran {}x{}, {what}", screen.width, screen.height) })
                }
                _ => {
                    let cells = g2d::read_cells(&bytes).map_err(|e| e.to_string())?;
                    c.cells = cells.cells.len();
                    c.cell = c.cell.min(c.cells.saturating_sub(1));
                    let objs = cells.cells.get(c.cell).map_or(0, |o| o.len());
                    Ok(Decoded::Image { rgba: g2d::render_cell(&cells, c.cell, &tiles, &palette), info: t!("{} cells ({objs} OBJ), {what}", "{} cellules ({objs} OBJ), {what}", cells.cells.len()) })
                }
            }
        }
        Kind::CellAnim => {
            let sequences = g2d::read_animations(&bytes).map_err(|e| e.to_string())?;
            c.anims = sequences.len();
            c.anim = c.anim.min(c.anims.saturating_sub(1));
            let sequence = sequences.get(c.anim).ok_or(tr("no sequence in the file", "aucune sequence dans le fichier"))?;
            let cells_bytes = load(&c.linked, c.link)?.ok_or(tr("no cell file (.ncer) found", "aucun fichier de cellules (.ncer) trouve"))?;
            let cells = g2d::read_cells(&cells_bytes).map_err(|e| e.to_string())?;
            let tiles_bytes = load(&c.tiles, c.tile)?.ok_or(tr("no tile file (.ncgr) found", "aucun fichier de tuiles (.ncgr) trouve"))?;
            let tiles = g2d::read_tiles(&tiles_bytes).map_err(|e| e.to_string())?;
            let palette = match load(&c.palettes, c.palette)? {
                Some(p) => g2d::read_palette(&p).map_err(|e| e.to_string())?,
                None => grey_palette(&tiles),
            };
            let frames = g2d::render_sequence(&cells, sequence, &tiles, &palette);
            let info = t!("{} sequences, {} frames in this one", "{} sequences, {} images dans celle-ci", sequences.len(), frames.len());
            Ok(Decoded::Frames { frames, info })
        }
        Kind::JCells | Kind::JLayout => {
            let is_layout = kind == Kind::JLayout;
            let cells_bytes = if is_layout { load(&c.linked, c.link)?.ok_or(tr("no cell file (.bncd) found", "aucun fichier de cellules (.bncd) trouve"))? } else { bytes.clone() };
            let (cells, tiles) = g2d::read_jcells(&cells_bytes).map_err(|e| e.to_string())?;
            let palette = match load(&c.palettes, c.palette)? {
                Some(p) => g2d::read_palette(&p).map_err(|e| e.to_string())?,
                None => grey_palette(&tiles),
            };
            if is_layout {
                let placements = g2d::read_jlayout(&bytes).map_err(|e| e.to_string())?;
                let info = t!("{} cells placed on the screen", "{} cellules posees sur l'ecran", placements.len());
                return Ok(Decoded::Image { rgba: g2d::render_layout(&placements, &cells, &tiles, &palette), info });
            }
            c.cells = cells.cells.len();
            c.cell = c.cell.min(c.cells.saturating_sub(1));
            let objs = cells.cells.get(c.cell).map_or(0, |o| o.len());
            let info = t!("{} cells ({objs} OBJ)", "{} cellules ({objs} OBJ)", cells.cells.len());
            Ok(Decoded::Image { rgba: g2d::render_cell(&cells, c.cell, &tiles, &palette), info })
        }
        Kind::ModelAnim => {
            let cont = model3d::read(&bytes).map_err(|e| e.to_string())?;
            c.anims = cont.animations.len();
            c.anim = c.anim.min(c.anims.saturating_sub(1));
            let animation = cont.animations.get(c.anim).ok_or(tr("no animation in the file", "aucune animation dans le fichier"))?;
            let Some(&rig) = c.linked.get(c.link) else {
                // Le modèle n'est pas lisible dans la rom (archive compressée...) : ce que dit l'animation
                let mut lines = vec![tr("No model with this skeleton found in the rom.", "Aucun modele avec ce squelette trouve dans la rom.").to_string(), String::new()];
                lines.extend(cont.animations.iter().map(|a| t!("{}: {} frames, {} bones", "{} : {} images, {} os", a.name, a.num_frames, a.objects_curves.len())));
                return Ok(Decoded::Text { lines, info: t!("{} animations", "{} animations", cont.animations.len()) });
            };
            let own = model3d::read(&read_entry(&entries[rig])?).map_err(|e| e.to_string())?;
            // Les textures du modèle, comme pour le modèle seul
            let textures = companions(entries, rig, &[Kind::Textures]);
            let others = match best_textures(entries, rig, &textures) {
                0 => Vec::new(),
                n => vec![model3d::read(&read_entry(&entries[textures[n - 1]])?).map_err(|e| e.to_string())?],
            };
            let lib = model3d::TextureLib::new(&own, &others);
            let bones = animation.objects_curves.len();
            let model = own.models.iter().filter(|m| m.objects.len() >= bones).min_by_key(|m| m.objects.len()).ok_or(tr("no model with this skeleton found", "aucun modele avec ce squelette trouve"))?;
            let out = model3d::build_animation(model, animation).ok_or(tr("no model with this skeleton found", "aucun modele avec ce squelette trouve"))?;
            Ok(Decoded::Animated { model: model3d::build_model(model, &lib), animation: out })
        }
        Kind::Audio => {
            let sound = match bytes.get(0..4) {
                Some(b"STRM") => sdat::read_strm(&bytes),
                Some(b"SWAV") => sdat::read_swav(&bytes),
                _ if adx::is_adx(&bytes) => adx::decode(&bytes),
                _ => sadl::decode(&bytes),
            };
            Ok(audio(sound.map_err(|e| e.to_string())?, String::new()))
        }
        Kind::AudioBank => {
            c.anims = sdat::swar_count(&bytes).map_err(|e| e.to_string())?;
            c.anim = c.anim.min(c.anims.saturating_sub(1));
            let sound = sdat::read_swar(&bytes, c.anim).map_err(|e| e.to_string())?;
            let extra = t!("sound {} of {}, ", "son {} sur {}, ", c.anim + 1, c.anims);
            Ok(audio(sound, extra))
        }
        Kind::PlainText => {
            // UTF-16 avec sa marque, UTF-8, sinon Shift-JIS (les jeux japonais)
            let text = match bytes.get(0..2) {
                Some([0xFF, 0xFE]) => encoding_rs::UTF_16LE.decode(&bytes).0.into_owned(),
                _ => match std::str::from_utf8(&bytes) {
                    Ok(text) => text.to_string(),
                    Err(_) => encoding_rs::SHIFT_JIS.decode(&bytes).0.into_owned(),
                },
            };
            let lines: Vec<String> = text.lines().map(str::to_string).collect();
            let info = t!("{} lines", "{} lignes", lines.len());
            Ok(Decoded::Text { lines, info })
        }
        Kind::Compressed => match decompress_any(&bytes) {
            Some(inner) => {
                let info = t!("LZ77, {} once decompressed", "LZ77, {} une fois decompresse", size_text(inner.len()));
                Ok(Decoded::Text { lines: text_lines(&inner), info })
            }
            None => Ok(Decoded::Text { lines: text_lines(&bytes), info: t!("Not LZ77: its text and bytes", "Pas du LZ77 : ses textes et ses octets") }),
        },
        Kind::NestedRom => {
            let info = crate::library::read_rom_bytes(&entries[sel].path, &bytes)?;
            let rgba = if info.icon.is_empty() { Rgba { width: 1, height: 1, data: vec![0; 4] } } else { Rgba { width: 32, height: 32, data: info.icon.clone() } };
            Ok(Decoded::Image { rgba, info: format!("{}  -  {}  -  {}", info.title.replace('\n', " "), info.publisher, info.code) })
        }
        Kind::EngineGeometry | Kind::EngineMesh => {
            let name = entries[sel].path.rsplit('/').next().unwrap_or("").to_string();
            if kind == Kind::EngineGeometry {
                let model = gx_mesh::read_geometry(&bytes, None, &name).map_err(|e| e.to_string())?;
                return Ok(Decoded::Model { model, own_textures: 0 });
            }
            let parts = mesh_parts(&bytes);
            let mut models = Vec::with_capacity(parts.len());
            let mut textured = 0;
            for (geometry, texture, palette) in &parts {
                // Un morceau manquant ou vide n'empêche pas de montrer les autres
                let Some(at) = entry_at(entries, geometry) else { continue };
                let Ok(geometry) = read_entry(&entries[at]) else { continue };
                let image = entry_at(entries, texture).and_then(|i| engine_texture(entries, i, palette.as_deref()));
                textured += usize::from(image.is_some());
                if let Ok(model) = gx_mesh::read_geometry(&geometry, image, &name) {
                    models.push(model);
                }
            }
            let model = gx_mesh::merge(&name, models).ok_or(tr("unreadable mesh", "maillage illisible"))?;
            Ok(Decoded::Model { model, own_textures: textured })
        }
        Kind::BsTexture => {
            let inner = decompress_any(&bytes).unwrap_or_else(|| bytes.clone());
            let (texel, palette) = g2d::read_bstga(&inner).map_err(|e| e.to_string())?;
            let rgba = g2d::render_texel(&texel, &palette).map_err(|e| e.to_string())?;
            Ok(Decoded::Image { rgba, info: t!("{}x{}, DS texture format {}", "{}x{}, format de texture DS {}", texel.width, texel.height, texel.format) })
        }
        Kind::RawTexture => {
            let palette = match load(&c.palettes, c.palette)? {
                Some(p) => g2d::raw_palette(&p),
                None => g2d::Palette { colors: (0..=255).map(|i| [i, i, i, 255]).collect() },
            };
            let texel = g2d::raw_texture(&bytes, &palette);
            let rgba = g2d::render_texel(&texel, &palette).map_err(|e| e.to_string())?;
            Ok(Decoded::Image { rgba, info: t!("{}x{} (size guessed)", "{}x{} (taille devinee)", texel.width, texel.height) })
        }
        Kind::Bitmap => {
            let rgba = g2d::read_bmp(&bytes).map_err(|e| e.to_string())?;
            let info = format!("{}x{}", rgba.width, rgba.height);
            Ok(Decoded::Image { rgba, info })
        }
        Kind::ZipFile => {
            let lines = zip_lines(&bytes);
            Ok(Decoded::Text { lines, info: t!("Zip archive: the list of its files", "Archive zip : la liste de ses fichiers") })
        }
        Kind::PaletteTiles => {
            let inner = decompress_any(&bytes).unwrap_or_else(|| bytes.clone());
            let (palette, tiles) = g2d::read_palette_tiles(&inner).ok_or(tr("unreadable tile sheet", "planche de tuiles illisible"))?;
            if c.width == 0 {
                // 40 tuiles de large = 320 pixels, la largeur des planches de Heroes of Mana
                c.width = if tiles.count() % 40 == 0 && tiles.count() % 32 != 0 { 40 } else { 32 };
            }
            let info = t!("{} tiles {} bpp, palette included", "{} tuiles {} bpp, palette incluse", tiles.count(), if tiles.bpp8 { 8 } else { 4 });
            Ok(Decoded::Image { rgba: g2d::render_tiles(&tiles, &palette, 0, c.width), info })
        }
        Kind::AnmFile => Ok(Decoded::Text { lines: anm_lines(&bytes), info: t!("Animation not played: its content", "Animation pas jouee : son contenu") }),
        Kind::ZImage => {
            let inner = decompress_any(&bytes).unwrap_or_else(|| bytes.clone());
            match g2d::read_z_image(&inner) {
                Some(rgba) => {
                    let info = format!("{}x{}", rgba.width, rgba.height);
                    Ok(Decoded::Image { rgba, info })
                }
                None => Ok(Decoded::Text { lines: text_lines(&inner), info: t!("Not an image: {} once decompressed", "Pas une image : {} une fois decompresse", size_text(inner.len())) }),
            }
        }
        Kind::Video => Ok(Decoded::Text { lines: video_lines(&bytes), info: t!("Video: not played", "Video : pas lue") }),
        Kind::Texel | Kind::TextureRef => {
            let texel_bytes = if kind == Kind::Texel { bytes.clone() } else { load(&c.linked, c.link)?.ok_or(tr("texture file not found", "fichier de la texture introuvable"))? };
            let texel = g2d::read_texel(&texel_bytes).map_err(|e| e.to_string())?;
            let palette = match load(&c.palettes, c.palette)? {
                Some(p) => g2d::read_palette(&p).map_err(|e| e.to_string())?,
                None => g2d::Palette { colors: (0..=255).map(|i| [i, i, i, 255]).collect() },
            };
            let rgba = g2d::render_texel(&texel, &palette).map_err(|e| e.to_string())?;
            Ok(Decoded::Image { rgba, info: t!("{}x{}, DS texture format {}", "{}x{}, format de texture DS {}", texel.width, texel.height, texel.format) })
        }
        Kind::EngineBitmap => {
            let palette = match load(&c.palettes, c.palette)? {
                Some(p) => g2d::read_palette(&p).map_err(|e| e.to_string())?,
                None => g2d::Palette { colors: (0..=255).map(|i| [i, i, i, 255]).collect() },
            };
            let rgba = g2d::render_engine_bitmap(&bytes, &palette).map_err(|e| e.to_string())?;
            let info = format!("{}x{}", rgba.width, rgba.height);
            Ok(Decoded::Image { rgba, info })
        }
        Kind::Font => {
            let (rgba, count) = g2d::render_font(&bytes).map_err(|e| e.to_string())?;
            Ok(Decoded::Image { rgba, info: t!("{count} characters", "{count} caracteres") })
        }
        Kind::Messages => {
            let lines = if bytes.starts_with(b"GNAL") { lang_lines(&bytes) } else { bmg_lines(&bytes) };
            let info = t!("{} messages", "{} messages", lines.len());
            Ok(Decoded::Text { lines, info })
        }
        Kind::RawPalette => {
            let p = g2d::raw_palette(&bytes);
            Ok(Decoded::Image { rgba: p.swatches(), info: t!("{} colors", "{} couleurs", p.colors.len()) })
        }
        Kind::RawTiles | Kind::RawScreen => {
            let palette = load(&c.palettes, c.palette)?.map(|p| g2d::raw_palette(&p));
            let tiles_bytes = if kind == Kind::RawTiles { Some(bytes.clone()) } else { load(&c.tiles, c.tile)? };
            let tiles = g2d::raw_tiles(&tiles_bytes.ok_or(tr("no tile file (.nbfc) found", "aucun fichier de tuiles (.nbfc) trouve"))?, palette.as_ref());
            let palette = palette.unwrap_or_else(|| grey_palette(&tiles));
            if kind == Kind::RawScreen {
                let screen = g2d::raw_screen(&bytes);
                return Ok(Decoded::Image { rgba: g2d::render_screen(&screen, &tiles, &palette), info: t!("screen {}x{}", "ecran {}x{}", screen.width, screen.height) });
            }
            c.banks = if tiles.bpp8 { 1 } else { palette.banks() };
            c.bank = c.bank.min(c.banks.saturating_sub(1));
            if c.width == 0 {
                c.width = tiles.default_width();
            }
            let info = t!("{} tiles {} bpp", "{} tuiles {} bpp", tiles.count(), if tiles.bpp8 { 8 } else { 4 });
            Ok(Decoded::Image { rgba: g2d::render_tiles(&tiles, &palette, c.bank, c.width), info })
        }
        Kind::Textures => {
            let cont = model3d::read(&bytes).map_err(|e| e.to_string())?;
            let all = model3d::texture_sheet(&cont);
            let ok: Vec<Rgba> = all.iter().filter_map(|(_, r)| r.as_ref().ok().cloned()).collect();
            let names: Vec<String> = all.iter().map(|(n, _)| n.clone()).take(12).collect();
            let more = if all.len() > 12 { ", ..." } else { "" };
            Ok(Decoded::Image { rgba: sheet(&ok), info: format!("{} textures, {} palettes : {}{more}", cont.textures.len(), cont.palettes.len(), names.join(", ")) })
        }
        Kind::Model => {
            let own = model3d::read(&bytes).map_err(|e| e.to_string())?;
            let others = match c.texture {
                0 => Vec::new(),
                n => match load(&c.textures, n - 1)? {
                    Some(t) => vec![model3d::read(&t).map_err(|e| e.to_string())?],
                    None => Vec::new(),
                },
            };
            let mut models = model3d::build_models(&own, &others);
            c.models = models.len();
            c.model = c.model.min(models.len().saturating_sub(1));
            if models.is_empty() {
                return Err(tr("no model in the file", "aucun modele dans le fichier").into());
            }
            Ok(Decoded::Model { model: models.swap_remove(c.model), own_textures: own.textures.len() })
        }
        Kind::Nitro3dOther => {
            // Animations de matériaux (.nsbta, .nsbtp...) : ce qu'elles contiennent, sans les jouer
            let mut lines = Vec::new();
            if let Ok(cont) = model3d::read(&bytes) {
                lines.extend(cont.patterns.iter().map(|p| t!("Texture animation {}: {} frames", "Animation de textures {} : {} images", p.name, p.num_frames)));
                lines.extend(cont.mat_anims.iter().map(|m| t!("Material animation {}: {} frames", "Animation de materiau {} : {} images", m.name, m.num_frames)));
            }
            lines.push(String::new());
            lines.extend(text_lines(&bytes));
            Ok(Decoded::Text { lines, info: t!("Animation not played: its content", "Animation pas jouee : son contenu") })
        }
        // Scripts et fichiers maison : leurs textes et leurs octets (décompressés s'ils le sont)
        _ => match decompress_any(&bytes) {
            Some(inner) => {
                let info = t!("Not decoded: {} once decompressed", "Pas decode : {} une fois decompresse", size_text(inner.len()));
                Ok(Decoded::Text { lines: text_lines(&inner), info })
            }
            None => Ok(Decoded::Text { lines: text_lines(&bytes), info: t!("Not decoded: its text and bytes", "Pas decode : ses textes et ses octets") }),
        },
    }
}

pub const THUMB_SIZE: u32 = 96;
/// Images gardées au plus pour une vignette animée.
const THUMB_FRAMES: usize = 24;

/// L'image dans un carré de la vignette : agrandie sans lissage (d'un nombre entier de fois) ou réduite, centrée.
fn fit_thumb(image: &Rgba) -> Rgba {
    let side = THUMB_SIZE as usize;
    let mut out = Rgba { width: THUMB_SIZE, height: THUMB_SIZE, data: vec![0; side * side * 4] };
    let longest = image.width.max(image.height).max(1) as f32;
    let scale = if longest > THUMB_SIZE as f32 { THUMB_SIZE as f32 / longest } else { (THUMB_SIZE as f32 / longest).floor().max(1.0) };
    let (w, h) = (((image.width as f32 * scale) as usize).clamp(1, side), ((image.height as f32 * scale) as usize).clamp(1, side));
    let (ox, oy) = ((side - w) / 2, (side - h) / 2);
    for y in 0..h {
        for x in 0..w {
            let sx = ((x as f32 / scale) as usize).min(image.width.max(1) as usize - 1);
            let sy = ((y as f32 / scale) as usize).min(image.height.max(1) as usize - 1);
            let from = (sy * image.width as usize + sx) * 4;
            if let Some(pixel) = image.data.get(from..from + 4) {
                let to = ((oy + y) * side + ox + x) * 4;
                out.data[to..to + 4].copy_from_slice(pixel);
            }
        }
    }
    out
}

/// La vignette de `sel` : une ou plusieurs images (avec leur durée en 1/60 s) ; `None` pour un fichier sans image.
pub fn thumbnail(entries: &[Entry], sel: usize) -> Option<Vec<(Rgba, u16)>> {
    catch_unwind(AssertUnwindSafe(|| {
        let mut choices = choices_for(entries, sel);
        match decode(entries, sel, &mut choices).ok()? {
            Decoded::Image { rgba, .. } => Some(vec![(fit_thumb(&rgba), 60)]),
            Decoded::Audio { waveform, .. } => Some(vec![(fit_thumb(&waveform), 60)]),
            Decoded::Frames { frames, .. } => {
                let step = frames.len().div_ceil(THUMB_FRAMES).max(1);
                Some(frames.chunks(step).map(|group| (fit_thumb(&group[0].0), group.iter().map(|f| f.1).sum())).collect())
            }
            Decoded::Model { model, .. } => Some(vec![(model3d::render_thumbnail(&model, None, model.min, model.max, THUMB_SIZE), 60)]),
            Decoded::Animated { model, animation } => {
                let step = animation.frames.len().div_ceil(THUMB_FRAMES).max(1);
                let duration = animation.step.saturating_mul(step as u16).max(1);
                Some(animation.frames.iter().step_by(step).map(|pose| (model3d::render_thumbnail(&model, Some(pose), animation.min, animation.max, THUMB_SIZE), duration)).collect())
            }
            Decoded::Text { .. } => None,
        }
    }))
    .unwrap_or(None)
}

/// L'aperçu de `sel`. Un fichier mal formé donne une erreur, il ne fait pas tomber le studio.
pub fn decode(entries: &[Entry], sel: usize, c: &mut Choices) -> Result<Decoded, String> {
    catch_unwind(AssertUnwindSafe(|| decode_inner(entries, sel, c))).unwrap_or_else(|_| Err(tr("decoding crashed (unexpected file)", "le decodage a plante (fichier inattendu)").into()))
}

// --------------------------------------------------------------------------------------------
//  Index de tous les jeux
// --------------------------------------------------------------------------------------------

/// Un format (extension) dans un jeu, ou tous jeux réunis.
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct FormatStat {
    pub count: usize,
    /// Fichiers de ce format que la fenêtre sait ouvrir (d'après leur signature).
    pub viewable: usize,
    /// Fichiers essayés : décodés avec le code de l'aperçu, et ceux qui ont donné quelque chose.
    pub tested: usize,
    pub shown: usize,
    /// Un fichier du format (chemin) et ce que donne l'essai.
    pub sample: Option<String>,
    pub result: Option<String>,
    pub failure: Option<String>,
    pub games: usize,
}

impl FormatStat {
    /// Coche : `[x]` tout est regardable, `[~]` une partie, `[!]` l'essai a échoué, `[ ]` non.
    pub fn check(&self) -> &'static str {
        if self.viewable == 0 {
            "[ ]"
        } else if self.shown == 0 {
            "[!]"
        } else if self.viewable == self.count && self.shown == self.tested {
            "[x]"
        } else {
            "[~]"
        }
    }

    fn merge(&mut self, label: &str, other: &FormatStat) {
        // Le fichier montré en exemple : un regardable si possible
        if self.sample.is_none() || (self.shown == 0 && other.shown > 0) {
            self.sample = other.sample.as_ref().map(|path| format!("{label} : {path}"));
            self.result = other.result.clone();
        }
        self.count += other.count;
        self.viewable += other.viewable;
        self.tested += other.tested;
        self.shown += other.shown;
        self.games += 1;
        if self.failure.is_none() {
            self.failure = other.failure.as_ref().map(|failure| format!("{label} : {failure}"));
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct GameStats {
    stamp: u64,
    pub files: usize,
    pub viewable: usize,
    pub data_bytes: u64,
    pub extracted_bytes: u64,
    pub formats: BTreeMap<String, FormatStat>,
}

impl GameStats {
    pub fn extracted_percent(&self) -> f64 {
        if self.data_bytes == 0 { 100.0 } else { self.extracted_bytes as f64 * 100.0 / self.data_bytes as f64 }
    }

    pub fn viewable_percent(&self) -> f64 {
        if self.files == 0 { 0.0 } else { self.viewable as f64 * 100.0 / self.files as f64 }
    }
}

#[derive(Default)]
pub struct IndexState {
    pub stats: HashMap<String, GameStats>,
    pub done: usize,
    pub total: usize,
    /// Change à chaque nouveau résultat.
    pub revision: u64,
}

/// Tous jeux réunis, du format le plus courant au moins courant.
pub fn all_formats(stats: &HashMap<String, GameStats>) -> Vec<(String, FormatStat)> {
    let mut all: BTreeMap<String, FormatStat> = BTreeMap::new();
    let mut labels: Vec<&String> = stats.keys().collect();
    labels.sort();
    for label in labels {
        for (ext, stat) in &stats[label].formats {
            all.entry(ext.clone()).or_default().merge(label, stat);
        }
    }
    let mut list: Vec<_> = all.into_iter().collect();
    list.sort_by(|a, b| b.1.count.cmp(&a.1.count).then(a.0.cmp(&b.0)));
    list
}

fn modified_secs(path: &Path) -> u64 {
    path.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())
}

fn stamp_of(rom: &Path) -> u64 {
    modified_secs(rom) ^ rom.metadata().map_or(0, |m| m.len()).rotate_left(21)
}

/// Essaie de montrer un fichier avec le code de l'aperçu.
fn try_show(entries: &[Entry], sel: usize) -> Result<String, String> {
    if entries[sel].kind == Kind::Archive {
        let bytes = read_entry(&entries[sel])?;
        return narc::read(&bytes).map(|m| format!("archive de {} fichiers", m.len())).map_err(|e| e.to_string());
    }
    let mut choices = choices_for(entries, sel);
    match decode(entries, sel, &mut choices)? {
        Decoded::Image { rgba, .. } => {
            if rgba.data.chunks_exact(4).any(|p| p[3] != 0) {
                Ok(format!("image {}x{}", rgba.width, rgba.height))
            } else {
                Err(format!("image vide {}x{}", rgba.width, rgba.height))
            }
        }
        Decoded::Frames { frames, .. } => {
            if frames.iter().any(|(image, _)| image.data.chunks_exact(4).any(|p| p[3] != 0)) {
                Ok(format!("animation {} images", frames.len()))
            } else {
                Err("animation vide".into())
            }
        }
        Decoded::Animated { animation, .. } => Ok(format!("animation {} images", animation.frames.len())),
        Decoded::Audio { info, .. } | Decoded::Text { info, .. } => Ok(info),
        Decoded::Model { model, .. } => {
            if model.triangle_count == 0 {
                Err("modele sans triangles".into())
            } else if model.warnings.is_empty() {
                Ok(format!("modele {} triangles", model.triangle_count))
            } else {
                Ok(format!("modele {} triangles, {} textures manquantes", model.triangle_count, model.warnings.len()))
            }
        }
    }
}

fn is_media(entry: &Entry) -> bool {
    let name = entry.path.to_lowercase();
    let ext = entry.format();
    name.contains("sound") || name.contains("movie") || matches!(ext.as_str(), "sdat" | "mods" | "vx" | "moflex" | "dpk")
}

/// Les chiffres d'un jeu, et l'essai d'un fichier de chaque format.
pub fn game_stats(rom: &Path) -> GameStats {
    let entries = scan(rom);
    let mut stats = GameStats { stamp: stamp_of(rom), ..GameStats::default() };
    let rom_bin_total = entries.iter().filter(|e| e.rom_index.is_some()).count();
    for (i, e) in entries.iter().enumerate() {
        let is_top_file = e.member.is_none() && e.rom_index.is_none();
        let parsed_archive = e.kind == Kind::Archive && read_entry(e).ok().is_some_and(|b| narc::read(&b).is_ok());
        let is_rom_bin = is_top_file && e.path.rsplit('/').next().is_some_and(|n| n.eq_ignore_ascii_case("rom.bin"));
        // Octets : les fichiers du NitroFS seulement (ce qui est dedans n'y est pas compté deux fois)
        if is_top_file && !is_media(e) {
            stats.data_bytes += e.size as u64;
            let part = if is_rom_bin {
                if rom_bin_total > 0 { 1.0 } else { 0.0 }
            } else if (e.kind == Kind::Archive && !parsed_archive) || (e.kind == Kind::Other && e.size >= BIG_FILE) {
                0.0
            } else {
                1.0
            };
            stats.extracted_bytes += (e.size as f64 * part) as u64;
        }
        // Fichiers : une archive ouverte ne compte pas, ce qu'elle contient oui
        if !parsed_archive && !(is_rom_bin && rom_bin_total > 0) {
            stats.files += 1;
            stats.viewable += usize::from(e.kind.viewable());
        }
        let format = stats.formats.entry(e.format()).or_default();
        format.count += 1;
        if e.kind.viewable() || parsed_archive {
            format.viewable += 1;
            if format.tested == 0 {
                format.tested = 1;
                match try_show(&entries, i) {
                    Ok(result) => {
                        format.shown = 1;
                        format.sample = Some(e.path.clone());
                        format.result = Some(result);
                    }
                    Err(error) => format.failure = Some(format!("{} : {error}", e.path)),
                }
            }
        } else if format.sample.is_none() {
            format.sample = Some(e.path.clone());
            let what = e.kind.label();
            format.result = Some(if what.is_empty() { "signature inconnue".into() } else { format!("{what} : pas d'apercu") });
        }
    }
    stats
}

fn index_path() -> PathBuf {
    crate::save::data_dir().join("assets_index.json")
}

#[derive(Serialize, Deserialize)]
struct IndexFile {
    version: u32,
    games: HashMap<String, GameStats>,
}

fn load_index() -> HashMap<String, GameStats> {
    let Ok(text) = std::fs::read_to_string(index_path()) else { return HashMap::new() };
    match serde_json::from_str::<IndexFile>(&text) {
        Ok(index) if index.version == INDEX_VERSION => index.games,
        _ => HashMap::new(),
    }
}

fn save_index(stats: &HashMap<String, GameStats>) {
    let index = IndexFile { version: INDEX_VERSION, games: stats.clone() };
    let Ok(text) = serde_json::to_string(&index) else { return };
    let (path, temp) = (index_path(), index_path().with_extension("json.tmp"));
    if std::fs::write(&temp, text).is_ok() {
        let _ = std::fs::rename(&temp, &path);
    }
}

/// Met l'index à jour rom après rom (dans un fil à part) ; `fresh` les refait toutes.
pub fn run_index(dir: &Path, state: &Arc<Mutex<IndexState>>, fresh: bool) {
    let games = find_games(dir);
    let known = if fresh { HashMap::new() } else { load_index() };
    {
        let mut s = state.lock().unwrap();
        s.total = games.len();
        s.stats = known.iter().filter(|(label, stat)| games.iter().any(|g| &g.label == *label && stat.stamp == stamp_of(&g.rom))).map(|(l, s)| (l.clone(), s.clone())).collect();
        s.done = s.stats.len();
        s.revision += 1;
    }
    for game in &games {
        let fresh_enough = state.lock().unwrap().stats.contains_key(&game.label);
        if fresh_enough {
            continue;
        }
        let stats = game_stats(&game.rom);
        let mut s = state.lock().unwrap();
        s.stats.insert(game.label.clone(), stats);
        s.done += 1;
        s.revision += 1;
        save_index(&s.stats);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Une rom avec NitroFS : `rom.bin` (un fichier en copie, un en données brutes) et une palette.
    fn rom_with_files(name: &str) -> PathBuf {
        let mut rom_bin: Vec<u8> = [12u32, 20, 28].iter().flat_map(|v| v.to_le_bytes()).collect();
        rom_bin.extend([0x00, 4, 0, 0, b'B', b'M', b'D', b'0']);
        rom_bin.extend([0x60, 16, 0, 0, 1, 2, 3, 4]);
        let mut rom = nds_nitro::nitrofs::tests_rom(b"RLCN....", &rom_bin);
        rom[0x15C..0x15E].copy_from_slice(&0xCF56u16.to_le_bytes());
        let dir = std::env::temp_dir().join(format!("ndsgs-scan-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("jeu.nds");
        std::fs::write(&path, rom).unwrap();
        path
    }

    #[test]
    fn raw_files_are_known_by_their_extension() {
        assert_eq!(by_extension(Kind::Other, "data/icon.NBFC"), Kind::RawTiles);
        assert_eq!(by_extension(Kind::Other, "a.nbfp"), Kind::RawPalette);
        assert_eq!(by_extension(Kind::Other, "a.xyz"), Kind::Other);
        assert_eq!(by_extension(Kind::Other, "a.txt"), Kind::PlainText);
        assert_eq!(by_extension(Kind::Model, "a.nbfc"), Kind::Model);
    }

    #[test]
    fn splits_tables_of_positions() {
        let mut bytes: Vec<u8> = [8u32, 12].iter().flat_map(|v| v.to_le_bytes()).collect();
        bytes.extend(b"AAAABB");
        assert_eq!(table_members(&bytes), [(8, 12), (12, 14)]);
        assert!(table_members(b"pas une table").is_empty());
        assert!(table_members(&[0u8; 16]).is_empty());
    }

    #[test]
    fn reads_all_parts_of_a_mesh() {
        // Battle of Giants : plusieurs morceaux, sans palette
        let long = b"HSEM\x02\0\0\0\x0E\0\0\x003d/geometry/a\0\x03\0\0\x00t/a\0\0\x03\0\xFF\x7F\x0E\0\0\x003d/geometry/b\0\x03\0\0\x00t/b\0\0";
        let parts = mesh_parts(long);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1], ("3d/geometry/b".to_string(), "t/b".to_string(), None));
        // Rayman : compte sur un octet, palette nommée
        let short = b"HSEM\x01\x0D\0\0\x003d/geometry/a\x03\0\0\x00t/a\x0C\0\0\x003d/palette/p";
        assert_eq!(mesh_parts(short), [("3d/geometry/a".to_string(), "t/a".to_string(), Some("3d/palette/p".to_string()))]);
    }

    #[test]
    fn reads_video_headers() {
        let mut b = vec![0u8; 0x24];
        b[0..4].copy_from_slice(b"BIKi");
        b[8] = 100;
        b[0x15] = 1;
        b[0x18] = 192;
        b[0x1C] = 30;
        b[0x20] = 1;
        assert!(video_lines(&b)[0].contains("256x192, 100 frames, 30.0"));
    }

    #[test]
    fn reads_engine_texts_and_texture_links() {
        let mut b = b"GNAL\x01\0\0\0\0\0\0\0\x01\0\0\0\x02\0\0\0Hi".to_vec();
        assert_eq!(lang_lines(&b), ["   0  Hi"]);
        b = b"TXET\x03\0\0\0a/b\x04\0\0\0c/d\0".to_vec();
        assert_eq!(texture_ref(&b), Some(("a/b".to_string(), "c/d".to_string())));
    }

    #[test]
    fn reads_bmg_messages() {
        let mut b = vec![0u8; 0x20];
        b[0..8].copy_from_slice(b"MESGbmg1");
        b[0x10] = 2;
        b.extend(b"INF1\x18\0\0\0\x01\0\x04\0\0\0\0\0\x02\0\0\0\0\0\0\0");
        b.extend(b"DAT1\x10\0\0\0\0\0H\0i\0\0\0\0\0");
        assert_eq!(bmg_lines(&b), ["   0  Hi"]);
    }

    #[test]
    fn finds_nitro_files_stuck_in_a_file() {
        let mut bytes = vec![0u8; 8];
        bytes.extend(b"RLCN\xFF\xFE\x00\x01\x14\x00\x00\x00\x10\x00\x01\x00");
        bytes.extend([0u8; 4]);
        bytes.extend(b"RLCN sans en-tete valide........");
        // Un début de fichier compressé : la taille tient, pas la taille de l'en-tête
        bytes.extend(b"RGCN\xFF\xFE\x01\x01\x08\x00\x00\x00\x01\x00RAHC");
        assert_eq!(embedded_files(&bytes), [(8, 28, "nclr")]);
    }

    #[test]
    fn shows_text_and_bytes_of_unknown_files() {
        let mut bytes = b"\x00\x01boss_broly.ima\x00".to_vec();
        bytes.extend([0x83, 0x75, 0x83, 0x8D, 0x83, 0x8A, 0x00]);
        let lines = text_lines(&bytes);
        assert_eq!(lines[1], "boss_broly.ima");
        assert_eq!(lines[2], "\u{30d6}\u{30ed}\u{30ea}");
        assert!(lines.last().unwrap().starts_with("000010  "));
        // Surtout du binaire : les octets d'abord, et des kanji seuls ne sont pas du texte
        let mut noise = vec![0xFFu8; 64];
        noise.extend([0x8A, 0xA3, 0x8E, 0x94, 0x00]);
        let lines = text_lines(&noise);
        assert!(lines[0].starts_with("Bytes"));
        assert!(lines.iter().any(|l| l == "Text found: 0"));
    }

    #[test]
    fn titles_fit_the_font() {
        assert_eq!(plain_title("Madagascar™", "MADAGASCAR"), "Madagascar");
        assert_eq!(plain_title("Pokémon Dash", "POKEMON DASH"), "Pokemon Dash");
        assert_eq!(plain_title("ゲームででましたっ！", "POWERPUFF"), "POWERPUFF");
    }

    #[test]
    fn lists_and_reads_files_from_the_rom() {
        let path = rom_with_files("list");
        let entries = scan(&path);
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["data/a.bin", "rom.bin", "rom.bin#0", "rom.bin#1"]);
        assert_eq!(entries[0].kind, Kind::Palette);
        assert_eq!(read_entry(&entries[0]).unwrap(), b"RLCN....");
        // rom.bin#0 : dépaqueté, reconnu par sa signature
        assert_eq!(entries[2].kind, Kind::Model);
        assert_eq!(read_entry(&entries[2]).unwrap(), b"BMD0");
        assert_eq!(entries[2].format(), "nsbmd");
        // rom.bin#1 : ressemble seulement à un fichier empaqueté
        assert_eq!(read_entry(&entries[3]).unwrap(), [0x60, 16, 0, 0, 1, 2, 3, 4]);
        assert_eq!(entries[3].format(), "bin");
    }

    #[test]
    fn the_stats_count_rom_bin() {
        let stats = game_stats(&rom_with_files("stats"));
        // data/a.bin (8 octets) et rom.bin (28 octets), lisibles tous les deux
        assert_eq!(stats.data_bytes, 36);
        assert_eq!(stats.extracted_bytes, 36);
    }

    #[test]
    fn a_broken_rom_gives_nothing() {
        let dir = std::env::temp_dir().join(format!("ndsgs-scan-broken-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("casse.nds");
        std::fs::write(&path, [0u8; 8]).unwrap();
        assert!(scan(&path).is_empty());
        assert!(scan(&dir.join("absente.nds")).is_empty());
    }
}
