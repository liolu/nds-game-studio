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

use nds_nitro::{g2d, model3d, narc, nitrofs, romarc, sniff, unpack, Kind, Rgba};
use serde::{Deserialize, Serialize};

use crate::i18n::{t, tr};

/// À changer quand le calcul change : l'index gardé est alors refait.
const INDEX_VERSION: u32 = 2;
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
    if kind == Kind::Other && matches!(head.first(), Some(0x10 | 0x11)) {
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
        let kind = kind_of(&data[..size.min(4)], || small.then(|| data.to_vec()));
        let dir = path.rsplit_once('/').map_or(String::new(), |(d, _)| d.to_string());
        let stem = stem_of(&path);
        entries.push(Entry { path: path.clone(), member_name: None, rom: rom_path.to_path_buf(), span, member: None, kind, size, dir, stem, rom_index: None });
        if path.rsplit('/').next().is_some_and(|n| n.eq_ignore_ascii_case("rom.bin")) {
            rom_bin = scan_rom_bin(rom_path, data, file.start);
        }
        if kind != Kind::Archive {
            continue;
        }
        let bytes = unpack(data.to_vec());
        let Ok(members) = narc::read(&bytes) else { continue };
        for m in members {
            let data = &bytes[m.start..m.end];
            let kind = kind_of(data, || Some(data.to_vec()));
            let member_path = format!("{path}#{}", m.index);
            let named = !m.name.chars().all(|c| c.is_ascii_digit());
            let stem = if named { stem_of(&m.name) } else { String::new() };
            entries.push(Entry { path: member_path, member_name: named.then(|| m.name.clone()), rom: rom_path.to_path_buf(), span, member: Some((m.start, m.end)), kind, size: m.end - m.start, dir: path.clone(), stem, rom_index: None });
        }
    }
    entries.extend(rom_bin);
    entries
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
                // Sans palette : niveaux de gris
                None => g2d::Palette { colors: (0..256).map(|i| if tiles.bpp8 { [i as u8, i as u8, i as u8, 255] } else { let v = (i % 16 * 17) as u8; [v, v, v, 255] }).collect() },
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
        _ => Err(tr("unsupported format", "format non pris en charge").into()),
    }
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
