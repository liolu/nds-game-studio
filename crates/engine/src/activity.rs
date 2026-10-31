//! Le journal de l'app : les événements qui comptent (jeu prêt, erreur, nouvelle version, décompilation qui
//! avance...), gardés dans `activity.jsonl` du dossier des données et montrés par le launcher. Les textes sont
//! faits à l'affichage, pour suivre la langue.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::i18n::{t, tr};

/// Événements gardés sur le disque, et écart de points de décompilation qui mérite une ligne.
const KEPT: usize = 500;
const BIG_PROGRESS: f32 = 1.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum What {
    /// Le programme natif du jeu vient d'être généré depuis sa rom.
    Ready { game: String, seconds: u64 },
    PrepareFailed { game: String, error: String },
    /// Le jeu natif s'est arrêté tout seul, ou n'a pas démarré.
    GameStopped { game: String, error: String },
    /// Le pourcentage de code décompilé a changé avec une nouvelle version de l'app.
    Decompiled { game: String, from: f32, to: f32 },
    Installed { version: String },
    UpdateFound { version: String },
    Covers { found: usize, total: usize },
    RomAdded { title: String },
    RomRemoved { file: String },
    RomFailed { error: String },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Good,
    Error,
    Update,
    Info,
}

fn duration(seconds: u64) -> String {
    if seconds < 60 { format!("{seconds} s") } else { format!("{} min {:02} s", seconds / 60, seconds % 60) }
}

impl What {
    pub fn level(&self) -> Level {
        match self {
            What::Ready { .. } | What::Decompiled { .. } => Level::Good,
            What::PrepareFailed { .. } | What::GameStopped { .. } | What::RomFailed { .. } => Level::Error,
            What::Installed { .. } | What::UpdateFound { .. } => Level::Update,
            What::Covers { .. } | What::RomAdded { .. } | What::RomRemoved { .. } => Level::Info,
        }
    }

    /// Le texte de la ligne ; `title` donne le nom d'un jeu d'après son code.
    pub fn text(&self, title: &dyn Fn(&str) -> String) -> String {
        match self {
            What::Ready { game, seconds } => t!("{}: ready to play (prepared in {})", "{} : pret a jouer (prepare en {})", title(game), duration(*seconds)),
            What::PrepareFailed { game, error } => t!("{}: could not be prepared: {error}", "{} : preparation impossible : {error}", title(game)),
            What::GameStopped { game, error } => t!("{}: the game stopped: {error}", "{} : le jeu s'est arrete : {error}", title(game)),
            What::Decompiled { game, to, .. } if *to >= 100.0 => t!("{}: fully decompiled", "{} : entierement decompile", title(game)),
            What::Decompiled { game, from, to } => t!("{}: decompiled code {from:.1} % -> {to:.1} %", "{} : code decompile {from:.1} % -> {to:.1} %", title(game)),
            What::Installed { version } => t!("Version {version} installed", "Version {version} installee"),
            What::UpdateFound { version } => t!("New version available: {version}", "Nouvelle version disponible : {version}"),
            What::Covers { found, total } => t!("Covers downloaded: {found} of {total}", "Jaquettes telechargees : {found} sur {total}"),
            What::RomAdded { title } => t!("{title} added", "{title} ajoute"),
            What::RomRemoved { file } => t!("{file} removed", "{file} retire"),
            What::RomFailed { error } => t!("Rom not added: {error}", "Rom non ajoutee : {error}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub time: u64,
    #[serde(flatten)]
    pub what: What,
}

/// Le journal lu au premier besoin ; `REVISION` change à chaque événement.
static LOG: Mutex<Option<Vec<Event>>> = Mutex::new(None);
static REVISION: AtomicU64 = AtomicU64::new(0);

fn log_path() -> PathBuf {
    crate::save::data_dir().join("activity.jsonl")
}

/// Les lignes illisibles (écrites par une autre version) sont ignorées.
fn parse_log(text: &str) -> Vec<Event> {
    text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect()
}

fn load_log() -> Vec<Event> {
    let mut events = parse_log(&std::fs::read_to_string(log_path()).unwrap_or_default());
    if events.len() > KEPT {
        events.drain(..events.len() - KEPT);
        let text: String = events.iter().filter_map(|e| serde_json::to_string(e).ok()).map(|line| line + "\n").collect();
        let _ = crate::save::write_atomic(&log_path(), &text);
    }
    events
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Ajoute un événement (depuis n'importe quel fil).
pub fn record(what: What) {
    let event = Event { time: now_secs(), what };
    if let Ok(line) = serde_json::to_string(&event) {
        let path = log_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let file = std::fs::OpenOptions::new().create(true).append(true).open(&path);
        if let Err(e) = file.and_then(|mut f| writeln!(f, "{line}")) {
            bevy::log::warn!("journal d'activite impossible a ecrire : {}: {e}", path.display());
        }
    }
    LOG.lock().unwrap().get_or_insert_with(load_log).push(event);
    REVISION.fetch_add(1, Ordering::SeqCst);
}

pub fn revision() -> u64 {
    REVISION.load(Ordering::SeqCst)
}

/// Les derniers événements, du plus récent au plus ancien.
pub fn latest(count: usize) -> Vec<Event> {
    LOG.lock().unwrap().get_or_insert_with(load_log).iter().rev().take(count).cloned().collect()
}

/// « il y a 5 min » : l'heure de la ligne par rapport à maintenant.
pub fn ago(time: u64, now: u64) -> String {
    let seconds = now.saturating_sub(time);
    match seconds {
        0..60 => tr("just now", "a l'instant").to_string(),
        60..3600 => t!("{} min ago", "il y a {} min", seconds / 60),
        3600..86400 => t!("{} h ago", "il y a {} h", seconds / 3600),
        _ => t!("{} d ago", "il y a {} j", seconds / 86400),
    }
}

// ─────────────────────────────────────────────────────────────────────────
//  Ce qui a changé depuis le dernier lancement
// ─────────────────────────────────────────────────────────────────────────

/// Ce qu'on a vu au dernier lancement (`activity_state.json`).
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Seen {
    version: String,
    offered: String,
    code: BTreeMap<String, f32>,
}

fn seen_path() -> PathBuf {
    crate::save::data_dir().join("activity_state.json")
}

fn load_seen() -> Seen {
    std::fs::read_to_string(seen_path()).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

fn store_seen(seen: &Seen) {
    if let Ok(text) = serde_json::to_string_pretty(seen) {
        if let Some(dir) = seen_path().parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = crate::save::write_atomic(&seen_path(), &text);
    }
}

/// À chaque lancement : une version différente de la dernière fois donne une ligne.
pub fn version_started(version: &str) {
    let mut seen = load_seen();
    if seen.version != version {
        seen.version = version.to_string();
        store_seen(&seen);
        record(What::Installed { version: version.to_string() });
    }
}

/// Une mise à jour proposée n'a sa ligne qu'une fois.
pub fn update_found(version: &str) {
    let mut seen = load_seen();
    if seen.offered != version {
        seen.offered = version.to_string();
        store_seen(&seen);
        record(What::UpdateFound { version: version.to_string() });
    }
}

/// Les changements à retenir entre l'ancien pourcentage de chaque jeu et le nouveau.
fn progress_changes(seen: &BTreeMap<String, f32>, games: &[(String, f32)]) -> Vec<What> {
    games
        .iter()
        .filter_map(|(game, to)| {
            let from = *seen.get(game)?;
            ((*to - from).abs() >= BIG_PROGRESS).then(|| What::Decompiled { game: game.clone(), from, to: *to })
        })
        .collect()
}

/// Compare la décompilation des jeux installés avec la dernière fois ; un nouveau jeu est seulement retenu.
pub fn compare_progress(games: &[(String, f32)]) {
    let mut seen = load_seen();
    let changes = progress_changes(&seen.code, games);
    let unchanged = games.iter().all(|(game, to)| seen.code.get(game).is_some_and(|from| (*to - *from).abs() < BIG_PROGRESS));
    if unchanged {
        return;
    }
    for (game, code) in games {
        seen.code.insert(game.clone(), *code);
    }
    store_seen(&seen);
    for change in changes {
        record(change);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn une_ligne_illisible_ne_perd_pas_le_reste() {
        let good = serde_json::to_string(&Event { time: 5, what: What::Ready { game: "AJTP".into(), seconds: 75 } }).unwrap();
        let events = parse_log(&format!("{good}\n{{\"time\":6,\"kind\":\"plus_tard\"}}\npas du json\n{good}\n"));
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].what, What::Ready { game: "AJTP".into(), seconds: 75 });
    }

    #[test]
    fn seuls_les_gros_changements_de_jeux_connus_comptent() {
        let seen = BTreeMap::from([("A".to_string(), 10.0), ("B".to_string(), 10.0)]);
        let games = [("A".to_string(), 10.5), ("B".to_string(), 12.0), ("C".to_string(), 90.0)];
        assert_eq!(progress_changes(&seen, &games), [What::Decompiled { game: "B".into(), from: 10.0, to: 12.0 }]);
    }

    #[test]
    fn les_textes_donnent_le_nom_du_jeu_et_la_duree() {
        let text = What::Ready { game: "AJTP".into(), seconds: 75 }.text(&|_| "Tom".to_string());
        assert!(text.starts_with("Tom") && text.contains("1 min 15 s"), "{text}");
        assert_eq!(What::Decompiled { game: "A".into(), from: 99.0, to: 100.0 }.level(), Level::Good);
    }
}
