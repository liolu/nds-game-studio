//! La bibliothèque du launcher, sans interface : les roms du dossier `rom` (à côté du launcher),
//! ce qu'on lit dans leur en-tête (nom, éditeur, icône), l'ajout d'une rom et les préférences.
//!
//! Les dossiers de jeux sont de vrais sous-dossiers de `rom` : une rom y est désignée par son chemin relatif
//! (`Nintendo/jeu.nds`, avec des `/` partout), et les favoris d'une rom à la racine restent valables.
//!
//! Les roms ne sont jamais émulées : elles identifient le jeu (code `AQIE`...) pour trouver son module
//! natif, et donnent ses fichiers à la fenêtre Assets (F6) et au runtime.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::i18n::{t, tr};

const HEADER_SIZE: usize = 0x200;
/// CRC du logo Nintendo, le même dans toutes les roms DS.
const LOGO_CRC: u16 = 0xCF56;
const BANNER_SIZE: usize = 0x840;
const ICON_SIZE: usize = 32;
/// Langues du titre de la bannière, par ordre de préférence : français, anglais, puis les autres.
const TITLE_LANGUAGES: [usize; 6] = [2, 1, 3, 5, 4, 0];

#[derive(Clone, Debug)]
pub struct RomInfo {
    /// Chemin relatif au dossier des roms (voir plus haut).
    pub file: String,
    pub path: PathBuf,
    pub title: String,
    pub publisher: String,
    /// Code du jeu (`AQIE`) : les 3 premières lettres le nomment, la dernière donne la région.
    pub code: String,
    pub internal_name: String,
    pub size: u64,
    /// Icône 32x32 de la bannière, en RGBA. Vide si la rom n'a pas de bannière.
    pub icon: Vec<u8>,
}

impl RomInfo {
    pub fn region(&self) -> &'static str {
        match self.code.as_bytes().get(3) {
            Some(b'E') => "USA",
            Some(b'P') => "Europe",
            Some(b'J') => tr("Japan", "Japon"),
            Some(b'F') => "France",
            Some(b'D') => tr("Germany", "Allemagne"),
            Some(b'S') => tr("Spain", "Espagne"),
            Some(b'I') => tr("Italy", "Italie"),
            Some(b'K') => tr("Korea", "Coree"),
            _ => tr("Other", "Autre"),
        }
    }

    /// Couleur moyenne de l'icône (fond de la jaquette).
    pub fn average_color(&self) -> [f32; 3] {
        let (mut sum, mut count) = ([0u32; 3], 0u32);
        for pixel in self.icon.chunks_exact(4).filter(|p| p[3] != 0) {
            for (total, value) in sum.iter_mut().zip(pixel) {
                *total += u32::from(*value);
            }
            count += 1;
        }
        if count == 0 {
            return [0.3, 0.32, 0.38];
        }
        sum.map(|total| total as f32 / count as f32 / 255.0)
    }
}

fn le16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn ascii(bytes: &[u8]) -> String {
    bytes.iter().take_while(|b| **b != 0).map(|b| *b as char).collect::<String>().trim().to_string()
}

/// Icône de la bannière : 4x4 tuiles de 8x8 en 4 bits par pixel, palette 16 couleurs BGR555
/// (la couleur 0 est transparente).
fn decode_icon(banner: &[u8]) -> Vec<u8> {
    let (bitmap, palette) = (&banner[0x20..0x220], &banner[0x220..0x240]);
    let mut rgba = vec![0u8; ICON_SIZE * ICON_SIZE * 4];
    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            let tile = (y / 8) * 4 + x / 8;
            let byte = bitmap[tile * 32 + (y % 8) * 4 + (x % 8) / 2];
            let index = if x % 2 == 0 { byte & 0xF } else { byte >> 4 } as usize;
            if index == 0 {
                continue;
            }
            let color = le16(palette, index * 2);
            let channel = |shift: u16| (((color >> shift) & 0x1F) as u32 * 255 / 31) as u8;
            let out = (y * ICON_SIZE + x) * 4;
            rgba[out..out + 4].copy_from_slice(&[channel(0), channel(5), channel(10), 255]);
        }
    }
    rgba
}

/// Titre de la bannière : « Jeu\nSous-titre\nEditeur » (l'éditeur est toujours la dernière ligne).
fn banner_title(banner: &[u8]) -> Option<(String, String)> {
    for language in TITLE_LANGUAGES {
        let start = 0x240 + language * 0x100;
        let units: Vec<u16> = banner[start..start + 0x100].chunks_exact(2).map(|c| le16(c, 0)).take_while(|u| *u != 0).collect();
        let text = String::from_utf16_lossy(&units);
        let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        match lines.as_slice() {
            [] => continue,
            [title] => return Some((title.to_string(), String::new())),
            [title @ .., publisher] => return Some((title.join(" - "), publisher.to_string())),
        }
    }
    None
}

fn parse_rom(file: &str, path: &Path, header: &[u8], banner: Option<&[u8]>, size: u64) -> RomInfo {
    let internal_name = ascii(&header[0..12]);
    let code = ascii(&header[0x0C..0x10]);
    let (title, publisher) = banner.and_then(banner_title).unwrap_or_else(|| (internal_name.clone(), String::new()));
    RomInfo { file: file.to_string(), path: path.to_path_buf(), title, publisher, code, internal_name, size, icon: banner.map(decode_icon).unwrap_or_default() }
}

/// Le titre, l'éditeur et l'icône d'une rom déjà en mémoire (`.srl` dans une autre rom).
pub fn read_rom_bytes(file: &str, bytes: &[u8]) -> Result<RomInfo, String> {
    if bytes.len() < HEADER_SIZE || le16(bytes, 0x15C) != LOGO_CRC {
        return Err(t!("{file}: not a DS rom", "{file} : pas une rom DS"));
    }
    let banner_at = le32(bytes, 0x68) as usize;
    let banner = (banner_at != 0).then(|| bytes.get(banner_at..banner_at + BANNER_SIZE)).flatten();
    Ok(parse_rom(file, Path::new(file), &bytes[..HEADER_SIZE], banner, bytes.len() as u64))
}

/// Lit l'en-tête d'une rom DS (le reste du fichier n'est pas lu). Erreur si ce n'est pas une rom.
pub fn read_rom(path: &Path) -> Result<RomInfo, String> {
    let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut file = File::open(path).map_err(|e| format!("{file_name} : {e}"))?;
    let size = file.metadata().map_err(|e| format!("{file_name} : {e}"))?.len();
    let mut header = [0u8; HEADER_SIZE];
    file.read_exact(&mut header).map_err(|_| t!("{file_name}: not a DS rom (too small)", "{file_name} : pas une rom DS (trop petit)"))?;
    if le16(&header, 0x15C) != LOGO_CRC {
        return Err(t!("{file_name}: not a DS rom", "{file_name} : pas une rom DS"));
    }
    let banner_at = u64::from(le32(&header, 0x68));
    let mut banner = vec![0u8; BANNER_SIZE];
    let has_banner = banner_at != 0 && file.seek(SeekFrom::Start(banner_at)).is_ok() && file.read_exact(&mut banner).is_ok();
    Ok(parse_rom(&file_name, path, &header, has_banner.then_some(banner.as_slice()), size))
}

/// Le dossier `rom` à côté de l'exécutable (le launcher et le studio sont dans le même dossier).
pub fn rom_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("NDSGS_ROM_DIR") {
        return PathBuf::from(dir);
    }
    let exe = std::env::current_exe().unwrap_or_default();
    exe.parent().unwrap_or(Path::new(".")).join("rom")
}

fn has_rom_extension(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("nds") || e.eq_ignore_ascii_case("dsi"))
}

fn walk(dir: &Path, prefix: &str, stamp: &mut Vec<(String, u64)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let relative = format!("{prefix}{name}");
        // Un lien symbolique n'est pas suivi : pas de boucle possible
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            if !name.starts_with('.') {
                let folder = format!("{relative}/");
                walk(&entry.path(), &folder, stamp);
                stamp.push((folder, 0));
            }
        } else if has_rom_extension(&entry.path()) {
            stamp.push((relative, entry.metadata().map_or(0, |m| m.len())));
        }
    }
}

/// Ce qui change quand le dossier change : les roms (chemin relatif, taille) et les sous-dossiers (`nom/`, 0), triés.
pub fn folder_stamp(dir: &Path) -> Vec<(String, u64)> {
    let mut stamp = Vec::new();
    walk(dir, "", &mut stamp);
    stamp.sort();
    stamp
}

/// Les sous-dossiers, chemins relatifs sans `/` à la fin.
pub fn folders_in(stamp: &[(String, u64)]) -> Vec<String> {
    stamp.iter().filter_map(|(name, _)| name.strip_suffix('/').map(str::to_string)).collect()
}

/// Les roms du dossier et de ses sous-dossiers, triées par titre (les fichiers illisibles sont ignorés).
pub fn scan_roms(dir: &Path) -> Vec<RomInfo> {
    let mut roms: Vec<RomInfo> = folder_stamp(dir)
        .iter()
        .filter(|(name, _)| !name.ends_with('/'))
        .filter_map(|(name, _)| {
            let mut info = read_rom(&dir.join(name)).ok()?;
            info.file = name.clone();
            Some(info)
        })
        .collect();
    roms.sort_by_key(|r| r.title.to_lowercase());
    roms
}

/// Le dossier d'une rom ou d'un dossier (chemin relatif) ; vide à la racine.
pub fn parent_of(relative: &str) -> &str {
    relative.rsplit_once('/').map_or("", |(parent, _)| parent)
}

/// Le dernier élément d'un chemin relatif.
pub fn last_name(relative: &str) -> &str {
    relative.rsplit('/').next().unwrap_or(relative)
}

fn join_relative(folder: &str, name: &str) -> String {
    if folder.is_empty() { name.to_string() } else { format!("{folder}/{name}") }
}

/// Le chemin d'un sous-dossier (vide = le dossier des roms).
pub fn folder_path(dir: &Path, folder: &str) -> PathBuf {
    if folder.is_empty() { dir.to_path_buf() } else { dir.join(folder) }
}

/// Un nom qui reste un seul dossier sur tous les systèmes (les caractères interdits sous Windows sont refusés partout :
/// le dossier des roms peut changer de machine).
fn valid_folder_name(name: &str) -> bool {
    const MAX_NAME: usize = 64;
    !name.is_empty() && name.chars().count() <= MAX_NAME && !name.starts_with('.') && !name.ends_with('.') && !name.chars().any(|c| c.is_control() || "/\\:*?\"<>|".contains(c))
}

/// Crée un dossier de jeux dans `parent` ; rend son chemin relatif.
pub fn create_folder(dir: &Path, parent: &str, name: &str) -> Result<String, String> {
    let name = name.trim();
    if !valid_folder_name(name) {
        return Err(t!("Invalid folder name: {name}", "Nom de dossier invalide : {name}"));
    }
    let relative = join_relative(parent, name);
    let path = dir.join(&relative);
    if path.exists() {
        return Err(t!("{name} already exists", "{name} existe deja"));
    }
    std::fs::create_dir_all(&path).map_err(|e| format!("{name} : {e}"))?;
    Ok(relative)
}

/// Retire un dossier de jeux vide (jamais une rom).
pub fn remove_folder(dir: &Path, folder: &str) -> Result<(), String> {
    std::fs::remove_dir(dir.join(folder)).map_err(|e| format!("{folder} : {e}"))
}

/// Range une rom dans un dossier, sans jamais écraser un fichier ; rend son nouveau chemin relatif.
pub fn move_rom(dir: &Path, file: &str, folder: &str) -> Result<String, String> {
    let relative = join_relative(folder, last_name(file));
    if relative == file {
        return Ok(relative);
    }
    let target = dir.join(&relative);
    if target.exists() {
        return Err(t!("{relative} already exists", "{relative} existe deja"));
    }
    std::fs::rename(dir.join(file), &target).map_err(|e| format!("{file} : {e}"))?;
    Ok(relative)
}

/// Résultat de l'ajout d'une rom : son titre, et si elle était déjà dans le dossier.
#[derive(Debug, PartialEq)]
pub struct Imported {
    pub title: String,
    pub already: bool,
}

fn same_start(a: &Path, b: &Path) -> bool {
    let read = |path: &Path| -> Option<[u8; HEADER_SIZE]> {
        let mut bytes = [0u8; HEADER_SIZE];
        File::open(path).ok()?.read_exact(&mut bytes).ok()?;
        Some(bytes)
    };
    matches!((read(a), read(b)), (Some(x), Some(y)) if x == y)
}

/// Copie une rom dans le dossier (fichier temporaire puis renommage : jamais de rom à moitié écrite).
pub fn import_rom(source: &Path, dir: &Path) -> Result<Imported, String> {
    let info = read_rom(source)?;
    let name = source.file_name().ok_or(tr("invalid file name", "nom de fichier invalide"))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{} : {e}", dir.display()))?;
    let mut target = dir.join(name);
    // Même nom mais autre rom (autre version du jeu) : les deux sont gardées
    let mut n = 2;
    while target.exists() {
        if target.metadata().is_ok_and(|m| m.len() == info.size) && same_start(source, &target) {
            return Ok(Imported { title: info.title, already: true });
        }
        let stem = source.file_stem().unwrap_or(name).to_string_lossy();
        let extension = source.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
        target = dir.join(format!("{stem}_{n}.{extension}"));
        n += 1;
    }
    let part = target.with_extension("part");
    std::fs::copy(source, &part).map_err(|e| t!("cannot copy: {e}", "copie impossible : {e}"))?;
    std::fs::rename(&part, &target).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        t!("cannot copy: {e}", "copie impossible : {e}")
    })?;
    Ok(Imported { title: info.title, already: false })
}

/// Vue de la liste des jeux.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LibraryView {
    #[default]
    Grid,
    List,
}

/// Préférences du launcher (`launcher.toml`, à part d'options.toml : réinitialiser les options ne
/// doit pas effacer les favoris).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LibraryPrefs {
    /// Noms de fichier des roms, dans l'ordre où elles ont été ajoutées aux favoris.
    pub favorites: Vec<String>,
    pub view: LibraryView,
    /// Jaquettes des jeux installés : téléchargées depuis GameTDB et montrées à la place de l'icône.
    pub covers: bool,
}

fn prefs_path() -> PathBuf {
    crate::save::data_dir().join("launcher.toml")
}

impl LibraryPrefs {
    pub fn load() -> Self {
        std::fs::read_to_string(prefs_path()).ok().and_then(|text| toml::from_str(&text).ok()).unwrap_or_default()
    }

    pub fn store(&self) {
        let text = toml::to_string_pretty(self).unwrap_or_default();
        if let Err(e) = crate::save::write_atomic(&prefs_path(), &text) {
            bevy::log::warn!("launcher.toml non enregistre : {e}");
        }
    }

    pub fn is_favorite(&self, file: &str) -> bool {
        self.favorites.iter().any(|f| f == file)
    }

    pub fn toggle_favorite(&mut self, file: &str) {
        if self.is_favorite(file) {
            self.favorites.retain(|f| f != file);
        } else {
            self.favorites.push(file.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Une petite rom : en-tête + bannière (titre anglais et français, icône).
    fn fake_rom(title_fr: &str) -> Vec<u8> {
        let mut rom = vec![0u8; 0x1000];
        rom[0..4].copy_from_slice(b"TEST");
        rom[0x0C..0x10].copy_from_slice(b"ABCP");
        rom[0x15C..0x15E].copy_from_slice(&LOGO_CRC.to_le_bytes());
        rom[0x68..0x6C].copy_from_slice(&0x400u32.to_le_bytes());
        let banner = &mut rom[0x400..0x400 + BANNER_SIZE];
        banner[0x20] = 0x21; // pixel (0,0) = couleur 1, pixel (1,0) = couleur 2
        banner[0x222..0x224].copy_from_slice(&0x001Fu16.to_le_bytes()); // couleur 1 = rouge
        banner[0x224..0x226].copy_from_slice(&0x7C00u16.to_le_bytes()); // couleur 2 = bleu
        for (language, text) in [(1, "Title\nMaker"), (2, title_fr)] {
            for (i, unit) in text.encode_utf16().enumerate() {
                let at = 0x240 + language * 0x100 + i * 2;
                banner[at..at + 2].copy_from_slice(&unit.to_le_bytes());
            }
        }
        rom
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ndsgs-library-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reads_the_banner() {
        let dir = temp_dir("banner");
        let path = dir.join("jeu.nds");
        std::fs::write(&path, fake_rom("Le Jeu\nLa Suite\nEditeur")).unwrap();
        let info = read_rom(&path).unwrap();
        assert_eq!(info.title, "Le Jeu - La Suite");
        assert_eq!(info.publisher, "Editeur");
        assert_eq!(info.code, "ABCP");
        assert_eq!(info.region(), "Europe");
        assert_eq!(info.internal_name, "TEST");
        assert_eq!(&info.icon[..8], &[255, 0, 0, 255, 0, 0, 255, 255]);
        assert_eq!(info.icon[8 + 3], 0, "couleur 0 transparente");
    }

    #[test]
    fn refuses_what_is_not_a_rom() {
        let dir = temp_dir("refuse");
        let path = dir.join("photo.nds");
        std::fs::write(&path, vec![1u8; 0x400]).unwrap();
        assert!(read_rom(&path).is_err());
        std::fs::write(&path, [0u8; 4]).unwrap();
        assert!(read_rom(&path).is_err());
    }

    #[test]
    fn import_copies_once_and_keeps_other_versions() {
        let source_dir = temp_dir("import-source");
        let dir = temp_dir("import-target").join("rom");
        let source = source_dir.join("jeu.nds");
        std::fs::write(&source, fake_rom("Le Jeu\nEditeur")).unwrap();
        assert_eq!(import_rom(&source, &dir).unwrap(), Imported { title: "Le Jeu".into(), already: false });
        assert_eq!(import_rom(&source, &dir).unwrap(), Imported { title: "Le Jeu".into(), already: true });
        // Même nom, autre contenu : gardée à côté
        let mut other = fake_rom("Autre\nEditeur");
        other[0x0F] = b'E';
        std::fs::write(&source, other).unwrap();
        assert!(!import_rom(&source, &dir).unwrap().already);
        let names: Vec<String> = folder_stamp(&dir).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["jeu.nds", "jeu_2.nds"]);
        assert_eq!(scan_roms(&dir)[0].title, "Autre");
    }

    #[test]
    fn roms_in_subfolders() {
        let dir = temp_dir("folders");
        std::fs::write(dir.join("a.nds"), fake_rom("A\nEditeur")).unwrap();
        assert_eq!(create_folder(&dir, "", "  Nintendo ").unwrap(), "Nintendo");
        assert_eq!(create_folder(&dir, "Nintendo", "Mario").unwrap(), "Nintendo/Mario");
        assert!(create_folder(&dir, "", "Nintendo").is_err(), "existe deja");
        for name in ["", "  ", "..", ".cache", "a/b", "a\\b", "a:b", "fin."] {
            assert!(create_folder(&dir, "", name).is_err(), "{name}");
        }
        std::fs::write(dir.join("Nintendo/Mario/b.nds"), fake_rom("B\nEditeur")).unwrap();
        std::fs::write(dir.join("Nintendo/photo.txt"), "pas une rom").unwrap();

        let stamp = folder_stamp(&dir);
        assert_eq!(folders_in(&stamp), ["Nintendo", "Nintendo/Mario"]);
        let files: Vec<String> = scan_roms(&dir).into_iter().map(|r| r.file).collect();
        assert_eq!(files, ["a.nds", "Nintendo/Mario/b.nds"]);
        assert_eq!(parent_of("Nintendo/Mario/b.nds"), "Nintendo/Mario");
        assert_eq!(parent_of("a.nds"), "");
        assert_eq!(last_name("Nintendo/Mario"), "Mario");

        assert_eq!(move_rom(&dir, "a.nds", "Nintendo").unwrap(), "Nintendo/a.nds");
        assert!(dir.join("Nintendo/a.nds").exists() && !dir.join("a.nds").exists());
        assert_eq!(move_rom(&dir, "Nintendo/a.nds", "Nintendo").unwrap(), "Nintendo/a.nds", "deja rangee");
        // Même nom ailleurs : refusé, rien n'est écrasé
        std::fs::write(dir.join("a.nds"), fake_rom("Autre\nEditeur")).unwrap();
        assert!(move_rom(&dir, "a.nds", "Nintendo").is_err());
        assert_eq!(read_rom(&dir.join("Nintendo/a.nds")).unwrap().title, "A");
        assert_eq!(move_rom(&dir, "Nintendo/Mario/b.nds", "").unwrap(), "b.nds");

        assert!(remove_folder(&dir, "Nintendo").is_err(), "pas vide");
        remove_folder(&dir, "Nintendo/Mario").unwrap();
        assert!(!dir.join("Nintendo/Mario").exists());
    }

    #[test]
    fn prefs_round_trip_and_favorites() {
        let mut prefs = LibraryPrefs { favorites: vec![], view: LibraryView::List, covers: true };
        prefs.toggle_favorite("a.nds");
        prefs.toggle_favorite("b.nds");
        prefs.toggle_favorite("a.nds");
        assert_eq!(prefs.favorites, ["b.nds"]);
        let text = toml::to_string_pretty(&prefs).unwrap();
        assert_eq!(toml::from_str::<LibraryPrefs>(&text).unwrap(), prefs);
        assert_eq!(toml::from_str::<LibraryPrefs>("").unwrap(), LibraryPrefs::default());
    }
}
