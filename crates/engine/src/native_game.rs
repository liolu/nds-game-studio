//! Jeux DS natifs : décompilation + runtime nitro-pc, sans émulateur.
//!
//! Chaque jeu est un programme généré sur le PC du joueur, depuis sa rom : il contient le code du jeu et n'est donc
//! jamais publié (voir `native/README.md`). Le launcher le prépare en tâche de fond, un jeu à la fois, et Jouer n'est
//! proposé qu'une fois le module prêt. Les outils (traducteur, compilateur, runtime, recettes) sont livrés à côté du
//! studio, dans `native-toolchain` ; sinon ils sont téléchargés. Un `native/<code>.exe` à côté du studio passe avant.
//!
//! Le jeu tourne dans son propre processus (32 bits sous Windows, où son code suppose des pointeurs 32 bits ; 64 bits
//! sous macOS, voir `native/MACOS.md`), et reçoit la rom de l'utilisateur en argument. Il dessine ses deux écrans à la définition de la fenêtre dans une mémoire
//! partagée et lit les commandes sur son entrée standard ; le moteur les affiche sur les écrans habituels (fenêtres
//! liées ou séparées, F8, F9). Le launcher lui donne le format et la définition : tout changement s'applique en jeu.

use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::window::{CursorGrabMode, CursorOptions};

use crate::activity::{self, What};
use crate::i18n::{t, tr};
use crate::input::{DsButton, DsInput, Stylus};
use crate::link::ShowToast;
use crate::screens::{Aspect, DsScreens, Engine, RenderStyle, Screen, ScreenLayout, ScreenPlaces};
use crate::settings::PlayerSettings;
use crate::studio::{in_game, ActiveGame, ActiveRom, AppGameExt, StudioState};

const W: usize = 256;
const H: usize = 192;
/// Mémoire d'images, comme `native/nitro-pc/present.c` : un en-tête, puis deux jeux de deux écrans en RGBA.
const MAX_W: usize = 2688;
const MAX_H: usize = 1152;
const SHM_HEADER: usize = 4096;
const SHM_BYTES: usize = SHM_HEADER + 4 * MAX_W * MAX_H * 4;
/// Message du module dans l'en-tête, comme `present.c`.
const NOTICE_AT: usize = 64;
const NOTICE_MAX: usize = 960;

/// Ce que le module écrit en tête de la mémoire partagée.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Header {
    /// Numéro de l'image, largeur, hauteur, jeu d'images terminé.
    seq: u32,
    width: u32,
    height: u32,
    front: u32,
    /// 2 : le module lit le paquet de 64 octets ; 0 : un module d'avant, qui lit celui de 16 octets.
    protocol: u32,
    /// Bit 0 : le module veut le curseur (caméra à la souris d'un mod ou du runtime).
    flags: u32,
    /// Change à chaque message à afficher.
    notice_seq: u32,
}
/// Pixels dessinés par pixel DS : au moins 2 pour la HD, le runtime ne va pas au-delà de 6.
const MAX_SCALE: f32 = 6.0;

/// La mémoire que le launcher et le jeu partagent.
struct SharedFrames {
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
    /// Nom de l'objet POSIX, pour le détruire.
    #[cfg(unix)]
    name: std::ffi::CString,
    view: *const u8,
}

// La mémoire reste valable tant que `SharedFrames` existe et n'est que lue ici.
unsafe impl Send for SharedFrames {}
unsafe impl Sync for SharedFrames {}

impl SharedFrames {
    #[cfg(windows)]
    fn new(name: &str) -> Option<Self> {
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::System::Memory::{CreateFileMappingW, MapViewOfFile, FILE_MAP_ALL_ACCESS, PAGE_READWRITE};
        let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
        // SAFETY: nom terminé par zéro, mémoire demandée au système et rendue dans `drop`.
        unsafe {
            let handle = CreateFileMappingW(INVALID_HANDLE_VALUE, std::ptr::null(), PAGE_READWRITE, 0, SHM_BYTES as u32, wide.as_ptr());
            if handle.is_null() {
                return None;
            }
            let view = MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, 0);
            if view.Value.is_null() {
                windows_sys::Win32::Foundation::CloseHandle(handle);
                return None;
            }
            Some(Self { handle, view: view.Value as *const u8 })
        }
    }

    /// macOS limite le nom à 31 caractères, barre oblique du début comprise.
    #[cfg(unix)]
    fn new(name: &str) -> Option<Self> {
        let name = std::ffi::CString::new(name).ok()?;
        if name.as_bytes().len() > 31 || name.as_bytes().first() != Some(&b'/') {
            return None;
        }
        // SAFETY: nom terminé par zéro ; le descripteur est fermé dans tous les cas, la vue est rendue dans `drop`.
        unsafe {
            // Un reste d'un processus arrêté de force qui avait le même numéro
            libc::shm_unlink(name.as_ptr());
            let fd = libc::shm_open(name.as_ptr(), libc::O_CREAT | libc::O_EXCL | libc::O_RDWR, 0o600 as libc::c_uint);
            if fd < 0 {
                return None;
            }
            let view = if libc::ftruncate(fd, SHM_BYTES as libc::off_t) == 0 {
                libc::mmap(std::ptr::null_mut(), SHM_BYTES, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, fd, 0)
            } else {
                libc::MAP_FAILED
            };
            libc::close(fd);
            if view == libc::MAP_FAILED {
                libc::shm_unlink(name.as_ptr());
                return None;
            }
            Some(Self { name, view: view as *const u8 })
        }
    }

    #[cfg(not(any(windows, unix)))]
    fn new(_name: &str) -> Option<Self> {
        None
    }

    /// L'en-tête que le module écrit (`ShmHeader` de `present.c`).
    fn header(&self) -> Header {
        // SAFETY: l'en-tête est dans la mémoire ; le jeu l'écrit, d'où la lecture volatile.
        let word = |i: usize| unsafe { std::ptr::read_volatile((self.view as *const u32).add(i)) };
        Header { seq: word(0), width: word(1), height: word(2), front: word(3), protocol: word(4), flags: word(5), notice_seq: word(6) }
    }

    /// Le message que le module demande d'afficher (UTF-8, terminé par 0).
    fn notice(&self) -> String {
        // SAFETY: NOTICE_MAX octets à NOTICE_AT, dans l'en-tête.
        let bytes = unsafe { std::slice::from_raw_parts(self.view.add(NOTICE_AT), NOTICE_MAX) };
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(NOTICE_MAX);
        String::from_utf8_lossy(&bytes[..end]).into_owned()
    }

    /// Les lignes d'un écran, bout à bout.
    fn screen(&self, set: usize, screen: usize, width: usize, height: usize) -> Vec<u8> {
        let mut pixels = Vec::with_capacity(width * height * 4);
        // SAFETY: width <= MAX_W et height <= MAX_H sont vérifiés par l'appelant, set et screen valent 0 ou 1.
        unsafe {
            let base = self.view.add(SHM_HEADER + (set * 2 + screen) * MAX_W * MAX_H * 4);
            for y in 0..height {
                pixels.extend_from_slice(std::slice::from_raw_parts(base.add(y * MAX_W * 4), width * 4));
            }
        }
        pixels
    }
}

#[cfg(windows)]
impl Drop for SharedFrames {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Memory::{UnmapViewOfFile, MEMORY_MAPPED_VIEW_ADDRESS};
        // SAFETY: vue et poignée viennent de `new` et ne servent plus.
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: self.view as *mut _ });
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(unix)]
impl Drop for SharedFrames {
    fn drop(&mut self) {
        // SAFETY: vue et nom viennent de `new` et ne servent plus ; le jeu garde sa propre vue jusqu'à sa fin.
        unsafe {
            libc::munmap(self.view as *mut libc::c_void, SHM_BYTES);
            libc::shm_unlink(self.name.as_ptr());
        }
    }
}

/// Le nom de la mémoire partagée avec le jeu. Sous macOS : 31 caractères au plus, comme `os_posix.c`.
fn shm_name(code: &str) -> String {
    if cfg!(unix) {
        format!("/ndsgs-{}-{code}", std::process::id())
    } else {
        format!("nds-game-studio-{}-{code}", std::process::id())
    }
}

/// Le dossier `native` à côté de l'exécutable.
pub fn native_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("NDSGS_NATIVE_DIR") {
        return PathBuf::from(dir);
    }
    let exe = std::env::current_exe().unwrap_or_default();
    exe.parent().unwrap_or(Path::new(".")).join("native")
}

/// Les modules se génèrent sous Windows (GCC) et macOS (clang des Command Line Tools) ; ailleurs, rien à générer.
const CAN_GENERATE: bool = cfg!(any(windows, target_os = "macos"));

/// Architecture de la boîte à outils : celle de ce programme, comme `native_build.py --arch`.
const MACOS_ARCH: &str = if cfg!(target_arch = "aarch64") { "arm64" } else { "x86_64" };

/// Python de la boîte à outils.
fn toolchain_python(tools: &Path) -> PathBuf {
    if cfg!(windows) {
        tools.join("python").join("python.exe")
    } else {
        tools.join("python").join("bin").join("python3")
    }
}

/// Nom du module d'un jeu : `<code>.exe` sous Windows, `<code>` ailleurs.
fn module_file(code: &str) -> String {
    format!("{code}{}", std::env::consts::EXE_SUFFIX)
}

/// `AL7E.exe` ou `AL7E` : le code d'un module de `native/`.
fn module_code(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let suffix = std::env::consts::EXE_SUFFIX;
    let split = name.len().checked_sub(suffix.len())?;
    let code = name.get(..split).filter(|_| name[split..].eq_ignore_ascii_case(suffix))?;
    (code.len() == 4 && path.is_file()).then(|| code.to_string())
}

/// Les jeux que le launcher sait générer et la version de la boîte à outils (`native/tools/export_recipes.py`).
const INDEX: &str = include_str!("../../../native/games/index.json");
const TOOLCHAIN_URL: &str = "https://github.com/liolu/nds-game-studio/releases/download/native-toolchain";

#[derive(serde::Deserialize, Clone)]
struct Index {
    toolchain: u32,
    /// Version de la boîte commune (GCC, Python, Capstone) des jeux qui ont leur propre runtime.
    #[serde(default)]
    common: u32,
    /// Version de l'hôte Luau des mods en script (`nds-luau-host-<v>-<machine>.zip`).
    #[serde(default)]
    luau_host: u32,
    games: std::collections::BTreeMap<String, IndexGame>,
}

#[derive(serde::Deserialize, Clone)]
struct IndexGame {
    status: String,
    #[serde(default)]
    play: String,
    code: Option<f32>,
    c: Option<f32>,
    data: Option<f32>,
    /// Le runtime de ce jeu vient de son dépôt (release `runtime-v<version>`) ; sans, de la boîte à outils commune.
    #[serde(default)]
    runtime: Option<GameRuntime>,
}

#[derive(serde::Deserialize, Clone, Debug, PartialEq)]
struct GameRuntime {
    /// `propriétaire/dépôt` sur GitHub.
    repo: String,
    version: u32,
}

/// État d'un jeu : montré sur sa page. Pourcentages du code identique à l'original, du code écrit en C
/// sans assembleur, et des données ; `play` vient de l'essai du jeu (`native/GAMES.md`).
pub struct GameState {
    pub code: Option<f32>,
    pub c: Option<f32>,
    pub data: Option<f32>,
    pub play: String,
}

pub fn game_state(code: &str) -> Option<GameState> {
    index().games.get(code).map(|g| GameState { code: g.code, c: g.c, data: g.data, play: g.play.clone() })
}

/// Lu une fois : le launcher le consulte pour chaque jeu de la vue. L'index téléchargé (`refresh_index`) passe avant celui du
/// launcher s'il est au moins aussi récent : un jeu ou une version du runtime arrive ainsi sans nouveau launcher.
fn index() -> &'static Index {
    static PARSED: OnceLock<Index> = OnceLock::new();
    PARSED.get_or_init(|| {
        let built: Index = serde_json::from_str(INDEX).expect("native/games/index.json");
        let downloaded = std::fs::read(downloaded_index()).ok().and_then(|bytes| serde_json::from_slice::<Index>(&bytes).ok());
        match downloaded {
            Some(index) if index.toolchain >= built.toolchain && !index.games.is_empty() => index,
            _ => built,
        }
    })
}

/// L'index lu au démarrage de cette session, s'il a pu l'être : les mises à jour qu'il annonce se proposent tout de suite.
static FRESH: Mutex<Option<Index>> = Mutex::new(None);

fn downloaded_index() -> PathBuf {
    crate::save::data_dir().join("native").join("index.json")
}

/// L'adresse de l'index : `NDSGS_NATIVE_INDEX` pour un essai.
fn index_url() -> String {
    std::env::var("NDSGS_NATIVE_INDEX").unwrap_or_else(|_| format!("{TOOLCHAIN_URL}/index.json"))
}

/// Télécharge l'index publié avec la boîte à outils : gardé pour le prochain démarrage, et lu tout de suite pour proposer les
/// mises à jour des runtimes. Sans réseau, rien ne change. C'est le seul téléchargement du démarrage.
fn refresh_index() {
    std::thread::spawn(|| {
        let target = downloaded_index();
        let temp = target.with_extension("json.new");
        let _ = std::fs::create_dir_all(target.parent().unwrap_or(Path::new(".")));
        let parsed = download(&index_url(), &temp).ok().and_then(|()| serde_json::from_slice::<Index>(&std::fs::read(&temp).ok()?).ok()).filter(|i| !i.games.is_empty());
        match parsed {
            Some(index) => {
                let _ = std::fs::rename(&temp, &target);
                *FRESH.lock().unwrap() = Some(index);
            }
            None => {
                let _ = std::fs::remove_file(&temp);
            }
        }
    });
}

pub fn luau_host_version() -> u32 {
    index().luau_host.max(1)
}

/// L'archive de l'hôte Luau pour cette machine, dans la release de la boîte à outils.
pub fn luau_host_url(platform: &str) -> String {
    let base = std::env::var("NDSGS_TOOLCHAIN_URL").unwrap_or_else(|_| TOOLCHAIN_URL.to_string());
    format!("{base}/nds-luau-host-{}-{platform}.zip", luau_host_version())
}

/// Ce jeu a un module natif, mais pas sur ce système.
pub fn windows_only(code: &str) -> bool {
    !CAN_GENERATE && index().games.contains_key(code)
}

/// Ce jeu se lance mais n'a pas été essayé jusqu'en jeu, ou ne marche pas encore.
pub fn is_experimental(code: &str) -> bool {
    index().games.get(code).is_some_and(|g| g.status != "ok")
}

// ------------------------------------------------------------------ runtime propre à un jeu (ROADMAP-MODS.md, étape 3 bis)

fn runtime_of(code: &str) -> Option<GameRuntime> {
    index().games.get(code)?.runtime.clone()
}

/// La dernière version publiée du runtime de ce jeu : celle de l'index lu au démarrage, sinon celle du dernier index gardé.
fn latest_runtime(code: &str) -> Option<u32> {
    let fresh = FRESH.lock().unwrap().as_ref().and_then(|i| Some(i.games.get(code)?.runtime.as_ref()?.version));
    fresh.or_else(|| runtime_of(code).map(|r| r.version))
}

/// La version du runtime installée pour un jeu (`native/<code>.json`) : celle qu'on joue tant qu'on n'accepte pas une mise à jour.
#[derive(serde::Serialize, serde::Deserialize, Default, Clone, Debug, PartialEq)]
struct Installed {
    version: u32,
    /// La version d'avant, gardée jusqu'à ce que celle-ci ait démarré : on y revient si elle ne démarre pas.
    #[serde(default)]
    previous: Option<u32>,
    #[serde(default)]
    confirmed: bool,
    /// « Ne plus proposer » : cette version n'est plus proposée.
    #[serde(default)]
    ignore: Option<u32>,
}

fn installed_path(code: &str) -> PathBuf {
    crate::save::data_dir().join("native").join(format!("{code}.json"))
}

fn load_installed(code: &str) -> Option<Installed> {
    serde_json::from_slice(&std::fs::read(installed_path(code)).ok()?).ok()
}

fn save_installed(code: &str, installed: &Installed) {
    if let Ok(text) = serde_json::to_vec_pretty(installed) {
        let _ = std::fs::create_dir_all(installed_path(code).parent().unwrap_or(Path::new(".")));
        let _ = std::fs::write(installed_path(code), text);
    }
}

/// La version du runtime qu'on joue : l'installée si son module est là, sinon la dernière (premier lancement). `None` : ce jeu
/// n'a pas de runtime propre, il prend la boîte à outils commune.
fn module_version(code: &str) -> Option<u32> {
    runtime_of(code)?;
    match load_installed(code) {
        Some(i) if generated_path_for(code, Some(i.version)).exists() => Some(i.version),
        _ => latest_runtime(code),
    }
}

/// Une mise à jour du runtime à proposer pour ce jeu : (installée, dernière). Jamais imposée.
pub fn runtime_update(code: &str) -> Option<(u32, u32)> {
    let installed = load_installed(code)?;
    let latest = latest_runtime(code)?;
    (generated_path_for(code, Some(installed.version)).exists() && latest > installed.version && installed.ignore != Some(latest)).then_some((installed.version, latest))
}

/// Le joueur accepte la mise à jour : le nouveau runtime est téléchargé et le module généré ; l'ancien reste jusque-là.
pub fn update_runtime(code: &str, rom: &Path) {
    if let Some(latest) = latest_runtime(code) {
        start_job(code, rom, Some(latest), true);
    }
}

/// « Ne plus proposer » cette version.
pub fn ignore_update(code: &str) {
    if let (Some(mut installed), Some(latest)) = (load_installed(code), latest_runtime(code)) {
        installed.ignore = Some(latest);
        save_installed(code, &installed);
    }
}

/// La génération en cours pour ce jeu, s'il y en a une : son message.
pub fn job_message(code: &str) -> Option<String> {
    let jobs = JOBS.lock().unwrap();
    let job = jobs.as_ref()?.get(code)?;
    let state = job.lock().unwrap();
    state.result.is_none().then(|| state.message.clone())
}

/// L'empreinte des instructions que les mods actifs accrochent (`"instruction_hooks"`) : elles sont traduites dans le module.
fn hooks_print(code: &str) -> String {
    let hooks = crate::mod_packs::instruction_hooks(code);
    if hooks.is_empty() {
        return String::new();
    }
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(hooks.join(",").as_bytes());
    format!("-h{}", digest.iter().take(4).map(|b| format!("{b:02x}")).collect::<String>())
}

/// Le module généré : `<code>-r<version du runtime du jeu>`, ou `<code>-<boîte à outils>` pour un jeu sans runtime propre, puis
/// l'empreinte des accroches d'instructions. Changer de version ou de mods de ce genre en génère un autre ; les autres restent.
fn generated_path_for(code: &str, runtime: Option<u32>) -> PathBuf {
    let version = match runtime {
        Some(v) => format!("r{v}"),
        None => index().toolchain.to_string(),
    };
    crate::save::data_dir().join("native").join(format!("{code}-{version}{}{}", hooks_print(code), std::env::consts::EXE_SUFFIX))
}

fn generated_path(code: &str) -> PathBuf {
    generated_path_for(code, module_version(code))
}

/// Le module à lancer s'il existe déjà.
fn module_path(code: &str) -> Option<PathBuf> {
    [native_dir().join(module_file(code)), generated_path(code)].into_iter().find(|p| p.exists())
}

/// Où en est la génération d'un module : lu à chaque image.
#[derive(Default)]
pub struct Generation {
    message: String,
    /// Les dernières lignes écrites par le programme qui prépare le jeu.
    lines: Vec<String>,
    started: Option<std::time::Instant>,
    result: Option<Result<PathBuf, String>>,
}

/// Lignes de la préparation gardées pour le launcher.
const KEPT_LINES: usize = 5;

/// La préparation en cours : ce que le launcher montre pendant qu'elle tourne.
pub struct CurrentJob {
    pub code: String,
    pub message: String,
    pub lines: Vec<String>,
    pub seconds: u64,
}

pub fn current_job() -> Option<CurrentJob> {
    let jobs = JOBS.lock().unwrap();
    jobs.as_ref()?.iter().find_map(|(code, job)| {
        let state = job.lock().unwrap();
        state.result.is_none().then(|| CurrentJob { code: code.clone(), message: state.message.clone(), lines: state.lines.clone(), seconds: state.started.map_or(0, |s| s.elapsed().as_secs()) })
    })
}

/// Où en sont les jeux installés : prêts, à préparer, en erreur.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub struct Overview {
    pub ready: usize,
    pub waiting: usize,
    pub failed: usize,
}

pub fn overview<'a>(codes: impl Iterator<Item = &'a str>, all: bool) -> Overview {
    let mut result = Overview::default();
    for code in codes {
        match preparation(code) {
            Preparation::Ready => result.ready += 1,
            Preparation::Failed(_) => result.failed += 1,
            Preparation::NotStarted if CAN_GENERATE && index().games.get(code).is_some_and(|g| all || g.status == "ok") => result.waiting += 1,
            _ => {}
        }
    }
    result
}

/// Les générations de cette session, par code de jeu.
static JOBS: Mutex<Option<std::collections::HashMap<String, Arc<Mutex<Generation>>>>> = Mutex::new(None);

/// Ce que le launcher montre pour un jeu.
pub enum Preparation {
    Ready,
    Running(String),
    Failed(String),
    NotStarted,
}

pub fn preparation(code: &str) -> Preparation {
    if module_path(code).is_some() {
        return Preparation::Ready;
    }
    let jobs = JOBS.lock().unwrap();
    let Some(job) = jobs.as_ref().and_then(|j| j.get(code)) else { return Preparation::NotStarted };
    let state = job.lock().unwrap();
    match &state.result {
        Some(Err(e)) => Preparation::Failed(e.clone()),
        Some(Ok(_)) => Preparation::Ready,
        None => Preparation::Running(state.message.clone()),
    }
}

/// Lance la génération de ce jeu, ou renvoie celle en cours. Une génération ratée est relancée ; `again` en refait une finie
/// (mise à jour du runtime).
fn start_job(code: &str, rom: &Path, runtime: Option<u32>, again: bool) -> Arc<Mutex<Generation>> {
    let mut jobs = JOBS.lock().unwrap();
    let map = jobs.get_or_insert_with(Default::default);
    if let Some(job) = map.get(code) {
        let result = &job.lock().unwrap().result;
        if result.is_none() || (!again && matches!(result, Some(Ok(_)))) {
            return job.clone();
        }
    }
    let state = Arc::new(Mutex::new(Generation { started: Some(std::time::Instant::now()), ..Default::default() }));
    map.insert(code.to_string(), state.clone());
    let (thread_state, code, rom) = (state.clone(), code.to_string(), rom.to_path_buf());
    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let result = generate(&code, &rom, runtime, &thread_state);
        match &result {
            Ok(_) => activity::record(What::Ready { game: code.clone(), seconds: started.elapsed().as_secs() }),
            Err(e) => {
                error!("{code} : le module natif n'a pas pu etre genere : {e}");
                activity::record(What::PrepareFailed { game: code.clone(), error: e.clone() });
            }
        }
        thread_state.lock().unwrap().result = Some(result);
    });
    state
}

/// Le joueur demande ce jeu : seul « Préparer » ou « Jouer » génère un module, jamais le démarrage du launcher.
pub fn prepare(code: &str, rom: &Path) {
    start_job(code, rom, module_version(code), false);
}

/// Oublie le module de ce jeu : il sera refait. Sans effet tant que sa génération tourne.
pub fn forget(code: &str) {
    let mut jobs = JOBS.lock().unwrap();
    if let Some(map) = jobs.as_mut() {
        if map.get(code).is_some_and(|job| job.lock().unwrap().result.is_none()) {
            return;
        }
        map.remove(code);
    }
    let _ = std::fs::remove_file(generated_path(code));
}

/// Jeu lancé sans module (ligne de commande) : son écran attend la génération.
#[derive(Resource)]
struct Generating {
    code: String,
    rom: PathBuf,
    state: Arc<Mutex<Generation>>,
    label: Entity,
}

fn say(state: &Mutex<Generation>, message: String) {
    state.lock().unwrap().message = message;
}

/// Télécharge un fichier d'une release du projet.
pub(crate) fn download(url: &str, to: &Path) -> Result<(), String> {
    let tls = ureq::tls::TlsConfig::builder().provider(ureq::tls::TlsProvider::NativeTls).build();
    let config = ureq::Agent::config_builder().tls_config(tls).build();
    let mut response = ureq::Agent::new_with_config(config).get(url).call().map_err(|e| format!("{url} : {e}"))?;
    let mut body = response.body_mut().as_reader();
    let mut file = std::fs::File::create(to).map_err(|e| e.to_string())?;
    std::io::copy(&mut body, &mut file).map_err(|e| e.to_string())?;
    Ok(())
}

/// Le dossier de la boîte à outils si elle est déjà là : celui donné par `NDSGS_TOOLCHAIN_DIR` (essai d'une boîte
/// faite depuis le dépôt), ou `native-toolchain` à côté de l'exécutable (les paquets d'avant l'étape 3 bis).
fn local_toolchain() -> Option<PathBuf> {
    let exe = std::env::current_exe().unwrap_or_default();
    let beside = exe.parent().unwrap_or(Path::new(".")).join("native-toolchain");
    // celle à côté de l'exécutable ne sert que si elle a la version de l'index (un index téléchargé peut en demander une plus récente)
    let same_version = |dir: &PathBuf| {
        let bundled = std::fs::read(dir.join("native").join("games").join("index.json")).ok().and_then(|bytes| serde_json::from_slice::<Index>(&bytes).ok());
        bundled.is_none_or(|i| i.toolchain == index().toolchain)
    };
    let given = std::env::var_os("NDSGS_TOOLCHAIN_DIR").map(PathBuf::from).filter(|dir| toolchain_python(dir).exists());
    given.or_else(|| Some(beside).filter(|dir| toolchain_python(dir).exists() && same_version(dir)))
}

/// Nom d'une archive de la release `native-toolchain` : une par plateforme.
fn platform_asset(stem: &str, version: u32) -> String {
    if cfg!(target_os = "macos") {
        format!("{stem}-{version}-macos-{MACOS_ARCH}.zip")
    } else {
        format!("{stem}-{version}.zip")
    }
}

/// Télécharge (ou copie, `local`) une archive, la contrôle avec `sha256` s'il y en a une, et la décompresse dans `dir`, une fois.
fn install_archive(dir: &Path, url: &str, local: Option<std::ffi::OsString>, sha256: Option<&str>, state: &Mutex<Generation>, downloading: String) -> Result<(), String> {
    let ready = dir.join("installed");
    if ready.exists() {
        return Ok(());
    }
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let archive = dir.join("archive.zip");
    match local {
        Some(local) => {
            std::fs::copy(local, &archive).map_err(|e| e.to_string())?;
        }
        None => {
            say(state, downloading);
            download(url, &archive)?;
        }
    }
    if let Some(expected) = sha256 {
        use sha2::{Digest, Sha256};
        let got: String = Sha256::digest(std::fs::read(&archive).map_err(|e| e.to_string())?).iter().map(|b| format!("{b:02x}")).collect();
        if got != expected {
            let _ = std::fs::remove_dir_all(dir);
            return Err(t!("{url} does not match SHA256SUMS: refused.", "{url} ne correspond pas a SHA256SUMS : refuse."));
        }
    }
    say(state, tr("Installing the tools...", "Installation des outils...").to_string());
    let file = std::fs::File::open(&archive).map_err(|e| e.to_string())?;
    zip::ZipArchive::new(file).and_then(|mut z| z.extract(dir)).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&archive);
    std::fs::write(ready, "").map_err(|e| e.to_string())
}

/// Télécharge et décompresse la boîte à outils si elle n'est pas là ; renvoie son dossier. Pour les jeux sans runtime propre.
fn ensure_toolchain(state: &Mutex<Generation>) -> Result<PathBuf, String> {
    if let Some(dir) = local_toolchain() {
        return Ok(dir);
    }
    let version = index().toolchain;
    let folder = if cfg!(target_os = "macos") { format!("{version}-macos-{MACOS_ARCH}") } else { version.to_string() };
    let dir = crate::save::data_dir().join("native-toolchain").join(folder);
    let url = format!("{TOOLCHAIN_URL}/{}", platform_asset("native-toolchain", version));
    install_archive(&dir, &url, std::env::var_os("NDSGS_TOOLCHAIN_ZIP"), None, state, tr("Downloading the tools (only once)...", "Telechargement des outils (une seule fois)...").to_string())?;
    Ok(dir)
}

/// La boîte commune (GCC, Python, Capstone), téléchargée au premier jeu joué qui a son propre runtime. Une boîte à outils
/// complète déjà là (paquets d'avant, essais) a tout ce qu'il faut.
fn ensure_common(state: &Mutex<Generation>) -> Result<PathBuf, String> {
    if let Some(dir) = local_toolchain() {
        return Ok(dir);
    }
    let version = index().common;
    let folder = if cfg!(target_os = "macos") { format!("{version}-macos-{MACOS_ARCH}") } else { version.to_string() };
    let dir = crate::save::data_dir().join("native-common").join(folder);
    let base = std::env::var("NDSGS_TOOLCHAIN_URL").unwrap_or_else(|_| TOOLCHAIN_URL.to_string());
    let url = format!("{base}/{}", platform_asset("native-common", version));
    install_archive(&dir, &url, None, None, state, tr("Downloading the tools (only once)...", "Telechargement des outils (une seule fois)...").to_string())?;
    Ok(dir)
}

/// Le runtime de ce jeu, version `version` : release `runtime-v<version>` de son dépôt, contrôlée par `SHA256SUMS`. Une seule
/// version par dossier : une mise à jour ne touche ni aux autres versions, ni aux autres jeux.
fn ensure_runtime(code: &str, repo: &str, version: u32, state: &Mutex<Generation>) -> Result<PathBuf, String> {
    let dir = crate::save::data_dir().join("native-runtime").join(format!("{code}-r{version}"));
    if dir.join("installed").exists() {
        return Ok(dir);
    }
    let base = std::env::var("NDSGS_GITHUB_DOWNLOAD").unwrap_or_else(|_| "https://github.com".to_string());
    let release = format!("{base}/{repo}/releases/download/runtime-v{version}");
    let name = format!("native-runtime-{version}.zip");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let sums = dir.join("SHA256SUMS");
    say(state, t!("Downloading this game's runtime (version {version})...", "Telechargement du runtime de ce jeu (version {version})..."));
    download(&format!("{release}/SHA256SUMS"), &sums)?;
    let text = std::fs::read_to_string(&sums).map_err(|e| e.to_string())?;
    let expected = text
        .lines()
        .find_map(|line| {
            let (hash, file) = line.split_once(char::is_whitespace)?;
            (file.trim().trim_start_matches('*') == name).then(|| hash.to_lowercase())
        })
        .ok_or_else(|| t!("SHA256SUMS of runtime-v{version} does not list {name}.", "SHA256SUMS de runtime-v{version} ne donne pas {name}."))?;
    install_archive(&dir, &format!("{release}/{name}"), None, Some(&expected), state, t!("Downloading this game's runtime (version {version})...", "Telechargement du runtime de ce jeu (version {version})..."))?;
    Ok(dir)
}

/// Sous macOS, le compilateur est celui des Command Line Tools : le jeu est compilé chez le joueur, et le SDK d'Apple
/// ne peut pas être redistribué avec la boîte à outils.
fn check_compiler() -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Ok(());
    }
    let found = Command::new("xcrun").args(["--find", "clang"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
    if found {
        return Ok(());
    }
    Err(tr(
        "The Xcode Command Line Tools are missing, and they compile the game. Open Terminal, run: xcode-select --install, then try again.",
        "Les outils de ligne de commande Xcode manquent, ils compilent le jeu. Ouvrez Terminal, lancez : xcode-select --install, puis recommencez.",
    )
    .to_string())
}

/// Lit une sortie du programme ligne par ligne : tout pour le journal, la fin pour le launcher.
fn follow_output(output: impl Read, state: &Mutex<Generation>, log: &Mutex<String>) {
    for line in std::io::BufReader::new(output).split(b'\n').map_while(Result::ok) {
        let line = String::from_utf8_lossy(&line).trim_end().to_string();
        log.lock().unwrap().push_str(&format!("{line}\n"));
        if !line.is_empty() {
            let mut state = state.lock().unwrap();
            state.lines.push(line);
            let extra = state.lines.len().saturating_sub(KEPT_LINES);
            state.lines.drain(..extra);
        }
    }
}

/// Sous Windows, le module est en 64 bits (étape 6) : la mémoire du DS est dans une fenêtre, comme sur Mac.
fn arch_args() -> Vec<String> {
    if cfg!(windows) { vec!["--arch".to_string(), "x86_64".to_string()] } else { Vec::new() }
}

fn insn_hook_args(code: &str) -> Vec<String> {
    let hooks = crate::mod_packs::instruction_hooks(code);
    if hooks.is_empty() { Vec::new() } else { vec!["--insn-hooks".to_string(), hooks.join(",")] }
}

/// Génère le module de ce jeu. `runtime` : la version du runtime propre au jeu (`None` : la boîte à outils commune).
fn generate(code: &str, rom: &Path, runtime: Option<u32>, state: &Mutex<Generation>) -> Result<PathBuf, String> {
    check_compiler()?;
    // Les outils (Python, compilateur) et le traducteur avec son runtime : ensemble dans la boîte commune, ou séparés
    let (tools, sources) = match (runtime, runtime_of(code)) {
        (Some(version), Some(game)) => (ensure_common(state)?, ensure_runtime(code, &game.repo, version, state)?),
        _ => {
            let tools = ensure_toolchain(state)?;
            (tools.clone(), tools)
        }
    };
    say(state, tr("Preparing the game from your rom (about a minute, only once)...", "Preparation du jeu depuis votre rom (environ une minute, une seule fois)...").to_string());
    let out = crate::save::data_dir().join("native").join(format!("build-{code}"));
    let mut command = Command::new(toolchain_python(&tools));
    command
        .arg("-u")
        .arg(sources.join("native").join("tools").join("native_build.py"))
        .arg("--recipe")
        .arg(sources.join("native").join("games").join(format!("{code}.json.gz")))
        .arg("--rom")
        .arg(rom)
        .arg("--out")
        .arg(&out)
        .args(insn_hook_args(code))
        .args(arch_args())
        .env("PYTHONPATH", tools.join("python"))
        .env("NDSGS_COMPILER_BIN", tools.join("mingw64").join("bin"))
        .env("PYTHONDONTWRITEBYTECODE", "1");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    // Le Mac reste utilisable pendant la compilation (`native_build.py` baisse aussi la priorité de ses compilateurs)
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setpriority est sûr entre fork et exec.
        unsafe {
            command.pre_exec(|| {
                libc::setpriority(libc::PRIO_PROCESS, 0, 10);
                Ok(())
            });
        }
    }
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let log = Mutex::new(String::new());
    let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
    std::thread::scope(|scope| {
        scope.spawn(|| stdout.map(|out| follow_output(out, state, &log)));
        scope.spawn(|| stderr.map(|err| follow_output(err, state, &log)));
    });
    let status = child.wait().map_err(|e| e.to_string())?;
    let log = log.into_inner().unwrap();
    let built = out.join(module_file(code));
    if !status.success() || !built.exists() {
        let tail: Vec<&str> = log.lines().rev().filter(|l| !l.trim().is_empty()).take(3).collect();
        error!("generation de {code} :\n{log}");
        return Err(tail.into_iter().rev().collect::<Vec<_>>().join(" / "));
    }
    let target = generated_path_for(code, runtime);
    let _ = std::fs::remove_file(&target);
    std::fs::rename(&built, &target).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&out);
    if let Some(version) = runtime.filter(|_| runtime_of(code).is_some()) {
        // La nouvelle version devient celle du jeu ; l'ancienne reste jusqu'à ce que la nouvelle ait démarré
        let before = load_installed(code).filter(|i| i.version != version && generated_path_for(code, Some(i.version)).exists());
        let ignore = before.as_ref().and_then(|i| i.ignore);
        let previous = before.map(|i| i.version);
        save_installed(code, &Installed { version, previous, confirmed: previous.is_none(), ignore });
    }
    Ok(target)
}

/// Une partie avec une version du runtime pas encore confirmée : après quelques secondes de jeu, elle l'est et l'ancien module
/// part ; si le module s'arrête avant, on revient à l'ancienne version, et on le dit.
fn trial_of(code: &str) -> Option<Installed> {
    load_installed(code).filter(|i| !i.confirmed && i.previous.is_some())
}

const TRIAL_SECONDS: u64 = 10;

fn confirm_trial(code: &str) {
    if let Some(mut installed) = trial_of(code) {
        if let Some(old) = installed.previous.take() {
            let _ = std::fs::remove_file(generated_path_for(code, Some(old)));
            let _ = std::fs::remove_dir_all(crate::save::data_dir().join("native-runtime").join(format!("{code}-r{old}")));
        }
        installed.confirmed = true;
        save_installed(code, &installed);
    }
}

/// Rend la version d'avant ; renvoie (nouvelle, ancienne) si c'en était une à l'essai.
fn revert_trial(code: &str) -> Option<(u32, u32)> {
    let installed = trial_of(code)?;
    let old = installed.previous?;
    let _ = std::fs::remove_file(generated_path_for(code, Some(installed.version)));
    let _ = std::fs::remove_dir_all(crate::save::data_dir().join("native-runtime").join(format!("{code}-r{}", installed.version)));
    save_installed(code, &Installed { version: old, previous: None, confirmed: true, ignore: Some(installed.version) });
    Some((installed.version, old))
}

pub struct NativeGamesPlugin;

impl Plugin for NativeGamesPlugin {
    fn build(&self, app: &mut App) {
        if !CAN_GENERATE {
            // Les outils Windows et les essais laissés par les versions qui tentaient de préparer les jeux ici
            let data = crate::save::data_dir();
            let _ = std::fs::remove_dir_all(data.join("native-toolchain"));
            let _ = std::fs::remove_dir_all(data.join("native"));
        }
        let mut codes: std::collections::BTreeSet<String> = if CAN_GENERATE { index().games.keys().cloned().collect() } else { Default::default() };
        for entry in std::fs::read_dir(native_dir()).into_iter().flatten().flatten() {
            if let Some(code) = module_code(&entry.path()) {
                codes.insert(code);
            }
        }
        if CAN_GENERATE {
            refresh_index();
        }
        for code in codes {
            let spec = format!("id = \"native-{code}\"\nname = \"{code}\"\ngame_code = \"{code}\"\n");
            app.add_ds_game(&spec, start_module);
        }
        app.add_systems(Update, (show_frames, cursor_and_notices, send_input).chain().run_if(resource_exists::<Running>))
            .add_systems(Update, poll_generation.run_if(resource_exists::<Generating>))
            .add_systems(OnExit(StudioState::Playing), stop_module);
    }
}

/// Les paquets de commandes, comme `input_thread` de `native/nitro-pc/present.c`. Le launcher ne sait rien des mods :
/// il envoie tout ce que le joueur fait, et le module (ou ses mods) décide.
const PACKET_V1: usize = 16;
const PACKET_V2: usize = 64;
/// Premier mot du paquet 2 : les touches du paquet 1 n'atteignent jamais le bit 15.
const PACKET_V2_TAG: u16 = 0x8002;

#[derive(Resource)]
struct Running {
    code: String,
    child: Child,
    stdin: Option<ChildStdin>,
    frames: SharedFrames,
    seq: u32,
    top: Handle<Image>,
    bottom: Handle<Image>,
    sent: Vec<u8>,
    /// Le module demande le curseur ; F7 le rend au joueur jusqu'à la prochaine demande.
    wanted: bool,
    released: bool,
    /// Curseur retenu en ce moment : la souris va au module, le stylet ne touche rien.
    held: bool,
    /// Cumul des mouvements de la souris et de la molette, envoyés au module.
    mouse: (i16, i16),
    wheel: i32,
    notice_seq: u32,
    /// Une nouvelle version du runtime à l'essai : confirmée après quelques secondes, rendue si le module s'arrête avant.
    trial: bool,
    started: std::time::Instant,
}

#[derive(Component)]
struct NativeScreen;

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn screen_image(images: &mut Assets<Image>) -> Handle<Image> {
    let size = Extent3d { width: W as u32, height: H as u32, depth_or_array_layers: 1 };
    let mut image = Image::new_fill(
        size,
        TextureDimension::D2,
        &[0, 0, 0, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    image.sampler = ImageSampler::nearest();
    images.add(image)
}

fn start_module(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    rom: Option<Res<ActiveRom>>,
    active: Res<ActiveGame>,
    settings: Option<Res<PlayerSettings>>,
    mut style: ResMut<RenderStyle>,
    mut layout: ResMut<ScreenLayout>,
) {
    // La spécification a remis le rendu HD : le choix du joueur passe avant. Sans format choisi : 16:9.
    if let Some(settings) = settings {
        *style = settings.chosen_render();
    }
    if layout.aspect == Aspect::Fill {
        layout.aspect = Aspect::Wide16x9;
    }
    let Some(rom) = rom else {
        error!("jeu natif lance sans rom");
        commands.set_state(StudioState::Menu);
        return;
    };
    let code = active.0.clone().unwrap_or_default().trim_start_matches("native-").to_string();
    if let Some(module) = module_path(&code) {
        launch(&mut commands, &mut images, &code, &rom.0, &module);
        return;
    }
    let state = start_job(&code, &rom.0, module_version(&code), false);
    let label = commands.spawn((Text2d::new(""), TextFont::from_font_size(10.0), TextLayout::new(Justify::Center, LineBreak::WordBoundary), bevy::text::TextBounds::new_horizontal(230.0), Engine::Main.layers(), in_game())).id();
    commands.insert_resource(Generating { code, rom: rom.0.clone(), state, label });
}

fn poll_generation(mut commands: Commands, generating: Res<Generating>, mut images: ResMut<Assets<Image>>, mut texts: Query<&mut Text2d>, mut toasts: MessageWriter<ShowToast>) {
    let (message, result) = {
        let state = generating.state.lock().unwrap();
        (state.message.clone(), state.result.clone())
    };
    if let Ok(mut text) = texts.get_mut(generating.label) {
        if text.0 != message {
            text.0 = message;
        }
    }
    let Some(result) = result else { return };
    commands.entity(generating.label).despawn();
    commands.remove_resource::<Generating>();
    match result {
        Ok(module) => launch(&mut commands, &mut images, &generating.code, &generating.rom, &module),
        Err(e) => {
            toasts.write(ShowToast(t!("This game could not be prepared: {e}", "Ce jeu n'a pas pu etre prepare : {e}")));
            commands.set_state(StudioState::Menu);
        }
    }
}

fn launch(commands: &mut Commands, images: &mut Assets<Image>, code: &str, rom: &Path, module: &Path) {
    let id = format!("native-{code}");
    let saves = crate::save::data_dir().join("saves").join(&id);
    let _ = std::fs::create_dir_all(&saves);
    let log = std::fs::File::create(saves.join("runtime.log")).map(Stdio::from).unwrap_or_else(|_| Stdio::null());

    let name = shm_name(code);
    let Some(frames) = SharedFrames::new(&name) else {
        error!("{code} : pas de memoire pour les images");
        commands.set_state(StudioState::Menu);
        return;
    };
    let mod_args = crate::mod_packs::native_args(code);
    let lang = if crate::i18n::lang() == crate::i18n::Lang::Fr { "fr" } else { "en" };
    // Un mod qui touche à la sauvegarde a la sienne : celle du jeu sans mods reste intacte
    let save = match crate::mod_packs::separate_save(code) {
        Some(ids) => saves.join(format!("backup-{ids}.sav")),
        None => saves.join("backup.sav"),
    };
    let mut command = Command::new(module);
    command.arg(rom).arg("--pipe").arg("--shm").arg(&name).arg("--save").arg(save).arg("--lang").arg(lang).args(mod_args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(log);
    if let Some(plugins) = crate::mod_packs::write_plugins(code, rom) {
        command.arg("--plugins").arg(plugins);
    }
    if std::env::var_os("NDSGS_NATIVE_MUTE").is_some() {
        command.arg("--mute");
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            error!("{code} : le module natif ne demarre pas : {e}");
            activity::record(What::GameStopped { game: code.to_string(), error: e.to_string() });
            commands.set_state(StudioState::Menu);
            return;
        }
    };

    let top = screen_image(images);
    let bottom = screen_image(images);
    let size = Vec2::new(W as f32, H as f32);
    for (engine, image) in [(Engine::Main, &top), (Engine::Sub, &bottom)] {
        commands.spawn((Sprite { image: image.clone(), custom_size: Some(size), ..default() }, NativeScreen, engine.layers(), in_game()));
    }
    let stdin = child.stdin.take();
    commands.insert_resource(Running { code: code.to_string(), child, stdin, frames, seq: 0, top, bottom, sent: Vec::new(), wanted: false, released: false, held: false, mouse: (0, 0), wheel: 0, notice_seq: 0, trial: trial_of(code).is_some(), started: std::time::Instant::now() });
}

fn show_frames(mut commands: Commands, mut running: ResMut<Running>, mut images: ResMut<Assets<Image>>, mut sprites: Query<&mut Sprite, With<NativeScreen>>, mut toasts: MessageWriter<ShowToast>) {
    if let Ok(Some(status)) = running.child.try_wait() {
        if status.success() {
            info!("jeu natif termine");
        } else {
            error!("le jeu natif s'est arrete : {status}");
            activity::record(What::GameStopped { game: running.code.clone(), error: status.to_string() });
            // Une nouvelle version du runtime qui ne démarre pas : retour à l'ancienne, gardée pour ça
            if running.trial && running.started.elapsed().as_secs() < TRIAL_SECONDS {
                if let Some((new, old)) = revert_trial(&running.code) {
                    toasts.write(ShowToast(t!("Runtime version {new} did not start ({status}): back to version {old}.", "La version {new} du runtime n'a pas demarre ({status}) : retour a la version {old}.")));
                }
            }
        }
        commands.set_state(StudioState::Menu);
        return;
    }
    if running.trial && running.started.elapsed().as_secs() >= TRIAL_SECONDS {
        running.trial = false;
        confirm_trial(&running.code);
    }
    let Header { seq, width, height, front, .. } = running.frames.header();
    let (width, height) = (width as usize, height as usize);
    if seq == running.seq || width == 0 || width > MAX_W || height == 0 || height > MAX_H {
        return;
    }
    running.seq = seq;
    let set = (front & 1) as usize;
    for (screen, handle) in [running.top.clone(), running.bottom.clone()].iter().enumerate() {
        let pixels = running.frames.screen(set, screen, width, height);
        let Some(mut image) = images.get_mut(handle) else { continue };
        let size = Extent3d { width: width as u32, height: height as u32, depth_or_array_layers: 1 };
        if image.texture_descriptor.size != size {
            image.resize(size);
            image.sampler = if height == H { ImageSampler::nearest() } else { ImageSampler::linear() };
        }
        image.data = Some(pixels);
    }
    let shown = Vec2::new(width as f32 * H as f32 / height as f32, H as f32);
    for mut sprite in &mut sprites {
        if sprite.custom_size != Some(shown) {
            sprite.custom_size = Some(shown);
        }
    }
}

/// Le curseur retenu quand le module le demande (bit 0 de l'en-tête), F7 le rend ; les messages du module en toasts.
fn cursor_and_notices(mut running: ResMut<Running>, motion: Res<AccumulatedMouseMotion>, scroll: Res<AccumulatedMouseScroll>, keys: Res<ButtonInput<KeyCode>>, screens: Res<DsScreens>, mut windows: Query<(&Window, &mut CursorOptions)>, mut toasts: MessageWriter<ShowToast>) {
    let header = running.frames.header();
    if header.notice_seq != running.notice_seq {
        running.notice_seq = header.notice_seq;
        let text = running.frames.notice();
        if !text.is_empty() {
            toasts.write(ShowToast(text));
        }
    }
    let wanted = header.flags & 1 != 0;
    if wanted && !running.wanted {
        running.released = false;
    }
    running.wanted = wanted;
    if keys.just_pressed(KeyCode::F7) {
        running.released = !running.released;
    }
    let Ok((window, mut cursor)) = windows.get_mut(screens.main_window()) else { return };
    let held = wanted && !running.released && window.focused;
    running.held = held;
    let (grab, visible) = if held { (CursorGrabMode::Locked, false) } else { (CursorGrabMode::None, true) };
    if cursor.grab_mode != grab || cursor.visible != visible {
        cursor.grab_mode = grab;
        cursor.visible = visible;
    }
    if held {
        let (x, y) = running.mouse;
        running.mouse = (x.wrapping_add(motion.delta.x.round() as i16), y.wrapping_add(motion.delta.y.round() as i16));
    }
    if window.focused {
        running.wheel = running.wheel.wrapping_add(scroll.delta.y.round() as i32);
    }
}

/// Touches du clavier : code HID (page 7 des claviers USB, `NDS_KEY_*` de `native/sdk/nds_mod.h`) et nom, celui de `KeyCode`
/// (un manifeste écrit `"KeyC"`). Les touches sont physiques : `KeyQ` est le A d'un clavier AZERTY.
const KEYS: [(KeyCode, u8, &str); 94] = {
    use KeyCode::*;
    [
        (KeyA, 0x04, "KeyA"), (KeyB, 0x05, "KeyB"), (KeyC, 0x06, "KeyC"), (KeyD, 0x07, "KeyD"), (KeyE, 0x08, "KeyE"), (KeyF, 0x09, "KeyF"),
        (KeyG, 0x0A, "KeyG"), (KeyH, 0x0B, "KeyH"), (KeyI, 0x0C, "KeyI"), (KeyJ, 0x0D, "KeyJ"), (KeyK, 0x0E, "KeyK"), (KeyL, 0x0F, "KeyL"),
        (KeyM, 0x10, "KeyM"), (KeyN, 0x11, "KeyN"), (KeyO, 0x12, "KeyO"), (KeyP, 0x13, "KeyP"), (KeyQ, 0x14, "KeyQ"), (KeyR, 0x15, "KeyR"),
        (KeyS, 0x16, "KeyS"), (KeyT, 0x17, "KeyT"), (KeyU, 0x18, "KeyU"), (KeyV, 0x19, "KeyV"), (KeyW, 0x1A, "KeyW"), (KeyX, 0x1B, "KeyX"),
        (KeyY, 0x1C, "KeyY"), (KeyZ, 0x1D, "KeyZ"),
        (Digit1, 0x1E, "Digit1"), (Digit2, 0x1F, "Digit2"), (Digit3, 0x20, "Digit3"), (Digit4, 0x21, "Digit4"), (Digit5, 0x22, "Digit5"),
        (Digit6, 0x23, "Digit6"), (Digit7, 0x24, "Digit7"), (Digit8, 0x25, "Digit8"), (Digit9, 0x26, "Digit9"), (Digit0, 0x27, "Digit0"),
        (Enter, 0x28, "Enter"), (Escape, 0x29, "Escape"), (Backspace, 0x2A, "Backspace"), (Tab, 0x2B, "Tab"), (Space, 0x2C, "Space"),
        (Minus, 0x2D, "Minus"), (Equal, 0x2E, "Equal"), (BracketLeft, 0x2F, "BracketLeft"), (BracketRight, 0x30, "BracketRight"),
        (Backslash, 0x31, "Backslash"), (Semicolon, 0x33, "Semicolon"), (Quote, 0x34, "Quote"), (Backquote, 0x35, "Backquote"),
        (Comma, 0x36, "Comma"), (Period, 0x37, "Period"), (Slash, 0x38, "Slash"), (CapsLock, 0x39, "CapsLock"),
        (F1, 0x3A, "F1"), (F2, 0x3B, "F2"), (F3, 0x3C, "F3"), (F4, 0x3D, "F4"), (F5, 0x3E, "F5"), (F6, 0x3F, "F6"), (F7, 0x40, "F7"),
        (F8, 0x41, "F8"), (F9, 0x42, "F9"), (F10, 0x43, "F10"), (F11, 0x44, "F11"), (F12, 0x45, "F12"),
        (Insert, 0x49, "Insert"), (Home, 0x4A, "Home"), (PageUp, 0x4B, "PageUp"), (Delete, 0x4C, "Delete"), (End, 0x4D, "End"),
        (PageDown, 0x4E, "PageDown"), (ArrowRight, 0x4F, "ArrowRight"), (ArrowLeft, 0x50, "ArrowLeft"), (ArrowDown, 0x51, "ArrowDown"),
        (ArrowUp, 0x52, "ArrowUp"),
        (Numpad1, 0x59, "Numpad1"), (Numpad2, 0x5A, "Numpad2"), (Numpad3, 0x5B, "Numpad3"), (Numpad4, 0x5C, "Numpad4"), (Numpad5, 0x5D, "Numpad5"),
        (Numpad6, 0x5E, "Numpad6"), (Numpad7, 0x5F, "Numpad7"), (Numpad8, 0x60, "Numpad8"), (Numpad9, 0x61, "Numpad9"), (Numpad0, 0x62, "Numpad0"),
        (IntlBackslash, 0x64, "IntlBackslash"),
        (ControlLeft, 0xE0, "ControlLeft"), (ShiftLeft, 0xE1, "ShiftLeft"), (AltLeft, 0xE2, "AltLeft"), (SuperLeft, 0xE3, "SuperLeft"),
        (ControlRight, 0xE4, "ControlRight"), (ShiftRight, 0xE5, "ShiftRight"), (AltRight, 0xE6, "AltRight"), (SuperRight, 0xE7, "SuperRight"),
    ]
};

/// Code HID d'une touche, par sa place.
fn hid_usage(key: KeyCode) -> Option<u8> {
    KEYS.iter().find(|(k, _, _)| *k == key).map(|(_, hid, _)| *hid)
}

/// Code HID d'une touche nommée comme `KeyCode` (`"KeyC"`), pour les manifestes des mods.
pub fn hid_of_name(name: &str) -> Option<u8> {
    KEYS.iter().find(|(_, _, n)| *n == name).map(|(_, hid, _)| *hid)
}

/// Ce qu'on montre au joueur pour un code HID : la lettre ou le nom de la touche.
pub fn key_label(hid: u8) -> String {
    match KEYS.iter().find(|(_, h, _)| *h == hid) {
        Some((_, _, name)) => name.strip_prefix("Key").or_else(|| name.strip_prefix("Digit")).unwrap_or(name).to_string(),
        None => format!("#{hid}"),
    }
}

/// La touche que le joueur vient d'enfoncer, en code HID (choix d'une touche dans les réglages d'un mod).
pub fn hid_pressed(keys: &ButtonInput<KeyCode>) -> Option<u8> {
    keys.get_just_pressed().find_map(|k| hid_usage(*k))
}

/// Le paquet 2 : les champs du paquet 1, puis la souris, la fenêtre et le clavier entier (`input_thread` de `present.c`).
#[allow(clippy::too_many_arguments)]
fn packet_v2(keys: u16, touch: (u16, u16, u16), ds_width: u16, scale: u16, mouse: (i16, i16), window: u16, buttons: u32, wheel: i32, keyboard: &[u32; 8]) -> Vec<u8> {
    let mut packet = Vec::with_capacity(PACKET_V2);
    for word in [PACKET_V2_TAG, keys, touch.0, touch.1, touch.2, ds_width, scale, mouse.0 as u16, mouse.1 as u16, window] {
        packet.extend_from_slice(&word.to_le_bytes());
    }
    packet.extend_from_slice(&buttons.to_le_bytes());
    packet.extend_from_slice(&wheel.to_le_bytes());
    packet.resize(32, 0);
    for word in keyboard {
        packet.extend_from_slice(&word.to_le_bytes());
    }
    packet
}

#[allow(clippy::too_many_arguments)]
fn send_input(mut running: ResMut<Running>, input: Res<DsInput>, mouse_buttons: Res<ButtonInput<MouseButton>>, key_input: Res<ButtonInput<KeyCode>>, stylus: Res<Stylus>, places: Res<ScreenPlaces>, style: Res<RenderStyle>, windows: Query<&Window>, screens: Res<DsScreens>) {
    // Bits du registre KEYINPUT du DS, puis X et Y
    const ORDER: [DsButton; 12] = [
        DsButton::A,
        DsButton::B,
        DsButton::Select,
        DsButton::Start,
        DsButton::Right,
        DsButton::Left,
        DsButton::Up,
        DsButton::Down,
        DsButton::R,
        DsButton::L,
        DsButton::X,
        DsButton::Y,
    ];
    let mut keys = 0u16;
    for (bit, button) in ORDER.iter().enumerate() {
        if input.pressed(*button) {
            keys |= 1 << bit;
        }
    }
    // Curseur retenu : il n'est nulle part, il ne touche donc pas l'écran
    let touch = stylus.touch().filter(|p| !running.held && p.x >= 0.0 && p.y >= 0.0 && p.x < W as f32 && p.y < H as f32);
    let (down, x, y) = touch.map_or((0u16, 0u16, 0u16), |p| (1, p.x as u16, p.y as u16));
    // La définition suit la fenêtre : un pixel d'image par pixel d'écran, ou 1 en rendu DS
    let mut ds_width = W as u16;
    let mut scale = 1u16;
    for place in [Screen::Top, Screen::Bottom].into_iter().filter_map(|screen| places.get(screen)) {
        ds_width = ds_width.max(place.ds_width.round() as u16);
        if *style == RenderStyle::Hd {
            let factor = windows.get(place.window).map_or(1.0, |window| window.scale_factor());
            scale = scale.max((place.scale * factor).ceil().clamp(2.0, MAX_SCALE) as u16);
        }
    }
    // La 2D est affinée par puissances de 2 : 3 passe à 4, l'image réduite reste régulière
    if scale == 3 {
        scale = 4;
    }
    if running.frames.header().protocol >= 2 {
        let mut keyboard = [0u32; 8];
        for key in key_input.get_pressed() {
            if let Some(code) = hid_usage(*key) {
                keyboard[code as usize >> 5] |= 1 << (code & 31);
            }
        }
        let mut buttons = 0u32;
        for (bit, button) in [MouseButton::Left, MouseButton::Right, MouseButton::Middle].into_iter().enumerate() {
            if mouse_buttons.pressed(button) {
                buttons |= 1 << bit;
            }
        }
        let focused = windows.get(screens.main_window()).is_ok_and(|w| w.focused);
        let window = u16::from(running.held) | u16::from(focused) << 1;
        let packet = packet_v2(keys, (down, x, y), ds_width, scale, running.mouse, window, buttons, running.wheel, &keyboard);
        if packet != running.sent {
            if let Some(stdin) = running.stdin.as_mut() {
                let _ = stdin.write_all(&packet).and_then(|_| stdin.flush());
            }
            running.sent = packet;
        }
        return;
    }
    // Module d'avant le protocole 2
    let mut packet = [0u8; PACKET_V1];
    packet[0..2].copy_from_slice(&keys.to_le_bytes());
    packet[2..4].copy_from_slice(&down.to_le_bytes());
    packet[4..6].copy_from_slice(&x.to_le_bytes());
    packet[6..8].copy_from_slice(&y.to_le_bytes());
    packet[8..10].copy_from_slice(&ds_width.to_le_bytes());
    packet[10..12].copy_from_slice(&scale.to_le_bytes());
    packet[12..14].copy_from_slice(&running.mouse.0.to_le_bytes());
    packet[14..16].copy_from_slice(&running.mouse.1.to_le_bytes());
    if packet[..] != running.sent[..] {
        running.sent = packet.to_vec();
        if let Some(stdin) = running.stdin.as_mut() {
            let _ = stdin.write_all(&packet).and_then(|_| stdin.flush());
        }
    }
}

fn stop_module(mut commands: Commands, mut cursors: Query<&mut CursorOptions>) {
    for mut cursor in &mut cursors {
        if cursor.grab_mode != CursorGrabMode::None || !cursor.visible {
            cursor.grab_mode = CursorGrabMode::None;
            cursor.visible = true;
        }
    }
    commands.remove_resource::<Running>();
    commands.remove_resource::<Generating>();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Disposition lue par `input_thread` de `present.c` : mots de 16 bits, puis boutons, molette et clavier.
    #[test]
    fn le_paquet_2_suit_present_c() {
        let mut keyboard = [0u32; 8];
        let c = hid_usage(KeyCode::KeyC).unwrap() as usize;
        keyboard[c >> 5] |= 1 << (c & 31);
        let p = packet_v2(0x0201, (1, 100, 50), 341, 4, (-3, 7), 3, 5, -2, &keyboard);
        assert_eq!(p.len(), PACKET_V2);
        let word = |i: usize| u16::from_le_bytes([p[2 * i], p[2 * i + 1]]);
        assert_eq!([word(0), word(1), word(2), word(3), word(4), word(5), word(6)], [0x8002, 0x0201, 1, 100, 50, 341, 4]);
        assert_eq!([word(7) as i16, word(8) as i16], [-3, 7]);
        assert_eq!(word(9), 3);
        assert_eq!(u32::from_le_bytes(p[20..24].try_into().unwrap()), 5);
        assert_eq!(i32::from_le_bytes(p[24..28].try_into().unwrap()), -2);
        // C = 0x06 : bit 6 du premier mot du clavier, à l'octet 32
        assert_eq!(c, 0x06);
        assert_eq!(p[32], 1 << 6);
        // les touches du paquet 1 (12 bits du DS) ne prennent jamais la valeur du marqueur
        assert!(PACKET_V2_TAG > 0x0FFF);
    }

    #[test]
    fn les_codes_hid_sont_ceux_du_sdk() {
        assert_eq!(hid_usage(KeyCode::KeyA), Some(0x04));
        assert_eq!(hid_usage(KeyCode::KeyZ), Some(0x1D));
        assert_eq!(hid_usage(KeyCode::Digit0), Some(0x27));
        assert_eq!(hid_usage(KeyCode::F12), Some(0x45));
        assert_eq!(hid_usage(KeyCode::ArrowUp), Some(0x52));
        assert_eq!(hid_usage(KeyCode::ShiftLeft), Some(0xE1));
    }

    /// Le jeu ouvre l'objet par son nom (`os_shm_open`) et écrit l'en-tête que le launcher lit ; `drop` détruit le nom.
    #[cfg(unix)]
    #[test]
    fn la_memoire_partagee_est_vue_des_deux_cotes_puis_detruite() {
        let name = shm_name("TEST");
        assert!(name.len() <= 31 && name.starts_with('/'));
        let frames = SharedFrames::new(&name).expect("shm_open + ftruncate + mmap");
        assert_eq!(frames.header(), Header::default());
        let c_name = std::ffi::CString::new(name.clone()).unwrap();
        // SAFETY: côté jeu : ouverture par le nom, taille lue par fstat, comme os_posix.c.
        unsafe {
            let fd = libc::shm_open(c_name.as_ptr(), libc::O_RDWR, 0);
            assert!(fd >= 0);
            let mut st: libc::stat = std::mem::zeroed();
            assert_eq!(libc::fstat(fd, &mut st), 0);
            assert!(st.st_size as usize >= SHM_BYTES);
            let p = libc::mmap(std::ptr::null_mut(), st.st_size as usize, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, fd, 0);
            libc::close(fd);
            assert_ne!(p, libc::MAP_FAILED);
            let header = p as *mut u32;
            header.write_volatile(7);
            header.add(1).write_volatile(256);
            // Le dernier octet de la mémoire des images
            (p as *mut u8).add(SHM_BYTES - 1).write_volatile(0xAB);
            assert_eq!((frames.header().seq, frames.header().width), (7, 256));
            assert_eq!(*frames.view.add(SHM_BYTES - 1), 0xAB);
            libc::munmap(p, st.st_size as usize);
        }
        drop(frames);
        // SAFETY: le nom n'existe plus.
        assert!(unsafe { libc::shm_open(c_name.as_ptr(), libc::O_RDWR, 0) } < 0);
    }

    #[test]
    fn l_index_est_lisible_et_donne_l_etat_des_jeux() {
        let index = index();
        assert!(index.games.len() > 50);
        let tom = game_state("AJTP").expect("Tom et Jerry");
        assert_eq!(tom.play, "playable");
        assert!(tom.code.unwrap() > 50.0 && tom.data.unwrap() > 50.0 && tom.c.unwrap() > 0.0);
        assert!(is_experimental("BW7E") && !is_experimental("AL7E"));
    }
}
