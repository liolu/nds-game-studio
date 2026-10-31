//! Jeux DS natifs : décompilation + runtime nitro-pc, sans émulateur.
//!
//! Chaque jeu est un programme généré sur le PC du joueur, depuis sa rom : il contient le code du jeu et n'est donc
//! jamais publié (voir `native/README.md`). Le launcher le prépare en tâche de fond, un jeu à la fois, et Jouer n'est
//! proposé qu'une fois le module prêt. Les outils (traducteur, compilateur, runtime, recettes) sont livrés à côté du
//! studio, dans `native-toolchain` ; sinon ils sont téléchargés. Un `native/<code>.exe` à côté du studio passe avant.
//!
//! Le jeu tourne dans son propre processus 32 bits, car son code suppose des pointeurs 32 bits, et reçoit
//! la rom de l'utilisateur en argument. Il envoie ses deux écrans par sa sortie standard et lit les commandes
//! sur son entrée standard ; le moteur les affiche sur les écrans habituels (fenêtres liées ou séparées, F8, F9).

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
use crate::screens::Engine;
use crate::studio::{in_game, ActiveGame, ActiveRom, AppGameExt, StudioState};

const W: usize = 256;
const H: usize = 192;
/// Une image du jeu : les deux écrans en RGB 555 sur 16 bits.
const FRAME_BYTES: usize = W * H * 2 * 2;

/// Le dossier `native` à côté de l'exécutable.
pub fn native_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("NDSGS_NATIVE_DIR") {
        return PathBuf::from(dir);
    }
    let exe = std::env::current_exe().unwrap_or_default();
    exe.parent().unwrap_or(Path::new(".")).join("native")
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

/// Ce jeu se lance mais n'a pas été essayé jusqu'en jeu, ou ne marche pas encore.
pub fn is_experimental(code: &str) -> bool {
    index().games.get(code).is_some_and(|g| g.status != "ok")
}

fn generated_path(code: &str) -> PathBuf {
    crate::save::data_dir().join("native").join(format!("{code}-{}.exe", index().toolchain))
}

/// Le module à lancer s'il existe déjà.
fn module_path(code: &str) -> Option<PathBuf> {
    [native_dir().join(format!("{code}.exe")), generated_path(code)].into_iter().find(|p| p.exists())
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
            Preparation::NotStarted if index().games.get(code).is_some_and(|g| all || g.status == "ok") => result.waiting += 1,
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

/// Télécharge et décompresse la boîte à outils si elle n'est pas là ; renvoie son dossier.
fn ensure_toolchain(state: &Mutex<Generation>) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().unwrap_or_default();
    let bundled = exe.parent().unwrap_or(Path::new(".")).join("native-toolchain");
    if bundled.join("python").join("python.exe").exists() {
        return Ok(bundled);
    }
    let version = index().toolchain;
    let dir = crate::save::data_dir().join("native-toolchain").join(version.to_string());
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
        let tls = ureq::tls::TlsConfig::builder().provider(ureq::tls::TlsProvider::NativeTls).build();
        let config = ureq::Agent::config_builder().tls_config(tls).build();
        let url = format!("{TOOLCHAIN_URL}/native-toolchain-{version}.zip");
        let mut response = ureq::Agent::new_with_config(config).get(&url).call().map_err(|e| format!("{url} : {e}"))?;
        let mut body = response.body_mut().as_reader();
        let mut file = std::fs::File::create(&archive).map_err(|e| e.to_string())?;
        std::io::copy(&mut body, &mut file).map_err(|e| e.to_string())?;
    }
    say(state, tr("Installing the tools...", "Installation des outils...").to_string());
    let file = std::fs::File::open(&archive).map_err(|e| e.to_string())?;
    zip::ZipArchive::new(file).and_then(|mut z| z.extract(&dir)).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&archive);
    std::fs::write(ready, "").map_err(|e| e.to_string())?;
    Ok(dir)
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
    let tools = ensure_toolchain(state)?;
    say(state, tr("Preparing the game from your rom (about a minute, only once)...", "Preparation du jeu depuis votre rom (environ une minute, une seule fois)...").to_string());
    let out = crate::save::data_dir().join("native").join(format!("build-{code}"));
    let mut command = Command::new(tools.join("python").join("python.exe"));
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
    let built = out.join(format!("{code}.exe"));
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
        // Le runtime et les modules sont des programmes Windows
        let mut codes: std::collections::BTreeSet<String> = if cfg!(windows) { index().games.keys().cloned().collect() } else { Default::default() };
        for entry in std::fs::read_dir(native_dir()).into_iter().flatten().flatten() {
            let path = entry.path();
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")) && stem.len() == 4 {
                codes.insert(stem.to_string());
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
    latest: Arc<Mutex<Option<Vec<u8>>>>,
    top: Handle<Image>,
    bottom: Handle<Image>,
    sent: [u8; 8],
}

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

fn start_module(mut commands: Commands, mut images: ResMut<Assets<Image>>, rom: Option<Res<ActiveRom>>, active: Res<ActiveGame>) {
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

    let mut command = Command::new(module);
    command.arg(rom).arg("--pipe").arg("--save").arg(saves.join("backup.sav")).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(log);
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

    let latest = Arc::new(Mutex::new(None));
    if let Some(mut stdout) = child.stdout.take() {
        let latest = latest.clone();
        std::thread::spawn(move || {
            let mut raw = vec![0u8; FRAME_BYTES];
            while stdout.read_exact(&mut raw).is_ok() {
                let rgba: Vec<u8> = raw
                    .chunks_exact(2)
                    .flat_map(|p| {
                        let c = u16::from_le_bytes([p[0], p[1]]) as u32;
                        let scale = |v: u32| ((v & 31) * 255 / 31) as u8;
                        [scale(c), scale(c >> 5), scale(c >> 10), 255]
                    })
                    .collect();
                *latest.lock().unwrap() = Some(rgba);
            }
        });
    }

    let top = screen_image(images);
    let bottom = screen_image(images);
    let size = Vec2::new(W as f32, H as f32);
    for (engine, image) in [(Engine::Main, &top), (Engine::Sub, &bottom)] {
        commands.spawn((Sprite { image: image.clone(), custom_size: Some(size), ..default() }, engine.layers(), in_game()));
    }
    let stdin = child.stdin.take();
    commands.insert_resource(Running { code: code.to_string(), child, stdin, latest, top, bottom, sent: [0xFF; 8] });
}

fn show_frames(mut commands: Commands, mut running: ResMut<Running>, mut images: ResMut<Assets<Image>>) {
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
    let Some(rgba) = running.latest.lock().unwrap().take() else { return };
    let (top, bottom) = rgba.split_at(W * H * 4);
    for (handle, pixels) in [(running.top.clone(), top), (running.bottom.clone(), bottom)] {
        if let Some(mut image) = images.get_mut(&handle) {
            image.data = Some(pixels.to_vec());
        }
    }
}

fn send_input(mut running: ResMut<Running>, input: Res<DsInput>, stylus: Res<Stylus>) {
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
    let mut packet = [0u8; 8];
    packet[0..2].copy_from_slice(&keys.to_le_bytes());
    packet[2..4].copy_from_slice(&down.to_le_bytes());
    packet[4..6].copy_from_slice(&x.to_le_bytes());
    packet[6..8].copy_from_slice(&y.to_le_bytes());
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
