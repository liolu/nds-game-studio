//! Les mods venus d'ailleurs, comme l'onglet Mods d'OpenRAC (`docs/port/MODS.md`, section 10) : on colle le lien d'un dépôt, le launcher y
//! trouve les mods, les montre avec ce qu'ils font et s'ils vont avec ce jeu, et on installe celui qu'on veut. Sur GitHub, git n'est pas
//! nécessaire : la dernière release, sinon l'état du dépôt. On peut aussi donner un fichier (`.zip`, `.7z`, `.rar`, `.tar.gz`...) ou un dossier.
//!
//! Un mod installé va dans `mods/<code>/paquets/<id>/` : `nds-mod.json`, `fichiers/` (mêmes chemins que l'export : PNG, OBJ, WAV...),
//! `remplacements/` (fichiers au format du jeu). Il ne contient que des données : rien n'est jamais exécuté. `paquets.json` donne l'ordre, ce qui
//! est actif et d'où vient chaque mod ; `nds-mod-tools` applique ces mods, puis vos propres changements, à la copie de la rom.
//!
//! Un dépôt : un `nds-mod.json` en haut, ou un dossier par mod avec le sien ; `nds-mods.json`, `{"format": 1, "mods": ["un", "deux"]}`, peut
//! donner ces dossiers. Une release (tag `v<version>`) peut porter `<id>-<version>.zip` et `SHA256SUMS` : l'archive est alors contrôlée.
//!
//! ```json
//! { "format": 1, "id": "my-mod", "name": "My mod", "name_fr": "Mon mod", "version": "1.0.0", "author": "...", "description": "...", "description_fr": "...", "games": ["AL7E"] }
//! ```
//!
//! Dans le dossier d'un mod, un dossier par code de jeu (`AL7E/`) garde ce qui ne vaut que pour ce jeu : seul celui du jeu est installé. Un
//! `native.json`, `{"features": ["interpolate"]}`, demande au runtime natif une fonction qu'il a déjà.
//! Le dépôt de base de l'app ([`BASE_REPOSITORY`]) est lu de la même façon, si l'option est active.
//!
//! Format 2 (ROADMAP-MODS.md) : un mod peut aussi apporter du code, une bibliothèque par machine (`"plugin"`) qui ne connaît que
//! `native/sdk/nds_mod.h`. Le joueur est prévenu avant de l'installer. Une release peut porter `<id>-<version>-<machine>.zip` et `SHA256SUMS`.
//! Au lancement, le launcher écrit `plugins.json` (ordre, réglages) ; le module charge les mods et dit dans `status.json` ce qu'il en a fait.
//!
//! ```json
//! { "format": 2, "id": "camera-libre", "version": "2.0.0", "type": "native", "api": "1.0", "games": { "AL7E": ["<sha1 de la rom>"] },
//!   "plugin": { "windows-x86": "lib/windows-x86/camera_libre.dll", "macos-arm64": "lib/macos-arm64/camera_libre.dylib" },
//!   "settings": [ { "key": "toggle", "name": "Free camera", "type": "keybind", "default": "KeyC" } ],
//!   "instruction_hooks": { "AL7E": ["0x020ace58"] }, "multiplayer": "safe", "save": "separate" }
//! ```

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::i18n::{lang, t, tr, Lang};
use crate::mods;

const PACKS: &str = "paquets";
const STATE: &str = "paquets.json";
const MANIFEST: &str = "nds-mod.json";
const MANIFEST_LIST: &str = "nds-mods.json";
/// Le manifeste d'OpenRAC : montré pour dire que ces mods ne vont pas ici.
const OPENRAC_MANIFEST: &str = "openrac-mod.json";
const PREVIEW_DIR: &str = ".apercu";
const FILES: &str = "fichiers";
const DROP_INS: &str = "remplacements";
const TEMP: &str = ".temporaire";
/// Au-delà, ce n'est plus un mod : une archive piégée qui remplirait le disque.
const MAX_UNPACKED: u64 = 4 << 30;
const MAX_DEPTH: usize = 4;
const NATIVE: &str = "native.json";
/// L'adresse de l'hôte pour une partie en réseau.
const HOST_FILE: &str = "hote.txt";
const BASE_FILE: &str = "depot-de-base.txt";
/// Les mods de l'app : Options > Launcher les montre ou non dans l'onglet Moding.
pub const BASE_REPOSITORY: &str = "https://github.com/liolu/nds-game-studio-mods";
/// Le port des parties en réseau.
pub const NET_PORT: u16 = 27016;
/// Les extensions que propose le choix d'un fichier.
pub const ARCHIVES: [&str; 8] = ["zip", "7z", "rar", "tar", "gz", "tgz", "xz", "bz2"];

#[derive(Serialize, Deserialize, Default, Clone)]
struct Manifest {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    description: String,
    /// Le français, s'il existe : l'anglais sert sinon.
    #[serde(default)]
    name_fr: String,
    #[serde(default)]
    description_fr: String,
    /// Codes des jeux ; vide = tout jeu. Format 2 : aussi un objet code → SHA-1 des roms.
    #[serde(default)]
    games: Games,
    #[serde(default)]
    format: u32,
    /// `data` (par défaut), `native`, `mixed`
    #[serde(rename = "type", default)]
    kind: String,
    /// Version de `nds_mod.h`, `"1.0"`.
    #[serde(default)]
    api: String,
    /// Machine → bibliothèque, chemin dans le dossier du mod.
    #[serde(default)]
    plugin: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    settings: Vec<Setting>,
    #[serde(default)]
    presets: Vec<Preset>,
    /// Code du jeu → adresses d'instructions accrochées (le module est généré avec elles).
    #[serde(default)]
    instruction_hooks: std::collections::BTreeMap<String, Vec<String>>,
    /// `safe` : sans effet sur la partie, pas comparé en multi.
    #[serde(default)]
    multiplayer: String,
    /// `separate` : ce mod touche à la sauvegarde, elle est séparée.
    #[serde(default)]
    save: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct Entry {
    id: String,
    enabled: bool,
    /// Le lien d'où il vient, pour le mettre à jour ; vide pour un fichier ou un dossier.
    #[serde(default)]
    source: String,
    /// La release d'où il vient, s'il y en a une.
    #[serde(default)]
    tag: String,
}

#[derive(Serialize, Deserialize, Default)]
struct State {
    packs: Vec<Entry>,
}

/// Un mod installé, pour la page du jeu.
pub struct Pack {
    /// Ce mod active les parties en réseau : la page demande l'adresse de l'hôte.
    pub network: bool,
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub enabled: bool,
    pub source: String,
    /// Ce qu'il fait sur l'ordinateur (mod de code), à montrer.
    pub warning: Option<String>,
    /// Ce que le module en a fait à la dernière partie : (état, raison).
    pub status: Option<(String, Option<String>)>,
    pub has_settings: bool,
}

impl Manifest {
    /// Le nom et la description dans la langue du launcher.
    fn translate(mut self) -> Self {
        if lang() == Lang::Fr {
            if !self.name_fr.is_empty() {
                self.name = std::mem::take(&mut self.name_fr);
            }
            if !self.description_fr.is_empty() {
                self.description = std::mem::take(&mut self.description_fr);
            }
        }
        self
    }
}

fn packs_dir(mod_dir: &Path) -> PathBuf {
    mod_dir.join(PACKS)
}

fn load_state(mod_dir: &Path) -> State {
    std::fs::read(mod_dir.join(STATE)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

fn save_state(mod_dir: &Path, state: &State) -> Result<(), String> {
    let text = serde_json::to_vec_pretty(state).map_err(|e| e.to_string())?;
    std::fs::write(mod_dir.join(STATE), text).map_err(|e| e.to_string())
}

pub fn list(code: &str) -> Vec<Pack> {
    let dir = mods::mod_dir(code);
    load_state(&dir)
        .packs
        .into_iter()
        .filter(|entry| packs_dir(&dir).join(&entry.id).is_dir())
        .map(|entry| {
            let manifest = std::fs::read(packs_dir(&dir).join(&entry.id).join(MANIFEST)).ok().and_then(|bytes| serde_json::from_slice::<Manifest>(&bytes).ok()).unwrap_or_default().translate();
            let warning = warning(&manifest);
            let status = if manifest.runs_code() { last_status(&dir, &entry.id) } else { None };
            let has_settings = !manifest.settings.is_empty();
            let name = if manifest.name.is_empty() { entry.id.clone() } else { manifest.name };
            let network = pack_features(&packs_dir(&dir).join(&entry.id)).iter().any(|f| f == "network");
            Pack { network, id: entry.id, name, version: manifest.version, author: manifest.author, description: manifest.description, enabled: entry.enabled, source: entry.source, warning, status, has_settings }
        })
        .collect()
}

/// Ce que ce mod demande au runtime natif.
fn pack_features(pack: &Path) -> Vec<String> {
    #[derive(Deserialize)]
    struct Native {
        #[serde(default)]
        features: Vec<String>,
    }
    std::fs::read(pack.join(NATIVE)).ok().and_then(|bytes| serde_json::from_slice::<Native>(&bytes).ok()).map_or(Vec::new(), |n| n.features)
}

/// Les arguments du module natif d'après les mods actifs. Seules les fonctions connues passent : un mod ne choisit pas la ligne de commande.
/// « Les originaux » coupe aussi ces fonctions.
pub fn native_args(code: &str) -> Vec<OsString> {
    if !mods::this_run_has_mods(code) {
        return Vec::new();
    }
    let dir = mods::mod_dir(code);
    let mut args: Vec<OsString> = Vec::new();
    let mut features: Vec<String> = load_state(&dir).packs.iter().filter(|p| p.enabled).flat_map(|p| pack_features(&packs_dir(&dir).join(&p.id))).collect();
    features.sort();
    features.dedup();
    for feature in features {
        match feature.as_str() {
            "interpolate" => args.push("--interpolate".into()),
            "first_person" => args.push("--first-person".into()),
            "free_camera" => args.push("--free-camera".into()),
            "skip_intro" => args.push("--skip-intro".into()),
            "network" => {
                args.push("--net-port".into());
                args.push(NET_PORT.to_string().into());
                let host = host_address(code);
                if !host.is_empty() {
                    args.push("--net-join".into());
                    args.push(host.into());
                }
            }
            _ => {}
        }
    }
    args
}

/// Un mod actif demande une fonction au runtime, ou apporte du code (format 2).
pub fn has_active_features(code: &str) -> bool {
    let dir = mods::mod_dir(code);
    load_state(&dir).packs.iter().any(|p| p.enabled && (!pack_features(&packs_dir(&dir).join(&p.id)).is_empty() || pack_manifest(&dir, &p.id).runs_code()))
}

/// Les noms des mods actifs de ce jeu.
pub fn active_names(code: &str) -> Vec<String> {
    list(code).into_iter().filter(|p| p.enabled).map(|p| p.name).collect()
}

pub fn host_address(code: &str) -> String {
    std::fs::read_to_string(mods::mod_dir(code).join(HOST_FILE)).unwrap_or_default().trim().to_string()
}

/// Seuls des chiffres, des lettres, `.`, `-`, `:` : l'adresse devient un argument du module.
pub fn set_host_address(code: &str, address: &str) {
    let clean: String = address.trim().chars().filter(|c| c.is_ascii_alphanumeric() || ".-:".contains(*c)).collect();
    let dir = mods::mod_dir(code);
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join(HOST_FILE), clean);
}

static BASE_ENABLED: AtomicU8 = AtomicU8::new(2);

/// Le dépôt de base est montré dans l'onglet Moding : oui tant que l'utilisateur ne l'a pas coupé.
pub fn base_enabled() -> bool {
    match BASE_ENABLED.load(Ordering::Relaxed) {
        2 => {
            let on = std::fs::read_to_string(crate::save::data_dir().join("mods").join(BASE_FILE)).map_or(true, |text| text.trim() != "0");
            BASE_ENABLED.store(on as u8, Ordering::Relaxed);
            on
        }
        value => value == 1,
    }
}

pub fn set_base_enabled(on: bool) {
    BASE_ENABLED.store(on as u8, Ordering::Relaxed);
    let dir = crate::save::data_dir().join("mods");
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join(BASE_FILE), if on { "1" } else { "0" });
}

/// Une fois par jeu depuis le démarrage : ouvrir l'onglet Moding lit le dépôt de base, s'il est actif.
pub fn offer_base(code: &str) {
    static ASKED: Mutex<Vec<String>> = Mutex::new(Vec::new());
    if !base_enabled() || mods::running() || preview(code).is_some() {
        return;
    }
    let mut asked = ASKED.lock().unwrap();
    if !asked.iter().any(|c| c == code) {
        asked.push(code.to_string());
        find(code, BASE_REPOSITORY);
    }
}

/// Un mod qui ne fait que demander une fonction au runtime n'a pas besoin de l'outil de mod.
fn has_files(pack: &Path) -> bool {
    is_structured(pack)
}

fn any_files(code: &str) -> bool {
    let dir = mods::mod_dir(code);
    load_state(&dir).packs.iter().any(|p| has_files(&packs_dir(&dir).join(&p.id))) || mods::exported(code)
}

/// Active ou coupe un mod, puis refait la copie de la rom si ce mod y change quelque chose.
pub fn set_enabled(code: &str, rom: &Path, id: &str, enabled: bool) {
    let dir = mods::mod_dir(code);
    let mut state = load_state(&dir);
    if let Some(entry) = state.packs.iter_mut().find(|p| p.id == id) {
        entry.enabled = enabled;
    }
    let _ = save_state(&dir, &state);
    if enabled {
        mods::set_on(code, true);
    }
    if has_files(&packs_dir(&dir).join(id)) {
        mods::apply(code, rom);
    }
}

/// Retire un mod installé : sa copie, jamais le fichier d'où il vient. La copie de la rom est refaite.
pub fn remove(code: &str, rom: &Path, id: &str) {
    let dir = mods::mod_dir(code);
    let mut state = load_state(&dir);
    state.packs.retain(|p| p.id != id);
    let _ = save_state(&dir, &state);
    let had_files = has_files(&packs_dir(&dir).join(id));
    let _ = std::fs::remove_dir_all(packs_dir(&dir).join(id));
    if had_files {
        mods::apply(code, rom);
    }
}

// --------------------------------------------------------------------------------------------
//  Lire un lien
// --------------------------------------------------------------------------------------------

#[derive(PartialEq, Debug)]
pub enum Link {
    /// Dépôt GitHub : son archive se télécharge sans git installé.
    GitHub { owner: String, repo: String, reference: Option<String>, folder: Option<String> },
    /// Une archive en ligne.
    Archive(String),
    /// Un autre dépôt git.
    Git(String),
}

fn clean_part(part: &str) -> bool {
    !part.is_empty() && part != "." && part != ".." && part.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

/// Comme OpenRAC : https, http, sans schéma, `.git`, `/tree/<branche>/<dossier>`, `/releases/tag/<v>`, `git@github.com:o/r`, `o/r`.
pub fn parse_link(text: &str) -> Result<Link, String> {
    let text = text.trim().trim_end_matches('/');
    let wrong = || t!("This is not a repository's address: {text}", "Ce n'est pas l'adresse d'un depot : {text}");
    // Un lien qui commence par un tiret serait lu par git comme une option ; `ext::` lancerait une commande
    if text.is_empty() || text.starts_with('-') || text.contains(char::is_whitespace) || text.contains("::") {
        return Err(wrong());
    }
    let is_archive = |path: &str| {
        let lower = path.split(['?', '#']).next().unwrap_or(path).to_lowercase();
        lower.ends_with(".tar.gz") || ARCHIVES.iter().any(|ext| lower.ends_with(&format!(".{ext}")))
    };
    let github = text.strip_prefix("git@github.com:").or_else(|| ["https://www.github.com/", "http://www.github.com/", "https://github.com/", "http://github.com/", "www.github.com/", "github.com/"].iter().find_map(|prefix| text.strip_prefix(prefix)));
    let bare = !text.contains("://") && !text.contains('@') && text.matches('/').count() == 1;
    if let Some(path) = github.or(bare.then_some(text)) {
        if is_archive(path) && path.contains("/releases/download/") {
            return Ok(Link::Archive(format!("https://github.com/{path}")));
        }
        let mut parts = path.split('/');
        let (Some(owner), Some(repo)) = (parts.next(), parts.next()) else { return Err(wrong()) };
        let repo = repo.strip_suffix(".git").unwrap_or(repo);
        if !clean_part(owner) || !clean_part(repo) {
            return Err(wrong());
        }
        let rest: Vec<&str> = parts.collect();
        let safe = |list: &[&str]| list.iter().all(|p| !p.is_empty() && *p != ".." && *p != ".");
        let (reference, folder) = match rest.as_slice() {
            [] => (None, None),
            ["tree", branch, folder @ ..] if safe(folder) && safe(&[branch]) => (Some(format!("refs/heads/{branch}")), (!folder.is_empty()).then(|| folder.join("/"))),
            ["releases", "tag", version] if safe(&[version]) => (Some(format!("refs/tags/{version}")), None),
            _ => return Err(wrong()),
        };
        return Ok(Link::GitHub { owner: owner.to_string(), repo: repo.to_string(), reference, folder });
    }
    let web = text.starts_with("https://") || text.starts_with("http://");
    if web && is_archive(text) {
        return Ok(Link::Archive(text.to_string()));
    }
    if web || (text.starts_with("git@") && text.contains(':')) {
        return Ok(Link::Git(text.to_string()));
    }
    Err(wrong())
}

// --------------------------------------------------------------------------------------------
//  Ouvrir une archive
// --------------------------------------------------------------------------------------------

/// Une entrée à la fois : seuls les vrais fichiers, à l'intérieur du dossier, et une limite de taille.
fn unzip(archive: &Path, to: &Path) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(relative) = entry.enclosed_name() else { continue };
        if entry.is_dir() || entry.is_symlink() {
            continue;
        }
        total += entry.size();
        if total > MAX_UNPACKED {
            return Err(tr("this archive is too big once opened", "cette archive est trop grosse une fois ouverte").to_string());
        }
        let target = to.join(relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut out = std::fs::File::create(&target).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn hidden(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}

/// `.zip` par nous ; le reste par le `tar` du système, qui lit aussi `.7z` et `.rar` sous Windows et macOS.
fn unpack(archive: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    let zip_error = match unzip(archive, to) {
        Ok(()) => return Ok(()),
        Err(e) => e,
    };
    let is_zip = archive.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("zip"));
    let mut command = Command::new("tar");
    command.arg("-xf").arg(archive).arg("-C").arg(to);
    hidden(&mut command);
    match command.output() {
        Ok(output) if output.status.success() => Ok(()),
        _ if is_zip => Err(zip_error),
        _ => Err(t!("This archive could not be opened. Make it a .zip, or open it and add its folder.", "Cette archive n'a pas pu etre ouverte. Refaites-la en .zip, ou ouvrez-la et ajoutez son dossier.")),
    }
}

/// Copie les vrais fichiers d'un dossier : un lien symbolique dans un dépôt pourrait viser n'importe quoi.
fn copy_tree(from: &Path, to: &Path, keep: &dyn Fn(&str) -> bool) -> Result<u64, String> {
    let mut copied = 0;
    for entry in std::fs::read_dir(from).map_err(|e| e.to_string())?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let kind = std::fs::symlink_metadata(entry.path()).map_err(|e| e.to_string())?.file_type();
        if !keep(&name) || kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            copied += copy_tree(&entry.path(), &to.join(&name), &|n| !n.starts_with(".git"))?;
        } else {
            std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
            std::fs::copy(entry.path(), to.join(&name)).map_err(|e| e.to_string())?;
            copied += 1;
        }
    }
    Ok(copied)
}

// --------------------------------------------------------------------------------------------
//  Installer
// --------------------------------------------------------------------------------------------

fn find_manifests(dir: &Path, depth: usize, names: &[&str], out: &mut Vec<PathBuf>) {
    if names.iter().any(|name| dir.join(name).is_file()) {
        out.push(dir.to_path_buf());
        return;
    }
    if depth == 0 {
        return;
    }
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let is_dir = std::fs::symlink_metadata(entry.path()).is_ok_and(|m| m.is_dir());
        if is_dir && !entry.file_name().to_string_lossy().starts_with(".git") {
            find_manifests(&entry.path(), depth - 1, names, out);
        }
    }
}

fn only_folder(dir: &Path) -> Option<PathBuf> {
    let entries: Vec<_> = std::fs::read_dir(dir).into_iter().flatten().flatten().collect();
    match entries.as_slice() {
        [only] if only.path().is_dir() => Some(only.path()),
        _ => None,
    }
}

fn is_structured(dir: &Path) -> bool {
    dir.join(FILES).is_dir() || dir.join(DROP_INS).is_dir()
}

/// Une archive contient souvent un seul dossier, qui contient `fichiers/` : on y descend. Sans `fichiers/`, on ne touche à rien : ce dossier
/// peut être un vrai chemin du jeu (`rom.bin/`), et l'outil reconnaît les chemins à leur fin.
fn structured_root(dir: &Path) -> Option<PathBuf> {
    let mut current = dir.to_path_buf();
    for _ in 0..MAX_DEPTH {
        if is_structured(&current) {
            return Some(current);
        }
        current = only_folder(&current)?;
    }
    None
}

fn clean_id(text: &str) -> String {
    let id: String = text.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    let id = id.trim_matches('-').to_string();
    if id.is_empty() { "mod".to_string() } else { id }
}

fn not_meta(name: &str) -> bool {
    let lower = name.to_lowercase();
    name != MANIFEST && name != NATIVE && !lower.starts_with("readme") && !lower.starts_with("license") && !lower.starts_with("licence") && !lower.ends_with(".md") && !lower.starts_with(".git")
}

/// Met un mod dans `paquets/<id>` sous sa forme habituelle : son dossier pour ce jeu s'il en a un, sinon tout. Rend le nombre de fichiers.
fn install_one(mod_dir: &Path, code: &str, top: &Path, id: &str) -> Result<u64, String> {
    let for_game = top.join(code);
    let from = if for_game.is_dir() { for_game.as_path() } else { top };
    let target = packs_dir(mod_dir).join(id);
    let new = packs_dir(mod_dir).join(format!("{id}{TEMP}"));
    let _ = std::fs::remove_dir_all(&new);
    std::fs::create_dir_all(&new).map_err(|e| e.to_string())?;
    let mut copied = 0;
    let manifest = read_manifest(top, MANIFEST).unwrap_or_default();
    if is_structured(from) {
        for sub in [FILES, DROP_INS] {
            if from.join(sub).is_dir() {
                copied += copy_tree(&from.join(sub), &new.join(sub), &|n| !n.starts_with(".git"))?;
            }
        }
    } else if !manifest.runs_code() {
        // un mod de code sans fichiers/ n'a pas de fichiers de jeu : ses bibliothèques et ses sources n'iraient pas dans la rom
        copied += copy_tree(from, &new.join(FILES), &not_meta)?;
    }
    if top.join(MANIFEST).is_file() {
        std::fs::copy(top.join(MANIFEST), new.join(MANIFEST)).map_err(|e| e.to_string())?;
    }
    if from.join(NATIVE).is_file() {
        std::fs::copy(from.join(NATIVE), new.join(NATIVE)).map_err(|e| e.to_string())?;
        copied += 1;
    }
    // Mod de code : seule la bibliothèque de cette machine est gardée
    if let Some(file) = manifest.plugin.get(PLATFORM).filter(|f| !f.contains("..")) {
        let source = [top.join(file), from.join(file)].into_iter().find(|p| p.is_file()).ok_or_else(|| t!("{file} is missing from this mod.", "{file} manque dans ce mod."))?;
        let target_file = new.join(file);
        if let Some(parent) = target_file.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::copy(source, target_file).map_err(|e| e.to_string())?;
        copied += 1;
        clear_quarantine(&new);
    }
    if copied == 0 {
        let _ = std::fs::remove_dir_all(&new);
        return Err(tr("this mod has no file", "ce mod n'a aucun fichier").to_string());
    }
    // L'ancienne version n'est remplacée qu'une fois la nouvelle complète
    let _ = std::fs::remove_dir_all(&target);
    std::fs::rename(&new, &target).map_err(|e| e.to_string())?;
    Ok(copied)
}

/// Installe les mods d'un dossier. Sans `nds-mod.json`, un lien doit au moins montrer `fichiers/` ou `remplacements/` ; un fichier ou un
/// dossier ajouté à la main est pris en entier, sous `fallback_name`. Rend les noms installés.
fn install_tree(mod_dir: &Path, root: &Path, code: &str, source: &str, fallback_name: &str) -> Result<Vec<String>, String> {
    let mut folders = Vec::new();
    find_manifests(root, MAX_DEPTH, &[MANIFEST], &mut folders);
    let mut found: Vec<(PathBuf, Manifest)> = Vec::new();
    let mut other_games = Vec::new();
    for folder in folders {
        let manifest: Manifest = serde_json::from_slice(&std::fs::read(folder.join(MANIFEST)).map_err(|e| e.to_string())?).map_err(|e| t!("{MANIFEST} cannot be read: {e}", "{MANIFEST} illisible : {e}"))?;
        if manifest.games.fits(code) {
            if let Some(refused) = manifest.code_refusal() {
                return Err(refused);
            }
            found.push((folder, manifest));
        } else {
            other_games.push(if manifest.name.is_empty() { manifest.id } else { manifest.name });
        }
    }
    let plain_files = found.is_empty() && other_games.is_empty();
    if plain_files {
        let structured = structured_root(root);
        if !source.is_empty() && structured.is_none() {
            return Err(t!("No mod for DS games here: {MANIFEST} is missing. Mods made for another program, like OpenRAC's, cannot be used.", "Pas de mod pour jeu DS ici : {MANIFEST} manque. Les mods faits pour un autre programme, comme ceux d'OpenRAC, ne peuvent pas servir."));
        }
        found.push((structured.unwrap_or_else(|| root.to_path_buf()), Manifest { id: fallback_name.to_string(), ..Default::default() }));
    }
    if found.is_empty() {
        return Err(t!("These mods are for other games: {}", "Ces mods sont pour d'autres jeux : {}", other_games.join(", ")));
    }
    std::fs::create_dir_all(packs_dir(mod_dir)).map_err(|e| e.to_string())?;
    let mut state = load_state(mod_dir);
    // Un dépôt de plusieurs mods ne les active pas tous d'un coup : on choisit
    let enable_new = found.len() == 1;
    let mut names = Vec::new();
    for (folder, manifest) in found {
        let id = clean_id(if manifest.id.is_empty() { &manifest.name } else { &manifest.id });
        install_one(mod_dir, code, &folder, &id)?;
        match state.packs.iter_mut().find(|p| p.id == id) {
            Some(entry) => entry.source = source.to_string(),
            None => state.packs.push(Entry { id: id.clone(), enabled: enable_new, source: source.to_string(), tag: String::new() }),
        }
        names.push(if manifest.name.is_empty() { id } else { manifest.name });
    }
    save_state(mod_dir, &state)?;
    Ok(names)
}

fn stem_of(path: &Path) -> String {
    let name = path.file_name().map_or_else(|| "mod".to_string(), |n| n.to_string_lossy().into_owned());
    let lower = name.to_lowercase();
    let cut = lower.strip_suffix(".tar.gz").map(str::len).or_else(|| name.rfind('.'));
    name[..cut.unwrap_or(name.len())].to_string()
}

fn finish(code: &str, rom: &Path, names: Vec<String>) -> Result<(), String> {
    mods::say(t!("Installed: {}", "Installe : {}", names.join(", ")));
    if !any_files(code) {
        mods::set_on(code, true);
        return Ok(());
    }
    if !mods::installed() {
        mods::say(tr("Download the mod tools to play with it.", "Telechargez les outils de mod pour jouer avec.").to_string());
        return Ok(());
    }
    mods::apply_now(code, rom)
}

fn add_local(code: &str, rom: &Path, path: &Path) -> Result<(), String> {
    let dir = mods::mod_dir(code);
    let name = stem_of(path);
    if path.is_dir() {
        return finish(code, rom, install_tree(&dir, path, code, "", &name)?);
    }
    let temp = dir.join(TEMP);
    let _ = std::fs::remove_dir_all(&temp);
    mods::say(t!("Opening {name}...", "Ouverture de {name}..."));
    let result = unpack(path, &temp).and_then(|()| install_tree(&dir, &temp, code, "", &name));
    let _ = std::fs::remove_dir_all(&temp);
    finish(code, rom, result?)
}

/// Un fichier ou un dossier, dans un fil à part.
pub fn add_path(code: &str, rom: &Path, path: &Path) {
    let (code, rom, path) = (code.to_string(), rom.to_path_buf(), path.to_path_buf());
    mods::start(Some(code.clone()), move || add_local(&code, &rom, &path).inspect_err(|e| crate::activity::record(crate::activity::What::ModFailed { game: code.clone(), error: e.clone() })));
}

fn clone_repository(url: &str, to: &Path) -> Result<(), String> {
    // Seul le dernier état suffit ; un petit serveur git ne sait pas le faire : alors tout le dépôt
    let mut last = String::new();
    for shallow in [true, false] {
        let _ = std::fs::remove_dir_all(to);
        let mut command = Command::new("git");
        command.args(["-c", "core.symlinks=false", "clone", "--no-recurse-submodules"]);
        if shallow {
            command.args(["--depth", "1"]);
        }
        command.arg("--").arg(url).arg(to).env("GIT_TERMINAL_PROMPT", "0");
        hidden(&mut command);
        let output = command.output().map_err(|_| tr("Git is not installed: only GitHub links and archives work without it.", "Git n'est pas installe : sans lui, seuls les liens GitHub et les archives marchent.").to_string())?;
        if output.status.success() {
            return Ok(());
        }
        last = String::from_utf8_lossy(&output.stderr).lines().last().unwrap_or("git clone").to_string();
    }
    Err(last)
}

fn github_api() -> String {
    std::env::var("NDSGS_GITHUB_API").unwrap_or_else(|_| "https://api.github.com".to_string())
}

fn github_code() -> String {
    std::env::var("NDSGS_GITHUB_CODE").unwrap_or_else(|_| "https://codeload.github.com".to_string())
}

/// Une release GitHub : son tag et ses fichiers.
#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    assets: Vec<ReleaseAsset>,
}

#[derive(Deserialize, Clone)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

/// La dernière release, ou celle qu'on nomme. `None` s'il n'y en a pas, ou si GitHub limite les demandes : l'état du dépôt suffit alors.
fn read_release(owner: &str, repo: &str, tag: Option<&str>) -> Option<Release> {
    let url = match tag {
        Some(tag) => format!("{}/repos/{owner}/{repo}/releases/tags/{tag}", github_api()),
        None => format!("{}/repos/{owner}/{repo}/releases/latest", github_api()),
    };
    let temp = std::env::temp_dir().join(format!("ndsgs-release-{}.json", std::process::id()));
    let result = crate::native_game::download(&url, &temp).ok().and_then(|()| serde_json::from_slice(&std::fs::read(&temp).ok()?).ok());
    let _ = std::fs::remove_file(&temp);
    result
}

/// Ce qu'on a lu d'un lien : le dossier, et la release s'il y en a une.
struct Fetched {
    root: PathBuf,
    tag: String,
    assets: Vec<ReleaseAsset>,
    /// Un lien vers une archive : prise entière si elle ne dit rien d'elle-même.
    plain_name: Option<String>,
}

/// Télécharge ce que montre un lien dans `temp`.
fn fetch(link: &Link, temp: &Path) -> Result<Fetched, String> {
    let unpacked = temp.join("contenu");
    std::fs::create_dir_all(temp).map_err(|e| e.to_string())?;
    match link {
        Link::GitHub { owner, repo, reference, folder } => {
            mods::say(t!("Reading {owner}/{repo}...", "Lecture de {owner}/{repo}..."));
            let named_tag = reference.as_deref().and_then(|r| r.strip_prefix("refs/tags/"));
            // Une branche nommée dit l'état de cette branche ; sinon la release, comme OpenRAC
            let release = if reference.is_none() || named_tag.is_some() { read_release(owner, repo, named_tag) } else { None };
            let (tag, assets) = release.map_or((String::new(), Vec::new()), |r| (r.tag_name, r.assets));
            let wanted = if tag.is_empty() { reference.clone().unwrap_or_else(|| "HEAD".to_string()) } else { format!("refs/tags/{tag}") };
            let archive = temp.join("depot.zip");
            let url = format!("{}/{owner}/{repo}/zip/{wanted}", github_code());
            crate::native_game::download(&url, &archive).map_err(|e| if e.contains("404") { tr("This repository is private or does not exist. Only public repositories can be read.", "Ce depot est prive ou n'existe pas. Seuls les depots publics peuvent etre lus.").to_string() } else { e })?;
            unpack(&archive, &unpacked)?;
            let top = only_folder(&unpacked).unwrap_or_else(|| unpacked.clone());
            let root = match folder {
                Some(folder) => top.join(folder),
                None => top,
            };
            if !root.is_dir() {
                return Err(tr("No such folder in this repository.", "Ce dossier n'existe pas dans ce depot.").to_string());
            }
            Ok(Fetched { root, tag, assets, plain_name: None })
        }
        Link::Archive(url) => {
            mods::say(tr("Downloading...", "Telechargement...").to_string());
            let file_name = url.split(['?', '#']).next().unwrap_or(url).rsplit('/').next().unwrap_or("mod.zip").to_string();
            let extension = file_name.rfind('.').map_or(String::new(), |at| file_name[at..].to_string());
            let archive = temp.join(format!("archive{extension}"));
            crate::native_game::download(url, &archive)?;
            unpack(&archive, &unpacked)?;
            Ok(Fetched { root: unpacked, tag: String::new(), assets: Vec::new(), plain_name: Some(stem_of(Path::new(&file_name))) })
        }
        Link::Git(url) => {
            mods::say(t!("Git clone of {url}...", "Clone git de {url}..."));
            clone_repository(url, &unpacked)?;
            let _ = std::fs::remove_dir_all(unpacked.join(".git"));
            Ok(Fetched { root: unpacked, tag: String::new(), assets: Vec::new(), plain_name: None })
        }
    }
}

/// Un mod trouvé derrière un lien, avant de l'installer.
#[derive(Clone)]
pub struct Found {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    /// Pourquoi il ne peut pas servir ; `None` s'il va avec ce jeu.
    pub refused: Option<String>,
    /// La version installée.
    pub installed: Option<String>,
    /// Ce qu'il fait sur l'ordinateur (mod de code).
    pub warning: Option<String>,
    folder: PathBuf,
}

/// Ce que montre la page après la lecture d'un lien.
#[derive(Clone)]
pub struct Preview {
    pub code: String,
    pub source: String,
    pub tag: String,
    pub mods: Vec<Found>,
    assets: Vec<ReleaseAsset>,
}

static PREVIEW: Mutex<Option<Preview>> = Mutex::new(None);
/// Les mises à jour trouvées : (code du jeu, id du mod, version).
static UPDATES: Mutex<Vec<(String, String, String)>> = Mutex::new(Vec::new());

pub fn preview(code: &str) -> Option<Preview> {
    PREVIEW.lock().unwrap().clone().filter(|p| p.code == code)
}

pub fn close_preview(code: &str) {
    *PREVIEW.lock().unwrap() = None;
    let _ = std::fs::remove_dir_all(mods::mod_dir(code).join(PREVIEW_DIR));
}

/// La version trouvée plus récente que celle installée.
pub fn update_for(code: &str, id: &str) -> Option<String> {
    UPDATES.lock().unwrap().iter().find(|(c, i, _)| c == code && i == id).map(|(_, _, version)| version.clone())
}

/// Les versions se comparent nombre par nombre : 1.10 vient après 1.9.
fn is_newer(new: &str, old: &str) -> bool {
    let numbers = |text: &str| -> Vec<u64> { text.trim_start_matches('v').split(['.', '-']).map(|part| part.parse().unwrap_or(0)).collect() };
    numbers(new) > numbers(old)
}

fn read_manifest(folder: &Path, name: &str) -> Option<Manifest> {
    let bytes = std::fs::read(folder.join(name)).ok()?;
    if name == MANIFEST {
        return serde_json::from_slice::<Manifest>(&bytes).ok().map(Manifest::translate);
    }
    // OpenRAC écrit ses jeux autrement : seuls son nom et sa description servent ici
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let text = |key: &str| value.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string();
    Some(Manifest { id: text("id"), name: text("name"), version: text("version"), author: text("author"), description: text("description"), ..Default::default() })
}

/// Les mods d'un dossier, ceux qui ne vont pas compris, avec la raison.
fn scan(root: &Path, code: &str, mod_dir: &Path, plain_name: Option<&str>) -> Vec<Found> {
    let mut folders = Vec::new();
    #[derive(Deserialize)]
    struct List {
        mods: Vec<String>,
    }
    match std::fs::read(root.join(MANIFEST_LIST)).ok().and_then(|bytes| serde_json::from_slice::<List>(&bytes).ok()) {
        Some(list) => folders.extend(list.mods.iter().filter(|name| !name.contains("..")).map(|name| root.join(name))),
        None => find_manifests(root, MAX_DEPTH, &[MANIFEST, OPENRAC_MANIFEST], &mut folders),
    }
    let state = load_state(mod_dir);
    let installed = |id: &str| -> Option<String> {
        state.packs.iter().any(|p| p.id == id).then(|| read_manifest(&packs_dir(mod_dir).join(id), MANIFEST).map_or(String::new(), |m| m.version))
    };
    let mut found = Vec::new();
    for folder in folders {
        if folder.join(MANIFEST).is_file() {
            let Some(manifest) = read_manifest(&folder, MANIFEST) else { continue };
            let fits = manifest.games.fits(code);
            let refused = if fits { manifest.code_refusal() } else { Some(t!("For other games: {}", "Pour d'autres jeux : {}", manifest.games.codes().join(", "))) };
            let warning = warning(&manifest);
            let id = clean_id(if manifest.id.is_empty() { &manifest.name } else { &manifest.id });
            let name = if manifest.name.is_empty() { id.clone() } else { manifest.name };
            found.push(Found { installed: installed(&id), id, name, version: manifest.version, author: manifest.author, description: manifest.description, refused, warning, folder });
        } else if let Some(manifest) = read_manifest(&folder, OPENRAC_MANIFEST) {
            let refused = Some(tr("Made for OpenRAC's port of Ratchet & Clank: it holds code for that game, not DS game files.", "Fait pour le portage de Ratchet & Clank d'OpenRAC : il contient du code pour ce jeu, pas des fichiers de jeu DS.").to_string());
            let name = if manifest.name.is_empty() { manifest.id.clone() } else { manifest.name };
            found.push(Found { id: manifest.id, name, version: manifest.version, author: manifest.author, description: manifest.description, refused, installed: None, warning: None, folder });
        }
    }
    if found.is_empty() {
        let structured = structured_root(root);
        if let Some(name) = plain_name.map(str::to_string).or_else(|| structured.is_some().then(|| stem_of(root))) {
            let id = clean_id(&name);
            found.push(Found { installed: installed(&id), id, name, version: String::new(), author: String::new(), description: String::new(), refused: None, warning: None, folder: structured.unwrap_or_else(|| root.to_path_buf()) });
        }
    }
    found.sort_by_key(|f| (f.refused.is_some(), f.name.to_lowercase()));
    found
}

/// Lit un lien, dans un fil à part : la page montre ensuite ce qu'il y a derrière. Rien n'est installé.
pub fn find(code: &str, text: &str) {
    let (code, text) = (code.to_string(), text.trim().to_string());
    mods::start(Some(code.clone()), move || {
        close_preview(&code);
        let dir = mods::mod_dir(&code);
        let temp = dir.join(PREVIEW_DIR);
        let link = parse_link(&text)?;
        let fetched = fetch(&link, &temp)?;
        let all = scan(&fetched.root, &code, &dir, fetched.plain_name.as_deref());
        let any = !all.is_empty();
        // Ce qui ne va pas avec ce jeu n'a rien à faire dans sa liste
        let found: Vec<Found> = all.into_iter().filter(|f| f.refused.is_none()).collect();
        if found.is_empty() {
            let _ = std::fs::remove_dir_all(&temp);
            // Le dépôt de base n'a simplement rien pour ce jeu : il n'y a rien à dire
            if text == BASE_REPOSITORY {
                return Ok(());
            }
            return Err(if any { tr("This repository has no mod for this game.", "Ce depot n'a aucun mod pour ce jeu.").to_string() } else { t!("No mod in this repository: {MANIFEST} is missing.", "Aucun mod dans ce depot : {MANIFEST} manque.") });
        }
        mods::say(t!("{} mods for this game.", "{} mods pour ce jeu.", found.len()));
        *PREVIEW.lock().unwrap() = Some(Preview { code: code.clone(), source: text.clone(), tag: fetched.tag, mods: found, assets: fetched.assets });
        Ok(())
    });
}

fn sha256_of(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    Ok(Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect())
}

/// `<id>-<version>.zip` de la release, s'il est listé dans `SHA256SUMS` : téléchargé, contrôlé. Une somme fausse est un refus ; sans somme, c'est
/// le dépôt qui sert, comme pour une release sans archive.
fn from_release(preview: &Preview, found: &Found, temp: &Path) -> Result<Option<PathBuf>, String> {
    let names = [format!("{}-{}-{PLATFORM}.zip", found.id, found.version), format!("{}-{}.zip", found.id, found.version)];
    let Some(name) = names.into_iter().find(|n| preview.assets.iter().any(|a| a.name == *n)) else { return Ok(None) };
    let (Some(asset), Some(sums)) = (preview.assets.iter().find(|a| a.name == name), preview.assets.iter().find(|a| a.name == "SHA256SUMS")) else { return Ok(None) };
    std::fs::create_dir_all(temp).map_err(|e| e.to_string())?;
    let sums_file = temp.join("SHA256SUMS");
    crate::native_game::download(&sums.browser_download_url, &sums_file)?;
    let text = std::fs::read_to_string(&sums_file).map_err(|e| e.to_string())?;
    let expected = text.lines().find_map(|line| {
        let (hash, file) = line.split_once(char::is_whitespace)?;
        (file.trim().trim_start_matches('*') == name).then(|| hash.to_lowercase())
    });
    let Some(expected) = expected else { return Ok(None) };
    let archive = temp.join(&name);
    crate::native_game::download(&asset.browser_download_url, &archive)?;
    if sha256_of(&archive)? != expected {
        return Err(t!("{name} does not match SHA256SUMS: refused.", "{name} ne correspond pas a SHA256SUMS : refuse."));
    }
    let out = temp.join("archive");
    unpack(&archive, &out)?;
    let mut folders = Vec::new();
    find_manifests(&out, MAX_DEPTH, &[MANIFEST], &mut folders);
    let same = folders.into_iter().find(|folder| read_manifest(folder, MANIFEST).is_some_and(|m| clean_id(&m.id) == found.id && m.version == found.version));
    same.map(Some).ok_or_else(|| t!("{name}: its description is not the one shown: refused.", "{name} : sa description n'est pas celle montree : refuse."))
}

/// Installe un des mods montrés. La page reste ouverte s'il y en a d'autres.
pub fn install_found(code: &str, rom: &Path, id: &str) {
    let (code, rom, id) = (code.to_string(), rom.to_path_buf(), id.to_string());
    mods::start(Some(code.clone()), move || {
        let result = (|| {
            let preview = preview(&code).ok_or("le lien n'est plus montre")?;
            let found = preview.mods.iter().find(|f| f.id == id).ok_or("mod introuvable")?;
            let dir = mods::mod_dir(&code);
            std::fs::create_dir_all(packs_dir(&dir)).map_err(|e| e.to_string())?;
            let temp = dir.join(TEMP);
            let _ = std::fs::remove_dir_all(&temp);
            mods::say(t!("Installing {}...", "Installation de {}...", found.name));
            let from = from_release(&preview, found, &temp);
            let installed = from.and_then(|asset| install_one(&dir, &code, asset.as_deref().unwrap_or(&found.folder), &found.id));
            let _ = std::fs::remove_dir_all(&temp);
            installed?;
            let mut state = load_state(&dir);
            match state.packs.iter_mut().find(|p| p.id == found.id) {
                // Comme une réinstallation chez OpenRAC : sa place et son choix restent
                Some(entry) => {
                    entry.source = preview.source.clone();
                    entry.tag = preview.tag.clone();
                }
                None => state.packs.push(Entry { id: found.id.clone(), enabled: true, source: preview.source.clone(), tag: preview.tag.clone() }),
            }
            save_state(&dir, &state)?;
            UPDATES.lock().unwrap().retain(|(c, i, _)| !(c == &code && i == &found.id));
            // Le bouton passe à « Réinstaller »
            if let Some(shown) = PREVIEW.lock().unwrap().as_mut() {
                for f in shown.mods.iter_mut().filter(|f| f.id == found.id) {
                    f.installed = Some(found.version.clone());
                }
            }
            finish(&code, &rom, vec![found.name.clone()])
        })();
        result.inspect_err(|e: &String| crate::activity::record(crate::activity::What::ModFailed { game: code.clone(), error: e.clone() }))
    });
}

// ------------------------------------------------------------------ mods de code (format 2, native/sdk/nds_mod.h)

/// La machine des modules natifs, comme `PLATFORM` de `native/nitro-pc/mods.c` : sous Windows, le module reste en 32 bits.
pub const PLATFORM: &str = if cfg!(target_os = "macos") {
    if cfg!(target_arch = "aarch64") { "macos-arm64" } else { "macos-x86_64" }
} else {
    "windows-x86"
};
/// La version majeure de `nds_mod.h` que le runtime livré connaît.
const API_MAJOR: u32 = 1;
const PLUGINS: &str = "plugins.json";
const STATUS: &str = "status.json";
const SETTINGS: &str = "reglages.json";

/// `"games"` : une liste de codes, ou un objet code → SHA-1 des roms (format 2).
#[derive(Serialize, Deserialize, Clone)]
#[serde(untagged)]
enum Games {
    List(Vec<String>),
    Roms(std::collections::BTreeMap<String, Vec<String>>),
}

impl Default for Games {
    fn default() -> Self {
        Games::List(Vec::new())
    }
}

impl Games {
    fn codes(&self) -> Vec<String> {
        match self {
            Games::List(list) => list.clone(),
            Games::Roms(map) => map.keys().cloned().collect(),
        }
    }

    fn is_empty(&self) -> bool {
        self.codes().is_empty()
    }

    fn fits(&self, code: &str) -> bool {
        self.is_empty() || self.codes().iter().any(|g| g.eq_ignore_ascii_case(code))
    }

    /// Les SHA-1 acceptées pour ce jeu ; vide = toute rom de ce code.
    fn roms(&self, code: &str) -> Vec<String> {
        match self {
            Games::List(_) => Vec::new(),
            Games::Roms(map) => map.iter().find(|(c, _)| c.eq_ignore_ascii_case(code)).map_or(Vec::new(), |(_, r)| r.iter().map(|s| s.to_lowercase()).collect()),
        }
    }
}

/// Un réglage décrit par le manifeste : la page du mod le construit seule.
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Setting {
    pub key: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    name_fr: String,
    /// `bool`, `int`, `float`, `choice`, `string`, `keybind`
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub default: serde_json::Value,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub step: Option<f64>,
    #[serde(default)]
    pub choices: Vec<String>,
}

/// Des valeurs toutes prêtes pour plusieurs réglages.
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Preset {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    name_fr: String,
    #[serde(default)]
    pub values: std::collections::BTreeMap<String, serde_json::Value>,
}

impl Manifest {
    /// Un mod qui apporte du code : une bibliothèque native (ou, plus tard, des scripts).
    fn runs_code(&self) -> bool {
        self.kind == "native" || self.kind == "mixed" || !self.plugin.is_empty()
    }

    /// Pourquoi ce mod de code ne peut pas servir ici ; `None` s'il le peut.
    fn code_refusal(&self) -> Option<String> {
        if !self.runs_code() {
            return None;
        }
        let major = self.api.split('.').next().and_then(|m| m.parse::<u32>().ok());
        if major != Some(API_MAJOR) {
            return Some(t!("Made for another version of the mod interface ({}): this launcher has {API_MAJOR}.x.", "Fait pour une autre version de l'interface des mods ({}) : ce launcher a la {API_MAJOR}.x.", if self.api.is_empty() { "?" } else { &self.api }));
        }
        if !self.plugin.contains_key(PLATFORM) {
            return Some(t!("No version for this computer ({PLATFORM}).", "Pas de version pour cet ordinateur ({PLATFORM})."));
        }
        None
    }
}

/// Ce qu'un mod fait sur l'ordinateur, montré avant de l'installer et sur sa ligne.
fn warning(manifest: &Manifest) -> Option<String> {
    manifest.runs_code().then(|| tr("Runs code on this computer: install it only from a source you trust.", "Execute du code sur cet ordinateur : ne l'installez que depuis une source de confiance.").to_string())
}

/// Les valeurs choisies par le joueur : `reglages.json`, id du mod → clé → valeur.
type Values = std::collections::BTreeMap<String, std::collections::BTreeMap<String, serde_json::Value>>;

fn load_values(mod_dir: &Path) -> Values {
    std::fs::read(mod_dir.join(SETTINGS)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

fn pack_manifest(mod_dir: &Path, id: &str) -> Manifest {
    std::fs::read(packs_dir(mod_dir).join(id).join(MANIFEST)).ok().and_then(|bytes| serde_json::from_slice::<Manifest>(&bytes).ok()).unwrap_or_default()
}

/// Un touche choisie se garde par son code HID ; le manifeste peut la donner par son nom (`"KeyC"`).
fn keybind_value(value: &serde_json::Value) -> serde_json::Value {
    match value.as_str().and_then(crate::native_game::hid_of_name) {
        Some(hid) => serde_json::Value::from(hid),
        None => value.clone(),
    }
}

/// Les réglages d'un mod, avec la valeur en cours (choisie, sinon celle du manifeste), et ses préréglages.
pub fn settings_of(code: &str, id: &str) -> (Vec<(Setting, serde_json::Value)>, Vec<Preset>) {
    let dir = mods::mod_dir(code);
    let manifest = pack_manifest(&dir, id);
    let chosen = load_values(&dir).remove(id).unwrap_or_default();
    let french = lang() == Lang::Fr;
    let settings = manifest
        .settings
        .into_iter()
        .map(|mut s| {
            let mut value = chosen.get(&s.key).cloned().unwrap_or_else(|| s.default.clone());
            if s.kind == "keybind" {
                value = keybind_value(&value);
            }
            if french && !s.name_fr.is_empty() {
                s.name = std::mem::take(&mut s.name_fr);
            }
            if s.name.is_empty() {
                s.name = s.key.clone();
            }
            (s, value)
        })
        .collect();
    let presets = manifest
        .presets
        .into_iter()
        .map(|mut p| {
            if french && !p.name_fr.is_empty() {
                p.name = std::mem::take(&mut p.name_fr);
            }
            p
        })
        .collect();
    (settings, presets)
}

pub fn set_setting(code: &str, id: &str, key: &str, value: serde_json::Value) {
    let dir = mods::mod_dir(code);
    let mut values = load_values(&dir);
    values.entry(id.to_string()).or_default().insert(key.to_string(), value);
    if let Ok(text) = serde_json::to_vec_pretty(&values) {
        let _ = std::fs::write(dir.join(SETTINGS), text);
    }
}

/// Toutes les valeurs d'un préréglage d'un coup.
pub fn apply_preset(code: &str, id: &str, preset: &Preset) {
    for (key, value) in &preset.values {
        set_setting(code, id, key, value.clone());
    }
}

/// L'état d'un mod de code à la dernière partie, écrit par le module (`status.json`) : (état, raison).
fn last_status(mod_dir: &Path, id: &str) -> Option<(String, Option<String>)> {
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(mod_dir.join(STATUS)).ok()?).ok()?;
    let entry = value.get("mods")?.as_array()?.iter().find(|m| m.get("id").and_then(|v| v.as_str()) == Some(id))?;
    let state = entry.get("state")?.as_str()?.to_string();
    Some((state, entry.get("reason").and_then(|v| v.as_str()).map(str::to_string)))
}

/// SHA-1 d'une rom, gardée tant que le fichier ne change pas : une rom peut faire 256 Mo.
fn rom_sha1(rom: &Path) -> Option<String> {
    use sha1::{Digest, Sha1};
    static CACHE: Mutex<Vec<(PathBuf, u64, std::time::SystemTime, String)>> = Mutex::new(Vec::new());
    let meta = std::fs::metadata(rom).ok()?;
    let (size, time) = (meta.len(), meta.modified().ok()?);
    if let Some(hit) = CACHE.lock().unwrap().iter().find(|(p, s, t, _)| p == rom && *s == size && *t == time) {
        return Some(hit.3.clone());
    }
    let mut hasher = Sha1::new();
    let mut file = std::fs::File::open(rom).ok()?;
    std::io::copy(&mut file, &mut hasher).ok()?;
    let hex: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    CACHE.lock().unwrap().push((rom.to_path_buf(), size, time, hex.clone()));
    Some(hex)
}

/// Écrit `plugins.json` pour les mods de code actifs, dans l'ordre de la liste, et rend son chemin ; `None` sans mod de code.
/// Le module vérifie l'interface, le jeu et la machine, charge les mods et écrit `status.json` à côté. Le launcher refuse lui-même
/// ce qu'il est seul à savoir : la rom (SHA-1).
pub fn write_plugins(code: &str, rom: &Path) -> Option<PathBuf> {
    if !mods::this_run_has_mods(code) {
        return None;
    }
    let dir = mods::mod_dir(code);
    let values = load_values(&dir);
    let mut entries = Vec::new();
    for entry in load_state(&dir).packs.into_iter().filter(|p| p.enabled) {
        let manifest = pack_manifest(&dir, &entry.id);
        if !manifest.runs_code() {
            continue;
        }
        let pack = packs_dir(&dir).join(&entry.id);
        let plugin: serde_json::Map<String, serde_json::Value> = manifest.plugin.iter().map(|(platform, file)| (platform.clone(), serde_json::Value::from(pack.join(file).to_string_lossy().into_owned()))).collect();
        let mut settings = serde_json::Map::new();
        let (described, _) = settings_of(code, &entry.id);
        for (setting, value) in described {
            settings.insert(setting.key, value);
        }
        for (key, value) in values.get(&entry.id).into_iter().flatten() {
            settings.entry(key.clone()).or_insert_with(|| value.clone());
        }
        let mut item = serde_json::json!({
            "id": entry.id, "version": manifest.version, "api": manifest.api, "games": manifest.games.codes(),
            "plugin": plugin, "settings": settings, "multiplayer": manifest.multiplayer,
        });
        let roms = manifest.games.roms(code);
        if !roms.is_empty() && !rom_sha1(rom).is_some_and(|sha| roms.contains(&sha)) {
            item["refused"] = serde_json::Value::from(tr("made for another version of this game (its rom is not one of those listed)", "fait pour une autre version de ce jeu (sa rom n'est pas une de celles listees)"));
        }
        entries.push(item);
    }
    if entries.is_empty() {
        return None;
    }
    let path = dir.join(PLUGINS);
    let text = serde_json::to_vec_pretty(&serde_json::json!({ "format": 1, "mods": entries })).ok()?;
    std::fs::write(&path, text).ok()?;
    // L'état de la partie précédente ne vaut plus
    let _ = std::fs::remove_file(dir.join(STATUS));
    Some(path)
}

/// Les instructions que les mods actifs accrochent dans ce jeu (`"instruction_hooks"`) : le module est généré avec elles.
pub fn instruction_hooks(code: &str) -> Vec<String> {
    if !mods::this_run_has_mods(code) {
        return Vec::new();
    }
    let dir = mods::mod_dir(code);
    let mut all: Vec<String> = load_state(&dir)
        .packs
        .into_iter()
        .filter(|p| p.enabled)
        .flat_map(|p| {
            let manifest = pack_manifest(&dir, &p.id);
            manifest.instruction_hooks.iter().find(|(c, _)| c.eq_ignore_ascii_case(code)).map_or(Vec::new(), |(_, list)| list.clone())
        })
        .filter_map(|a| u32::from_str_radix(a.trim().trim_start_matches("0x").trim_start_matches("0X"), 16).ok())
        .map(|a| format!("0x{:08x}", a & !1))
        .collect();
    all.sort();
    all.dedup();
    all
}

/// Les mods actifs qui touchent à la sauvegarde (`"save": "separate"`) : elle est alors séparée, une par ensemble de mods.
pub fn separate_save(code: &str) -> Option<String> {
    if !mods::this_run_has_mods(code) {
        return None;
    }
    let dir = mods::mod_dir(code);
    let mut ids: Vec<String> = load_state(&dir).packs.into_iter().filter(|p| p.enabled && pack_manifest(&dir, &p.id).save == "separate").map(|p| p.id).collect();
    ids.sort();
    (!ids.is_empty()).then(|| ids.join("+"))
}

/// Sur Mac, une bibliothèque téléchargée peut porter la quarantaine : le module ne la chargerait pas.
fn clear_quarantine(dir: &Path) {
    if cfg!(target_os = "macos") {
        let mut command = Command::new("xattr");
        command.arg("-dr").arg("com.apple.quarantine").arg(dir);
        let _ = command.output();
    }
}

/// Les liens déjà utilisés pour ce jeu.
pub fn saved_links(code: &str) -> Vec<String> {
    let mut links: Vec<String> = load_state(&mods::mod_dir(code)).packs.into_iter().map(|p| p.source).filter(|s| !s.is_empty()).collect();
    links.sort();
    links.dedup();
    links
}

/// Demande à chaque source si elle a mieux, dans un fil à part.
pub fn check_updates(code: &str) {
    let code = code.to_string();
    mods::start(Some(code.clone()), move || {
        let dir = mods::mod_dir(&code);
        let temp = dir.join(TEMP);
        let packs = list(&code);
        let mut new = Vec::new();
        for source in saved_links(&code) {
            let _ = std::fs::remove_dir_all(&temp);
            let fetched = parse_link(&source).and_then(|link| fetch(&link, &temp));
            match fetched {
                Ok(fetched) => {
                    for found in scan(&fetched.root, &code, &dir, None) {
                        let older = packs.iter().find(|p| p.id == found.id && p.source == source).is_some_and(|p| is_newer(&found.version, &p.version));
                        if older && found.refused.is_none() {
                            new.push((code.clone(), found.id, found.version));
                        }
                    }
                }
                Err(e) => mods::say(format!("{source} : {e}")),
            }
        }
        let _ = std::fs::remove_dir_all(&temp);
        mods::say(if new.is_empty() { tr("Your mods are up to date.", "Vos mods sont a jour.").to_string() } else { t!("{} updates available.", "{} mises a jour disponibles.", new.len()) });
        let mut updates = UPDATES.lock().unwrap();
        updates.retain(|(c, _, _)| c != &code);
        updates.extend(new);
        Ok(())
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hub(owner: &str, repo: &str, reference: Option<&str>, folder: Option<&str>) -> Link {
        Link::GitHub { owner: owner.into(), repo: repo.into(), reference: reference.map(Into::into), folder: folder.map(Into::into) }
    }

    #[test]
    fn links_are_read_like_openrac() {
        let plain = hub("OpenRAC", "rac1-mods", None, None);
        for text in ["https://github.com/OpenRAC/rac1-mods", "http://github.com/OpenRAC/rac1-mods/", "github.com/OpenRAC/rac1-mods", "https://github.com/OpenRAC/rac1-mods.git", "git@github.com:OpenRAC/rac1-mods.git", "OpenRAC/rac1-mods", " OpenRAC/rac1-mods "] {
            assert_eq!(parse_link(text).unwrap(), plain, "{text}");
        }
        assert_eq!(parse_link("https://github.com/o/r/tree/main/mods/one").unwrap(), hub("o", "r", Some("refs/heads/main"), Some("mods/one")));
        assert_eq!(parse_link("https://github.com/o/r/releases/tag/v2").unwrap(), hub("o", "r", Some("refs/tags/v2"), None));
        assert_eq!(parse_link("https://gitlab.com/o/r.git").unwrap(), Link::Git("https://gitlab.com/o/r.git".into()));
        assert_eq!(parse_link("https://site.org/mod.zip?x=1").unwrap(), Link::Archive("https://site.org/mod.zip?x=1".into()));
        assert_eq!(parse_link("https://github.com/o/r/releases/download/v1/m.zip").unwrap(), Link::Archive("https://github.com/o/r/releases/download/v1/m.zip".into()));
    }

    #[test]
    fn dangerous_links_are_refused() {
        for text in ["", "--upload-pack=x", "ext::sh -c x", "file:///C:/x", "C:\\x\\y", "https://github.com/o/r/tree/main/../..", "../../x", "a b/c"] {
            assert!(parse_link(text).is_err(), "{text}");
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ndsgs-packs-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: PathBuf, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn a_plain_zip_folder_is_one_mod() {
        let (source, mods) = (temp("plain-s"), temp("plain-m"));
        write(source.join("wrapper/rom.bin/1.png"), "x");
        let names = install_tree(&mods, &source, "AL7E", "", "Mon mod").unwrap();
        assert_eq!(names, ["Mon-mod"]);
        assert!(mods.join("paquets/Mon-mod/fichiers/wrapper/rom.bin/1.png").is_file(), "le chemin reste entier : l'outil le reconnait par sa fin");
        let state = load_state(&mods);
        assert!(state.packs[0].enabled);
    }

    #[test]
    fn a_repository_can_hold_several_mods_and_keeps_only_this_game() {
        let (source, mods) = (temp("repo-s"), temp("repo-m"));
        write(source.join("a/nds-mod.json"), r#"{"id":"a","name":"Mod A","games":["AL7E"]}"#);
        write(source.join("a/fichiers/x.png"), "x");
        write(source.join("a/README.md"), "r");
        write(source.join("b/nds-mod.json"), r#"{"id":"b","name":"Mod B"}"#);
        write(source.join("b/data/y.png"), "y");
        write(source.join("b/LICENSE"), "l");
        write(source.join("c/nds-mod.json"), r#"{"id":"c","games":["ZZZZ"]}"#);
        write(source.join("c/z.png"), "z");
        let mut names = install_tree(&mods, &source, "AL7E", "o/r", "r").unwrap();
        names.sort();
        assert_eq!(names, ["Mod A", "Mod B"]);
        assert!(mods.join("paquets/b/fichiers/data/y.png").is_file());
        assert!(!mods.join("paquets/b/fichiers/LICENSE").exists());
        assert!(!mods.join("paquets/c").exists());
        let state = load_state(&mods);
        assert!(state.packs.iter().all(|p| !p.enabled && p.source == "o/r"), "plusieurs mods : a choisir");
    }

    #[test]
    fn a_zip_around_a_files_folder_is_opened() {
        let (source, mods) = (temp("wrap-s"), temp("wrap-m"));
        write(source.join("mon mod v2/fichiers/rom.bin/1.png"), "x");
        install_tree(&mods, &source, "AL7E", "", "m").unwrap();
        assert!(mods.join("paquets/m/fichiers/rom.bin/1.png").is_file());
    }

    #[test]
    fn a_repository_without_ds_mods_says_so() {
        let (source, mods) = (temp("none-s"), temp("none-m"));
        write(source.join("button-swap/openrac-mod.json"), "{}");
        write(source.join("README.md"), "r");
        let error = install_tree(&mods, &source, "AL7E", "OpenRAC/rac1-mods", "rac1-mods").unwrap_err();
        assert!(error.contains("nds-mod.json"), "{error}");
        assert!(!mods.join("paquets").exists() || std::fs::read_dir(mods.join("paquets")).unwrap().next().is_none());
    }

    #[test]
    fn an_update_keeps_the_choice_and_replaces_the_files() {
        let (source, mods) = (temp("up-s"), temp("up-m"));
        write(source.join("nds-mod.json"), r#"{"id":"m"}"#);
        write(source.join("old.png"), "x");
        install_tree(&mods, &source, "AL7E", "o/r", "r").unwrap();
        let mut state = load_state(&mods);
        state.packs[0].enabled = false;
        save_state(&mods, &state).unwrap();
        std::fs::remove_file(source.join("old.png")).unwrap();
        write(source.join("new.png"), "y");
        install_tree(&mods, &source, "AL7E", "o/r", "r").unwrap();
        assert!(!load_state(&mods).packs[0].enabled);
        assert!(mods.join("paquets/m/fichiers/new.png").is_file() && !mods.join("paquets/m/fichiers/old.png").exists());
    }

    #[test]
    fn a_mod_folder_gives_only_this_games_part() {
        let (source, mods_dir) = (temp("games-s"), temp("games-m"));
        write(source.join("fps/nds-mod.json"), r#"{"id":"fps","games":["AL7E","AR5E"]}"#);
        write(source.join("fps/AL7E/native.json"), r#"{"features":["first_person"]}"#);
        write(source.join("fps/AL7E/fichiers/a.png"), "a");
        write(source.join("fps/AR5E/fichiers/b.png"), "b");
        install_tree(&mods_dir, &source, "AL7E", "o/r", "r").unwrap();
        let pack = mods_dir.join("paquets/fps");
        assert!(pack.join("fichiers/a.png").is_file() && !pack.join("fichiers/b.png").exists());
        assert!(pack.join(MANIFEST).is_file());
        assert_eq!(pack_features(&pack), ["first_person"]);
    }

    #[test]
    fn a_mod_with_only_runtime_features_needs_no_tool() {
        let (source, mods_dir) = (temp("feat-s"), temp("feat-m"));
        write(source.join("nds-mod.json"), r#"{"id":"m"}"#);
        write(source.join("native.json"), r#"{"features":["interpolate"]}"#);
        install_tree(&mods_dir, &source, "AL7E", "o/r", "r").unwrap();
        let pack = mods_dir.join("paquets/m");
        assert!(!has_files(&pack));
        assert_eq!(pack_features(&pack), ["interpolate"]);
        assert!(!pack.join(FILES).join(NATIVE).exists());
    }

    #[test]
    fn a_code_mod_keeps_only_this_computers_library() {
        let (source, mods_dir) = (temp("code-s"), temp("code-m"));
        let manifest = format!(r#"{{"format":2,"id":"cam","version":"2.0.0","type":"native","api":"1.0","games":{{"AL7E":["abc"]}},
            "plugin":{{"{PLATFORM}":"lib/here/cam.dll","autre-machine":"lib/other/cam.so"}}}}"#);
        write(source.join("nds-mod.json"), &manifest);
        write(source.join("lib/here/cam.dll"), "dll");
        write(source.join("lib/other/cam.so"), "so");
        write(source.join("src/mod.c"), "c");
        install_tree(&mods_dir, &source, "AL7E", "o/r", "r").unwrap();
        let pack = mods_dir.join("paquets/cam");
        assert!(pack.join("lib/here/cam.dll").is_file());
        assert!(!pack.join("lib/other").exists() && !pack.join(FILES).exists(), "ni l'autre machine, ni les sources comme fichiers de jeu");
        let m = read_manifest(&pack, MANIFEST).unwrap();
        assert!(m.runs_code() && m.code_refusal().is_none() && m.games.fits("AL7E") && !m.games.fits("AJTP"));
        assert_eq!(m.games.roms("al7e"), ["abc"]);
        assert!(warning(&m).is_some());
    }

    #[test]
    fn a_code_mod_says_why_it_cannot_run_here() {
        let (source, mods_dir) = (temp("nocode-s"), temp("nocode-m"));
        write(source.join("mac/nds-mod.json"), r#"{"format":2,"id":"mac","type":"native","api":"1.0","plugin":{"autre-machine":"x.dylib"}}"#);
        write(source.join("v2/nds-mod.json"), &format!(r#"{{"format":2,"id":"v2","type":"native","api":"2.0","plugin":{{"{PLATFORM}":"x"}}}}"#));
        write(source.join("data/nds-mod.json"), r#"{"id":"data","games":["AL7E"]}"#);
        let found = scan(&source, "AL7E", &mods_dir, None);
        let reason = |id: &str| found.iter().find(|f| f.id == id).unwrap().refused.clone();
        assert!(reason("mac").unwrap().contains(PLATFORM));
        assert!(reason("v2").unwrap().contains("2.0"));
        assert!(reason("data").is_none() && found.iter().find(|f| f.id == "data").unwrap().warning.is_none());
        let error = install_tree(&mods_dir, &source.join("mac"), "AL7E", "", "mac").unwrap_err();
        assert!(error.contains(PLATFORM), "{error}");
    }

    #[test]
    fn keybinds_can_be_named_in_the_manifest() {
        assert_eq!(keybind_value(&serde_json::json!("KeyC")), serde_json::json!(6));
        assert_eq!(keybind_value(&serde_json::json!(25)), serde_json::json!(25));
        assert_eq!(crate::native_game::key_label(6), "C");
        assert_eq!(crate::native_game::key_label(0x1E), "1");
    }

    #[test]
    fn versions_compare_by_numbers() {
        assert!(is_newer("1.10", "1.9"));
        assert!(is_newer("v2.0.0", "1.9.9"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("0.9", "1.0"));
    }

    #[test]
    fn a_link_shows_every_mod_and_says_why_some_do_not_fit() {
        let (source, mods_dir) = (temp("scan-s"), temp("scan-m"));
        write(source.join("bon/nds-mod.json"), r#"{"id":"bon","name":"Bon","version":"1.1","games":["AL7E"]}"#);
        write(source.join("autre/nds-mod.json"), r#"{"id":"autre","games":["ZZZZ"]}"#);
        write(source.join("openrac/openrac-mod.json"), r#"{"format":2,"id":"o","name":"O","games":{"rac1":["pal"]}}"#);
        let found = scan(&source, "AL7E", &mods_dir, None);
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].id, "bon");
        assert!(found[0].refused.is_none());
        assert!(found[1].refused.is_some() && found[2].refused.is_some());
        assert!(found.iter().find(|f| f.id == "o").unwrap().refused.as_deref().unwrap().contains("OpenRAC"));
    }

    #[test]
    fn a_list_of_folders_is_the_only_one_read() {
        let (source, mods_dir) = (temp("list-s"), temp("list-m"));
        write(source.join("nds-mods.json"), r#"{"format":1,"mods":["un"]}"#);
        write(source.join("un/nds-mod.json"), r#"{"id":"un"}"#);
        write(source.join("deux/nds-mod.json"), r#"{"id":"deux"}"#);
        let found = scan(&source, "AL7E", &mods_dir, None);
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn a_zip_cannot_write_outside_its_folder() {
        use std::io::Write;
        let dir = temp("zip");
        let archive = dir.join("m.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("../dehors.png", options).unwrap();
        zip.write_all(b"x").unwrap();
        zip.start_file("dedans/ok.png", options).unwrap();
        zip.write_all(b"y").unwrap();
        zip.finish().unwrap();
        let out = dir.join("sortie");
        unpack(&archive, &out).unwrap();
        assert!(out.join("dedans/ok.png").is_file());
        assert!(!dir.join("dehors.png").exists());
    }
}
