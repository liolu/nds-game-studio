//! Sauvegardes des jeux, comme la mémoire de sauvegarde d'une cartouche DS, et dossier des données
//! du studio.
//!
//! Un jeu enregistre ce qu'il veut (n'importe quel type serde) sous un nom d'emplacement :
//!
//! ```ignore
//! fn finir(saves: Res<GameSaves>, score: Res<Score>) {
//!     saves.save("record", &score.best).ok();
//! }
//! fn demarrer(saves: Res<GameSaves>, mut score: ResMut<Score>) {
//!     score.best = saves.load("record").unwrap_or(0);
//! }
//! ```
//!
//! Les fichiers vont dans `<données>/saves/<id du jeu>/<emplacement>.json` ; le jeu en cours est
//! choisi par le studio. Écriture sûre : fichier temporaire puis renommage, et l'ancienne version
//! est gardée en `.bak` (relue si le fichier principal est abîmé).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use bevy::prelude::*;
use serde::de::DeserializeOwned;
use serde::Serialize;

/// Dossier des données du studio : `NDSGS_DATA_DIR` s'il est défini (tests), sinon
/// `%APPDATA%\Nds Game Studio` (Windows), `~/.local/share/Nds Game Studio` (Linux),
/// `~/Library/Application Support/Nds Game Studio` (macOS).
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("NDSGS_DATA_DIR") {
        return PathBuf::from(dir);
    }
    dirs::data_dir()
        .map(|d| d.join("Nds Game Studio"))
        .or_else(|| std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.join("data"))))
        .unwrap_or_else(|| PathBuf::from("data"))
}

pub fn saves_dir() -> PathBuf {
    data_dir().join("saves")
}

/// Nom de fichier sûr (lettres, chiffres, `-`, `_`).
fn clean(name: &str) -> String {
    let cleaned: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    if cleaned.is_empty() { "save".into() } else { cleaned }
}

/// Chemin du fichier d'un emplacement.
pub fn save_path(game: &str, slot: &str) -> PathBuf {
    saves_dir().join(clean(game)).join(format!("{}.json", clean(slot)))
}

/// Écrit `text` dans `path` sans jamais laisser un fichier à moitié écrit ; garde l'ancien en `.bak`.
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    write_atomic_bytes(path, text.as_bytes())
}

pub fn write_atomic_bytes(path: &Path, text: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    if path.exists() {
        let _ = std::fs::copy(path, path.with_extension("bak"));
        std::fs::remove_file(path)?;
    }
    std::fs::rename(&tmp, path)
}

fn read_value<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let parse = |p: &Path| std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str(&t).ok());
    parse(path).or_else(|| {
        let value = parse(&path.with_extension("bak"));
        if value.is_some() {
            warn!("sauvegarde abimee, ancienne version relue : {}", path.display());
        }
        value
    })
}

/// Sauvegardes du jeu en cours (`None` dans le menu du studio).
#[derive(Resource, Default, Debug, Clone)]
pub struct GameSaves {
    pub game: Option<String>,
}

impl GameSaves {
    pub fn for_game(game: &str) -> Self {
        Self { game: Some(game.to_string()) }
    }

    fn path(&self, slot: &str) -> Result<PathBuf, String> {
        let game = self.game.as_deref().ok_or("aucun jeu en cours")?;
        Ok(save_path(game, slot))
    }

    /// Enregistre `value` dans l'emplacement `slot` du jeu en cours.
    pub fn save<T: Serialize>(&self, slot: &str, value: &T) -> Result<(), String> {
        let path = self.path(slot)?;
        let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
        write_atomic(&path, &text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Relit l'emplacement `slot` (`None` s'il n'existe pas ou ne correspond plus au type).
    pub fn load<T: DeserializeOwned>(&self, slot: &str) -> Option<T> {
        read_value(&self.path(slot).ok()?)
    }

    pub fn exists(&self, slot: &str) -> bool {
        self.path(slot).is_ok_and(|p| p.exists())
    }

    pub fn delete(&self, slot: &str) {
        if let Ok(path) = self.path(slot) {
            delete_file(&path);
        }
    }
}

fn delete_file(path: &Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(path.with_extension("bak"));
}

/// Une sauvegarde sur le disque (page Options > Sauvegardes).
#[derive(Debug, Clone, PartialEq)]
pub struct SaveInfo {
    pub game: String,
    pub slot: String,
    pub path: PathBuf,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

/// Toutes les sauvegardes de tous les jeux, triées par jeu puis par emplacement.
pub fn list_saves() -> Vec<SaveInfo> {
    let mut out = Vec::new();
    let Ok(games) = std::fs::read_dir(saves_dir()) else { return out };
    for game in games.flatten().filter(|e| e.path().is_dir()) {
        let Ok(files) = std::fs::read_dir(game.path()) else { continue };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let meta = file.metadata().ok();
            out.push(SaveInfo {
                game: game.file_name().to_string_lossy().into_owned(),
                slot: path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                size: meta.as_ref().map_or(0, |m| m.len()),
                modified: meta.and_then(|m| m.modified().ok()),
                path,
            });
        }
    }
    out.sort_by(|a, b| (&a.game, &a.slot).cmp(&(&b.game, &b.slot)));
    out
}

pub fn delete_save(game: &str, slot: &str) {
    delete_file(&save_path(game, slot));
}

pub struct SavePlugin;

impl Plugin for SavePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GameSaves>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Un seul test touche au disque (variable d'environnement partagée entre les tests).
    #[test]
    fn saves_round_trip_survive_a_broken_file_and_are_listed() {
        let dir = std::env::temp_dir().join(format!("ndsgs-save-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // SAFETY: seul test du crate qui lit ou écrit cette variable
        unsafe { std::env::set_var("NDSGS_DATA_DIR", &dir) };

        let saves = GameSaves::for_game("bubbles");
        assert_eq!(saves.load::<u32>("record"), None);
        saves.save("record", &12u32).unwrap();
        assert_eq!(saves.load::<u32>("record"), Some(12));
        saves.save("record", &40u32).unwrap();
        assert_eq!(saves.load::<u32>("record"), Some(40));
        // Fichier abîmé : l'ancienne version (.bak) est relue
        std::fs::write(save_path("bubbles", "record"), "{pas du json").unwrap();
        assert_eq!(saves.load::<u32>("record"), Some(12));

        GameSaves::for_game("orbit").save("vue", &(1.5f32, 0.3f32)).unwrap();
        let list = list_saves();
        assert_eq!(list.iter().map(|s| (s.game.as_str(), s.slot.as_str())).collect::<Vec<_>>(), [("bubbles", "record"), ("orbit", "vue")]);
        delete_save("orbit", "vue");
        assert!(!GameSaves::for_game("orbit").exists("vue"));
        // Pas de jeu en cours : rien n'est écrit
        assert!(GameSaves::default().save("x", &1).is_err());
        // Noms dangereux nettoyés
        assert!(save_path("../a", "b/c").starts_with(saves_dir()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
