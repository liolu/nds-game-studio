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
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::activity::{self, What};
use crate::i18n::{t, tr};
use crate::input::{DsButton, DsInput, Stylus};
use crate::link::ShowToast;
use crate::screens::{Aspect, Engine, RenderStyle, Screen, ScreenLayout, ScreenPlaces};
use crate::settings::PlayerSettings;
use crate::studio::{in_game, ActiveGame, ActiveRom, AppGameExt, StudioState};

const W: usize = 256;
const H: usize = 192;
/// Mémoire d'images, comme `native/nitro-pc/present.c` : un en-tête, puis deux jeux de deux écrans en RGBA.
const MAX_W: usize = 2688;
const MAX_H: usize = 1152;
const SHM_HEADER: usize = 4096;
const SHM_BYTES: usize = SHM_HEADER + 4 * MAX_W * MAX_H * 4;
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

    /// (numéro de l'image, largeur, hauteur, jeu d'images terminé)
    fn header(&self) -> [u32; 4] {
        // SAFETY: l'en-tête est dans la mémoire ; le jeu l'écrit, d'où la lecture volatile.
        std::array::from_fn(|i| unsafe { std::ptr::read_volatile((self.view as *const u32).add(i)) })
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

#[derive(serde::Deserialize)]
struct Index {
    toolchain: u32,
    games: std::collections::BTreeMap<String, IndexGame>,
}

#[derive(serde::Deserialize)]
struct IndexGame {
    status: String,
    #[serde(default)]
    play: String,
    code: Option<f32>,
    c: Option<f32>,
    data: Option<f32>,
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

/// Lu une fois : le launcher le consulte pour chaque jeu de la vue.
fn index() -> &'static Index {
    static PARSED: OnceLock<Index> = OnceLock::new();
    PARSED.get_or_init(|| serde_json::from_str(INDEX).expect("native/games/index.json"))
}

/// Ce jeu a un module natif, mais pas sur ce système.
pub fn windows_only(code: &str) -> bool {
    !CAN_GENERATE && index().games.contains_key(code)
}

/// Ce jeu se lance mais n'a pas été essayé jusqu'en jeu, ou ne marche pas encore.
pub fn is_experimental(code: &str) -> bool {
    index().games.get(code).is_some_and(|g| g.status != "ok")
}

fn generated_path(code: &str) -> PathBuf {
    crate::save::data_dir().join("native").join(format!("{code}-{}{}", index().toolchain, std::env::consts::EXE_SUFFIX))
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

/// Où en sont les jeux installés : prêts, en attente de leur tour, en erreur.
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

/// Lance la génération de ce jeu, ou renvoie celle en cours. Une génération ratée est relancée.
fn start_job(code: &str, rom: &Path) -> Arc<Mutex<Generation>> {
    let mut jobs = JOBS.lock().unwrap();
    let map = jobs.get_or_insert_with(Default::default);
    if let Some(job) = map.get(code) {
        if !matches!(job.lock().unwrap().result, Some(Err(_))) {
            return job.clone();
        }
    }
    let state = Arc::new(Mutex::new(Generation { started: Some(std::time::Instant::now()), ..Default::default() }));
    map.insert(code.to_string(), state.clone());
    let (thread_state, code, rom) = (state.clone(), code.to_string(), rom.to_path_buf());
    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let result = generate(&code, &rom, &thread_state);
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

/// Le joueur demande ce jeu.
pub fn prepare(code: &str, rom: &Path) {
    start_job(code, rom);
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

/// Prépare le prochain jeu sans module : un seul à la fois, pour laisser le PC tranquille. Sans `all`, seulement ceux
/// qui marchent ; les expérimentaux attendent qu'on les demande.
pub fn prepare_next<'a>(roms: impl Iterator<Item = (&'a str, &'a Path)>, all: bool) {
    if !CAN_GENERATE {
        return;
    }
    let running = JOBS.lock().unwrap().as_ref().is_some_and(|j| j.values().any(|job| job.lock().unwrap().result.is_none()));
    if running {
        return;
    }
    let games = &index().games;
    for (code, rom) in roms {
        if games.get(code).is_some_and(|g| all || g.status == "ok") && matches!(preparation(code), Preparation::NotStarted) {
            start_job(code, rom);
            return;
        }
    }
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
/// faite depuis le dépôt), ou `native-toolchain` à côté de l'exécutable.
fn local_toolchain() -> Option<PathBuf> {
    let exe = std::env::current_exe().unwrap_or_default();
    let beside = exe.parent().unwrap_or(Path::new(".")).join("native-toolchain");
    [std::env::var_os("NDSGS_TOOLCHAIN_DIR").map(PathBuf::from), Some(beside)].into_iter().flatten().find(|dir| toolchain_python(dir).exists())
}

/// Nom de l'archive de la boîte à outils : une par plateforme.
fn toolchain_asset(version: u32) -> String {
    if cfg!(target_os = "macos") {
        format!("native-toolchain-{version}-macos-{MACOS_ARCH}.zip")
    } else {
        format!("native-toolchain-{version}.zip")
    }
}

/// Télécharge et décompresse la boîte à outils si elle n'est pas là ; renvoie son dossier.
fn ensure_toolchain(state: &Mutex<Generation>) -> Result<PathBuf, String> {
    if let Some(dir) = local_toolchain() {
        return Ok(dir);
    }
    let version = index().toolchain;
    let folder = if cfg!(target_os = "macos") { format!("{version}-macos-{MACOS_ARCH}") } else { version.to_string() };
    let dir = crate::save::data_dir().join("native-toolchain").join(folder);
    let ready = dir.join("installed");
    if ready.exists() {
        return Ok(dir);
    }
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let archive = dir.join("toolchain.zip");
    if let Some(local) = std::env::var_os("NDSGS_TOOLCHAIN_ZIP") {
        std::fs::copy(local, &archive).map_err(|e| e.to_string())?;
    } else {
        say(state, tr("Downloading the tools (only once)...", "Telechargement des outils (une seule fois)...").to_string());
        download(&format!("{TOOLCHAIN_URL}/{}", toolchain_asset(version)), &archive)?;
    }
    say(state, tr("Installing the tools...", "Installation des outils...").to_string());
    let file = std::fs::File::open(&archive).map_err(|e| e.to_string())?;
    zip::ZipArchive::new(file).and_then(|mut z| z.extract(&dir)).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&archive);
    std::fs::write(ready, "").map_err(|e| e.to_string())?;
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

fn generate(code: &str, rom: &Path, state: &Mutex<Generation>) -> Result<PathBuf, String> {
    check_compiler()?;
    let tools = ensure_toolchain(state)?;
    say(state, tr("Preparing the game from your rom (about a minute, only once)...", "Preparation du jeu depuis votre rom (environ une minute, une seule fois)...").to_string());
    let out = crate::save::data_dir().join("native").join(format!("build-{code}"));
    let mut command = Command::new(toolchain_python(&tools));
    command
        .arg("-u")
        .arg(tools.join("native").join("tools").join("native_build.py"))
        .arg("--recipe")
        .arg(tools.join("native").join("games").join(format!("{code}.json.gz")))
        .arg("--rom")
        .arg(rom)
        .arg("--out")
        .arg(&out)
        .env("PYTHONPATH", tools.join("python"))
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
    let target = generated_path(code);
    std::fs::rename(&built, &target).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&out);
    Ok(target)
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
        for code in codes {
            let spec = format!("id = \"native-{code}\"\nname = \"{code}\"\ngame_code = \"{code}\"\n");
            app.add_ds_game(&spec, start_module);
        }
        app.add_systems(Update, (show_frames, send_input).run_if(resource_exists::<Running>))
            .add_systems(Update, poll_generation.run_if(resource_exists::<Generating>))
            .add_systems(OnExit(StudioState::Playing), stop_module);
    }
}

#[derive(Resource)]
struct Running {
    code: String,
    child: Child,
    stdin: Option<ChildStdin>,
    frames: SharedFrames,
    seq: u32,
    top: Handle<Image>,
    bottom: Handle<Image>,
    sent: [u8; 12],
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
    let state = start_job(&code, &rom.0);
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
    let mut command = Command::new(module);
    command.arg(rom).arg("--pipe").arg("--shm").arg(&name).arg("--save").arg(saves.join("backup.sav")).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(log);
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
    commands.insert_resource(Running { code: code.to_string(), child, stdin, frames, seq: 0, top, bottom, sent: [0xFF; 12] });
}

fn show_frames(mut commands: Commands, mut running: ResMut<Running>, mut images: ResMut<Assets<Image>>, mut sprites: Query<&mut Sprite, With<NativeScreen>>) {
    if let Ok(Some(status)) = running.child.try_wait() {
        if status.success() {
            info!("jeu natif termine");
        } else {
            error!("le jeu natif s'est arrete : {status}");
            activity::record(What::GameStopped { game: running.code.clone(), error: status.to_string() });
        }
        commands.set_state(StudioState::Menu);
        return;
    }
    let [seq, width, height, front] = running.frames.header();
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

fn send_input(mut running: ResMut<Running>, input: Res<DsInput>, stylus: Res<Stylus>, places: Res<ScreenPlaces>, style: Res<RenderStyle>, windows: Query<&Window>) {
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
    let touch = stylus.touch().filter(|p| p.x >= 0.0 && p.y >= 0.0 && p.x < W as f32 && p.y < H as f32);
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
    let mut packet = [0u8; 12];
    packet[0..2].copy_from_slice(&keys.to_le_bytes());
    packet[2..4].copy_from_slice(&down.to_le_bytes());
    packet[4..6].copy_from_slice(&x.to_le_bytes());
    packet[6..8].copy_from_slice(&y.to_le_bytes());
    packet[8..10].copy_from_slice(&ds_width.to_le_bytes());
    packet[10..12].copy_from_slice(&scale.to_le_bytes());
    if packet != running.sent {
        running.sent = packet;
        if let Some(stdin) = running.stdin.as_mut() {
            let _ = stdin.write_all(&packet).and_then(|_| stdin.flush());
        }
    }
}

fn stop_module(mut commands: Commands) {
    commands.remove_resource::<Running>();
    commands.remove_resource::<Generating>();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Le jeu ouvre l'objet par son nom (`os_shm_open`) et écrit l'en-tête que le launcher lit ; `drop` détruit le nom.
    #[cfg(unix)]
    #[test]
    fn la_memoire_partagee_est_vue_des_deux_cotes_puis_detruite() {
        let name = shm_name("TEST");
        assert!(name.len() <= 31 && name.starts_with('/'));
        let frames = SharedFrames::new(&name).expect("shm_open + ftruncate + mmap");
        assert_eq!(frames.header(), [0; 4]);
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
            assert_eq!(frames.header(), [7, 256, 0, 0]);
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
