//! Outils de développement : voir le code du jeu (F5) et changer le jeu en changeant le code.
//!
//! - **F5** ouvre (ou ferme) la fenêtre « Code » : un éditeur des fichiers du jeu en cours (ou du
//!   studio dans le menu), avec l'état de la compilation et ses erreurs. **Ctrl+S** enregistre ; un
//!   fichier changé par un autre éditeur est relu s'il n'a pas de modification en cours.
//! - **Rechargement** : quand un fichier du jeu ou du moteur est enregistré, le studio recompile en
//!   arrière-plan. Si ça compile, il se relance tout seul sur le même jeu, avec les fenêtres à la
//!   même place (et la fenêtre Code ouverte si elle l'était). Sinon les erreurs s'affichent dans la
//!   fenêtre Code et le jeu continue de tourner.
//!
//! Ne marche qu'avec le dépôt sur le disque (compilation locale) : une version installée n'a pas
//! les sources.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use bevy::camera::RenderTarget;
use bevy::ecs::system::NonSendMarker;
use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::input_focus::InputFocus;
use bevy::text::{EditableText, LineBreak, TextCursorStyle, TextEdit, TextLayout};
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::Pressed;
use bevy::ui_widgets::{Activate, Button, TextInput};
use bevy::window::{WindowRef, WindowResolution};
use bevy::winit::WINIT_WINDOWS;

use crate::i18n::{t, tr};
use crate::link::ShowToast;
use crate::screens::{Aspect, DsScreens, ScreenLayout};
use crate::studio::{ActiveGame, AutoStart};

/// Intervalle entre deux regards sur les fichiers (secondes).
const WATCH_EVERY: f64 = 0.5;

// ─────────────────────────────────────────────────────────────────────────
//  Options de lancement (aussi utilisées pour se relancer après une recompilation)
//
//  nds-game-studio [jeu] [--at X,Y] [--size L,H] [--split] [--aspect fill|4:3|16:9|16:10] [--code]
// ─────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LaunchOptions {
    pub game: Option<String>,
    /// Coin de la fenêtre principale (pixels physiques).
    pub at: Option<IVec2>,
    /// Taille de la fenêtre principale (pixels logiques).
    pub size: Option<Vec2>,
    pub split: bool,
    pub aspect: Option<Aspect>,
    pub code: bool,
}

fn pair<T: std::str::FromStr>(text: &str) -> Option<(T, T)> {
    let (a, b) = text.split_once(',')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

fn aspect_name(aspect: Aspect) -> &'static str {
    match aspect {
        Aspect::Ds => "4:3",
        Aspect::Wide16x9 => "16:9",
        Aspect::Wide16x10 => "16:10",
        Aspect::Fill => "fill",
    }
}

impl LaunchOptions {
    pub fn from_args() -> Self {
        Self::parse(std::env::args().skip(1))
    }

    pub fn parse(args: impl IntoIterator<Item = String>) -> Self {
        let mut options = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--at" => options.at = args.next().and_then(|v| pair::<i32>(&v)).map(|(x, y)| IVec2::new(x, y)),
                "--size" => options.size = args.next().and_then(|v| pair::<f32>(&v)).map(|(w, h)| Vec2::new(w, h)),
                "--split" => options.split = true,
                "--code" => options.code = true,
                "--aspect" => options.aspect = args.next().and_then(|v| Aspect::ALL.into_iter().find(|a| aspect_name(*a) == v)),
                other if !other.starts_with("--") => options.game = Some(other.to_string()),
                _ => {}
            }
        }
        options
    }

    pub fn to_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        args.extend(self.game.clone());
        if let Some(at) = self.at {
            args.extend(["--at".into(), format!("{},{}", at.x, at.y)]);
        }
        if let Some(size) = self.size {
            args.extend(["--size".into(), format!("{},{}", size.x.round(), size.y.round())]);
        }
        if self.split {
            args.push("--split".into());
        }
        if let Some(aspect) = self.aspect {
            args.extend(["--aspect".into(), aspect_name(aspect).into()]);
        }
        if self.code {
            args.push("--code".into());
        }
        args
    }

    /// Place et taille de départ de la fenêtre principale.
    pub fn apply_to_window(&self, window: &mut Window) {
        if let Some(at) = self.at {
            window.position = WindowPosition::At(at);
        }
        if let Some(size) = self.size {
            window.resolution = WindowResolution::new(size.x.max(128.0) as u32, size.y.max(128.0) as u32);
        }
    }

    /// Ce qui se règle une fois l'application créée.
    pub fn apply_to_app(&self, app: &mut App) {
        if let Some(game) = &self.game {
            app.insert_resource(AutoStart(game.clone()));
        }
        let mut layout = ScreenLayout::default();
        layout.merged = !self.split;
        if let Some(aspect) = self.aspect {
            layout.aspect = aspect;
        }
        app.insert_resource(layout);
        if self.code {
            app.insert_resource(OpenCodeAtStart);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
//  Sources, surveillance, recompilation
// ─────────────────────────────────────────────────────────────────────────

/// Racine du dépôt (là où se trouve le Cargo.toml du workspace), si elle est sur ce PC.
fn source_root() -> Option<PathBuf> {
    let engine = PathBuf::from(option_env!("CARGO_MANIFEST_DIR")?);
    let root = engine.parent()?.parent()?.to_path_buf();
    (root.join("Cargo.toml").is_file() && root.join("app").join("src").is_dir()).then_some(root)
}

#[derive(Clone, Debug, PartialEq)]
enum BuildState {
    Idle,
    Building,
    Failed(String),
    Done,
}

#[derive(Resource)]
struct Dev {
    root: Option<PathBuf>,
    /// Dernière empreinte des fichiers surveillés.
    stamp: Option<u64>,
    /// Changement vu, recompilation dès que les fichiers ne bougent plus.
    pending: bool,
    last_watch: f64,
    build: Arc<Mutex<BuildState>>,
}

/// Réglages des outils (menu Options > Développement).
#[derive(Resource)]
pub struct DevSettings {
    /// Recompiler et relancer quand un fichier est enregistré.
    pub auto_reload: bool,
    /// Accès développeur : montre les outils de validation dans Options > Développement.
    pub developer_access: bool,
}

impl Default for DevSettings {
    fn default() -> Self {
        Self { auto_reload: true, developer_access: false }
    }
}

/// Ouvrir ou fermer la fenêtre du code (comme F5).
#[derive(Message)]
pub struct ToggleCodeWindow;

/// Rouvrir la fenêtre du code au démarrage (après une relance).
#[derive(Resource)]
struct OpenCodeAtStart;

fn watched(root: &Path) -> Vec<PathBuf> {
    vec![root.join("app").join("src"), root.join("crates").join("engine").join("src"), root.join("app").join("Cargo.toml"), root.join("crates").join("engine").join("Cargo.toml")]
}

/// Empreinte (chemin, taille, date) de tous les fichiers .rs / .toml surveillés.
fn sources_stamp(root: &Path) -> u64 {
    fn walk(path: &Path, h: &mut u64) {
        let Ok(meta) = std::fs::metadata(path) else { return };
        if meta.is_dir() {
            let Ok(entries) = std::fs::read_dir(path) else { return };
            let mut children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
            children.sort();
            for child in children {
                walk(&child, h);
            }
            return;
        }
        if !matches!(path.extension().and_then(|e| e.to_str()), Some("rs" | "toml")) {
            return;
        }
        let modified = meta.modified().ok().and_then(|m| m.duration_since(SystemTime::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as u64);
        for byte in path.to_string_lossy().bytes().chain(meta.len().to_le_bytes()).chain(modified.to_le_bytes()) {
            *h ^= byte as u64;
            *h = h.wrapping_mul(0x100000001b3);
        }
    }
    let mut h = 0xcbf29ce484222325;
    for path in watched(root) {
        walk(&path, &mut h);
    }
    h
}

/// `cargo build` du studio, avec la même toolchain et le même profil que l'exécutable actuel.
fn cargo_command(root: &Path) -> Command {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from);
    let cargo_bin = home.as_ref().map(|h| h.join(".cargo").join("bin"));
    let cargo = cargo_bin.as_ref().map(|b| b.join(format!("cargo{}", std::env::consts::EXE_SUFFIX))).filter(|c| c.is_file()).unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command.current_dir(root);
    if cfg!(all(windows, target_env = "gnu")) {
        // Pas de Visual Studio : toolchain gnu + WinLibs (comme build.cmd)
        command.arg("+stable-x86_64-pc-windows-gnu");
        let mut path: Vec<PathBuf> = Vec::new();
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            let winlibs = PathBuf::from(local).join(r"Microsoft\WinGet\Packages\BrechtSanders.WinLibs.POSIX.UCRT_Microsoft.Winget.Source_8wekyb3d8bbwe\mingw64\bin");
            if winlibs.is_dir() {
                path.push(winlibs);
            }
        }
        path.extend(cargo_bin);
        path.extend(std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).unwrap_or_default());
        if let Ok(joined) = std::env::join_paths(path) {
            command.env("PATH", joined);
        }
    }
    let package = std::env::current_exe().ok().and_then(|e| e.file_stem().map(|s| s.to_string_lossy().into_owned())).unwrap_or_else(|| "nds-game-studio".into());
    command.args(["build", "-p", &package, "--message-format", "short"]);
    if !cfg!(debug_assertions) {
        command.arg("--release");
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // pas de fenêtre de console
    }
    command
}

/// Recompile dans un fil. L'exécutable qui tourne est d'abord renommé : Windows ne laisse pas
/// écrire par-dessus un programme ouvert, mais il laisse le renommer.
fn start_build(root: PathBuf, state: Arc<Mutex<BuildState>>) {
    *state.lock().unwrap() = BuildState::Building;
    std::thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let old = exe.with_extension(format!("old-{}", std::process::id()));
            let renamed = std::fs::rename(&exe, &old).is_ok();
            let output = cargo_command(&root).output().map_err(|e| t!("cargo not found: {e}", "cargo introuvable : {e}"));
            let restore = || {
                if renamed && !exe.exists() {
                    let _ = std::fs::rename(&old, &exe);
                }
            };
            let output = match output {
                Ok(o) => o,
                Err(e) => {
                    restore();
                    return Err(e);
                }
            };
            if !output.status.success() {
                restore();
                let text = String::from_utf8_lossy(&output.stderr);
                let errors: Vec<&str> = text
                    .lines()
                    .filter(|l| !l.trim_start().starts_with("Compiling") && (l.contains("error") || l.contains("warning: unused") || l.starts_with("  ")))
                    .take(40)
                    .collect();
                return Err(if errors.is_empty() { text.lines().rev().take(20).collect::<Vec<_>>().join("\n") } else { errors.join("\n") });
            }
            if !exe.exists() {
                // Rien n'a été relinké : on garde l'actuel
                restore();
                return Err(tr("Build finished but no new executable (nothing to rebuild?)", "Compilation terminee mais aucun nouvel executable (rien a recompiler ?)").into());
            }
            Ok(())
        })();
        *state.lock().unwrap() = match result {
            Ok(()) => BuildState::Done,
            Err(e) => BuildState::Failed(e),
        };
    });
}

/// Restes des recompilations précédentes (exécutables renommés).
fn clean_old_executables() {
    let Ok(exe) = std::env::current_exe() else { return };
    let (Some(dir), Some(stem)) = (exe.parent(), exe.file_stem().map(|s| s.to_string_lossy().into_owned())) else { return };
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(&format!("{stem}.old-")) {
            let _ = std::fs::remove_file(entry.path()); // celui d'un studio encore ouvert reste
        }
    }
}

pub struct DevToolsPlugin;

impl Plugin for DevToolsPlugin {
    fn build(&self, app: &mut App) {
        clean_old_executables();
        app.insert_resource(Dev { root: source_root(), stamp: None, pending: false, last_watch: f64::NEG_INFINITY, build: Arc::new(Mutex::new(BuildState::Idle)) })
            .init_resource::<CodeView>()
            .init_resource::<DevSettings>()
            .add_message::<ToggleCodeWindow>()
            .add_systems(Update, (watch_sources, finish_build, toggle_code_window, refresh_code_window, edit_code, scroll_code, tab_looks).chain());
    }
}

fn watch_sources(time: Res<Time<Real>>, settings: Res<DevSettings>, mut dev: ResMut<Dev>, mut toast: MessageWriter<ShowToast>) {
    let now = time.elapsed_secs_f64();
    if now - dev.last_watch < WATCH_EVERY {
        return;
    }
    dev.last_watch = now;
    let Some(root) = dev.root.clone() else { return };
    let stamp = sources_stamp(&root);
    match dev.stamp {
        None => dev.stamp = Some(stamp),
        Some(previous) if previous != stamp => {
            // Encore en train d'enregistrer : on attend que ça ne bouge plus
            dev.stamp = Some(stamp);
            dev.pending = true;
        }
        Some(_) if dev.pending && !settings.auto_reload => dev.pending = false,
        Some(_) if dev.pending && *dev.build.lock().unwrap() != BuildState::Building => {
            dev.pending = false;
            info!("code modifie : recompilation");
            toast.write(ShowToast(tr("Code changed: rebuilding...", "Code modifie : recompilation...").into()));
            start_build(root, dev.build.clone());
        }
        _ => {}
    }
}

/// Recompilé : on se relance sur le même jeu, fenêtres à la même place.
fn finish_build(
    dev: Res<Dev>,
    active: Res<ActiveGame>,
    screens: Option<Res<DsScreens>>,
    layout: Res<ScreenLayout>,
    windows: Query<&Window>,
    code: Res<CodeView>,
    mut toast: MessageWriter<ShowToast>,
    mut exit: MessageWriter<AppExit>,
    mut reported: Local<Option<BuildState>>,
    _main_thread: NonSendMarker,
) {
    let state = dev.build.lock().unwrap().clone();
    if reported.as_ref() == Some(&state) {
        return;
    }
    *reported = Some(state.clone());
    match state {
        BuildState::Failed(_) => {
            toast.write(ShowToast(tr("Build error: see the Code window (F5)", "Erreur de compilation : voir la fenetre Code (F5)").into()));
        }
        BuildState::Done => {
            let Some(screens) = screens else { return };
            let main = screens.main_window();
            let at = WINIT_WINDOWS.with_borrow(|w| w.get_window(main).and_then(|w| w.outer_position().ok())).map(|p| IVec2::new(p.x, p.y));
            let size = windows.get(main).ok().map(|w| Vec2::new(w.width(), w.height()));
            let options = LaunchOptions { game: active.0.clone(), at, size, split: screens.bottom_window.is_some(), aspect: Some(layout.aspect), code: code.window.is_some() };
            let Ok(exe) = std::env::current_exe() else { return };
            info!("recompile : relance {:?}", options.to_args());
            match Command::new(&exe).args(options.to_args()).spawn() {
                Ok(_) => {
                    exit.write(AppExit::Success);
                }
                Err(e) => {
                    toast.write(ShowToast(t!("Cannot restart: {e}", "Relance impossible : {e}")));
                }
            }
        }
        _ => {}
    }
}

// ─────────────────────────────────────────────────────────────────────────
//  Fenêtre du code (F5)
// ─────────────────────────────────────────────────────────────────────────

/// La fenêtre du code (la fermer ne quitte pas le studio).
#[derive(Component)]
pub struct CodeWindow;

/// L'éditeur de la fenêtre du code.
#[derive(Component)]
pub struct CodeEditor;

#[derive(Component)]
struct CodeStatus;

#[derive(Component)]
struct CodeErrors;

#[derive(Component)]
struct CodeTabs;

#[derive(Component)]
struct CodeTab(usize);

#[derive(Resource, Default)]
struct CodeView {
    window: Option<Entity>,
    /// Caméra et interface de la fenêtre (supprimées avec elle).
    parts: Vec<Entity>,
    files: Vec<PathBuf>,
    current: usize,
    /// Jeu dont les fichiers sont listés (`None` = le studio, dans le menu).
    shown_for: Option<Option<String>>,
    loaded: Option<(PathBuf, SystemTime)>,
    /// Contenu du fichier tel qu'il est sur le disque (pour savoir s'il y a des modifications).
    saved: String,
    dirty: bool,
    /// Images à attendre avant de remonter au début du fichier (après sa mise en page).
    go_top: u8,
    font: Option<Handle<Font>>,
}

const CODE_BG: Color = Color::srgb(0.08, 0.09, 0.11);
const TAB: Color = Color::srgb(0.14, 0.15, 0.18);
const TAB_ON: Color = Color::srgb(0.24, 0.32, 0.5);
const TEXT: Color = Color::srgb(0.86, 0.87, 0.9);
const DIM: Color = Color::srgb(0.55, 0.58, 0.65);
const ERROR: Color = Color::srgb(1.0, 0.45, 0.4);

/// Fichiers montrés : ceux du jeu en cours (`app/src/games/<id>/`), sinon ceux du studio.
fn files_for(root: &Path, game: Option<&str>) -> Vec<PathBuf> {
    let games = root.join("app").join("src").join("games");
    if let Some(id) = game {
        let dir = games.join(id);
        if let Ok(entries) = std::fs::read_dir(&dir) {
            let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("rs" | "toml"))).collect();
            // mod.rs d'abord, puis le reste par nom
            files.sort_by_key(|p| (p.file_name().is_none_or(|n| n != "mod.rs"), p.clone()));
            if !files.is_empty() {
                return files;
            }
        }
        let single = games.join(format!("{id}.rs"));
        if single.is_file() {
            return vec![single];
        }
    }
    vec![root.join("app").join("src").join("main.rs")]
}

fn monospace_font(fonts: &mut Assets<Font>) -> Option<Handle<Font>> {
    let windir = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let bytes = std::fs::read(windir.join("Fonts").join("consola.ttf")).ok()?;
    Some(fonts.add(Font::from_bytes(bytes)))
}

fn toggle_code_window(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    dev: Res<Dev>,
    start: Option<Res<OpenCodeAtStart>>,
    mut view: ResMut<CodeView>,
    mut fonts: ResMut<Assets<Font>>,
    windows: Query<(), With<Window>>,
    mut requests: MessageReader<ToggleCodeWindow>,
    mut toast: MessageWriter<ShowToast>,
) {
    // Fenêtre fermée par sa croix : sa caméra et son interface aussi
    if view.window.is_some_and(|w| windows.get(w).is_err()) {
        view.window = None;
        for part in std::mem::take(&mut view.parts) {
            commands.entity(part).despawn();
        }
    }
    let open_now = start.is_some();
    if open_now {
        commands.remove_resource::<OpenCodeAtStart>();
    }
    let requested = requests.read().count() % 2 == 1;
    if !keys.just_pressed(KeyCode::F5) && !open_now && !requested {
        return;
    }
    if let Some(window) = view.window.take() {
        commands.entity(window).despawn();
        for part in std::mem::take(&mut view.parts) {
            commands.entity(part).despawn();
        }
        return;
    }
    if dev.root.is_none() {
        toast.write(ShowToast(tr("Source code not found: F5 works with the repository (local build)", "Code source introuvable : F5 marche avec le depot (compilation locale)").into()));
        return;
    }
    if view.font.is_none() {
        view.font = monospace_font(&mut fonts);
    }
    let window = commands.spawn((Window { title: "Nds Game Studio - Code (F5)".into(), resolution: WindowResolution::new(900, 860), ..default() }, CodeWindow)).id();
    let camera = commands
        .spawn((Camera2d, Camera { order: 5, clear_color: ClearColorConfig::Custom(CODE_BG), ..default() }, RenderTarget::Window(WindowRef::Entity(window)), bevy::camera::visibility::RenderLayers::none()))
        .id();
    let font = |size: f32| match &view.font {
        Some(f) => TextFont { font: f.clone().into(), font_size: FontSize::Px(size), ..default() },
        None => TextFont::from_font_size(size),
    };
    let root = commands
        .spawn((
            Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Column, padding: UiRect::all(px(8)), row_gap: px(6), ..default() },
            UiTargetCamera(camera),
        ))
        .id();
    commands.entity(root).with_children(|ui| {
        ui.spawn((Node { flex_direction: FlexDirection::Row, column_gap: px(4), flex_wrap: FlexWrap::Wrap, ..default() }, CodeTabs));
        ui.spawn((Text::new(""), font(14.0), TextColor(DIM), CodeStatus));
        ui.spawn((Text::new(""), font(13.0), TextColor(ERROR), CodeErrors, Node { display: Display::None, ..default() }));
        // min_height 0 : sinon la zone grandit à la taille du fichier et rien ne défile
        ui.spawn((
            Node { flex_grow: 1.0, min_height: px(0), width: percent(100), padding: UiRect::all(px(6)), ..default() },
            TextInput,
            // Sans TabIndex, un clic ne donne pas le clavier à l'éditeur
            bevy::input_focus::tab_navigation::TabIndex(0),
            editable(""),
            TextLayout { linebreak: LineBreak::NoWrap, ..default() },
            TextCursorStyle { color: Color::WHITE, selected_text_color: Some(Color::BLACK), ..default() },
            font(15.0),
            TextColor(TEXT),
            BackgroundColor(Color::srgb(0.055, 0.06, 0.075)),
            CodeEditor,
        ));
    });
    view.window = Some(window);
    // Pas enfants de la fenêtre : une interface Bevy doit avoir une racine sans parent
    view.parts = vec![camera, root];
    view.shown_for = None;
    view.loaded = None;
}

fn editable(text: &str) -> EditableText {
    EditableText { allow_newlines: true, ..EditableText::new(text) }
}

fn editor_text(editor: &EditableText) -> String {
    editor.value().into_iter().collect()
}

/// L'éditeur a le clavier : les touches du jeu (et Échap) ne doivent pas réagir.
pub fn typing_in_editor(focus: Option<&InputFocus>, editors: &Query<(), With<CodeEditor>>) -> bool {
    focus.and_then(|f| f.get()).is_some_and(|e| editors.get(e).is_ok())
}

fn save_file(path: &Path, text: &str, view: &mut CodeView, toast: &mut MessageWriter<ShowToast>) {
    match std::fs::write(path, text) {
        Ok(()) => {
            view.saved = text.to_string();
            view.dirty = false;
            view.loaded = std::fs::metadata(path).and_then(|m| m.modified()).ok().map(|m| (path.to_path_buf(), m));
            info!("enregistre : {}", path.display());
        }
        Err(e) => {
            toast.write(ShowToast(t!("Cannot save: {e}", "Enregistrement impossible : {e}")));
        }
    }
}

fn refresh_code_window(
    mut commands: Commands,
    dev: Res<Dev>,
    active: Res<ActiveGame>,
    mut view: ResMut<CodeView>,
    tabs: Query<(Entity, Option<&Children>), With<CodeTabs>>,
    editors: Query<(Entity, &EditableText), With<CodeEditor>>,
    mut status: Query<&mut Text, (With<CodeStatus>, Without<CodeErrors>)>,
    mut errors: Query<(&mut Text, &mut Node), (With<CodeErrors>, Without<CodeStatus>)>,
    mut toast: MessageWriter<ShowToast>,
) {
    let (Some(_), Some(root)) = (view.window, dev.root.clone()) else { return };

    // Jeu changé (ou fenêtre ouverte) : nouveaux onglets
    if view.shown_for.as_ref() != Some(&active.0) {
        view.files = files_for(&root, active.0.as_deref());
        view.current = 0;
        view.shown_for = Some(active.0.clone());
        view.loaded = None;
        if let Ok((tabs_entity, children)) = tabs.single() {
            for child in children.into_iter().flatten() {
                commands.entity(*child).despawn();
            }
            for (i, file) in view.files.iter().enumerate() {
                let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let tab = commands
                    .spawn((
                        Button,
                        Hovered::default(),
                        Node { padding: UiRect::axes(px(10), px(4)), ..default() },
                        BackgroundColor(TAB),
                        CodeTab(i),
                        children![(Text::new(name), TextFont::from_font_size(13.0), TextColor(TEXT))],
                    ))
                    .observe(select_tab)
                    .id();
                commands.entity(tabs_entity).add_child(tab);
            }
        }
    }

    // Fichier chargé dans l'éditeur, puis relu quand il change sur le disque
    if let (Some(path), Ok((editor, editable))) = (view.files.get(view.current).cloned(), editors.single()) {
        let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        let other_file = view.loaded.as_ref().is_none_or(|(p, _)| *p != path);
        let changed_on_disk = view.loaded.as_ref().zip(modified).is_some_and(|((p, t), m)| *p == path && *t != m);
        // Changement d'onglet avec des modifications : on les enregistre d'abord
        if other_file
            && view.dirty
            && let Some((previous, _)) = view.loaded.clone()
        {
            let text = editor_text(editable);
            save_file(&previous, &text, &mut view, &mut toast);
        }
        if other_file || (changed_on_disk && !view.dirty) {
            let content = std::fs::read_to_string(&path).unwrap_or_else(|e| t!("Cannot read: {e}", "Lecture impossible : {e}"));
            commands.entity(editor).insert(self::editable(&content));
            view.go_top = 3;
            view.saved = content;
            view.dirty = false;
            view.loaded = modified.map(|m| (path.clone(), m));
        }
    }

    // État de la compilation
    let build = dev.build.lock().unwrap().clone();
    let file = view.files.get(view.current).and_then(|p| p.strip_prefix(&root).ok()).map(|p| p.display().to_string()).unwrap_or_default();
    let line = match &build {
        BuildState::Building => t!("{file}   |   Rebuilding...", "{file}   |   Recompilation en cours..."),
        BuildState::Failed(_) => t!("{file}   |   Build error (the game keeps running the old code)", "{file}   |   Erreur de compilation (le jeu continue avec l'ancien code)"),
        _ if view.dirty => t!("{file}   |   Modified: Ctrl+S to save (the studio rebuilds and restarts)", "{file}   |   Modifie : Ctrl+S pour enregistrer (le studio recompile et se relance)"),
        _ => t!("{file}   |   Ctrl+S saves: the studio rebuilds and restarts by itself", "{file}   |   Ctrl+S enregistre : le studio recompile et se relance tout seul"),
    };
    if let Ok(mut s) = status.single_mut()
        && s.0 != line
    {
        s.0 = line;
    }
    if let Ok((mut text, mut node)) = errors.single_mut() {
        let message = if let BuildState::Failed(e) = &build { e.clone() } else { String::new() };
        if text.0 != message {
            node.display = if message.is_empty() { Display::None } else { Display::Flex };
            text.0 = message;
        }
    }
}

/// Modifications en cours, Ctrl+S, et le clavier rendu au jeu quand on quitte la fenêtre du code.
fn edit_code(
    keys: Res<ButtonInput<KeyCode>>,
    mut view: ResMut<CodeView>,
    windows: Query<&Window>,
    changed: Query<&EditableText, (With<CodeEditor>, Changed<EditableText>)>,
    editors: Query<&EditableText, With<CodeEditor>>,
    editor_marks: Query<(), With<CodeEditor>>,
    mut focus: ResMut<InputFocus>,
    mut toast: MessageWriter<ShowToast>,
) {
    let Some(window) = view.window.and_then(|w| windows.get(w).ok()) else { return };
    if let Ok(editable) = changed.single() {
        let dirty = editor_text(editable) != view.saved;
        if view.dirty != dirty {
            view.dirty = dirty;
        }
    }
    // La fenêtre du code n'a plus le focus : le clavier revient au jeu
    if !window.focused && typing_in_editor(Some(&focus), &editor_marks) {
        focus.clear();
    }
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    if window.focused
        && ctrl
        && keys.just_pressed(KeyCode::KeyS)
        && let (Some((path, _)), Ok(editable)) = (view.loaded.clone(), editors.single())
    {
        let text = editor_text(editable);
        save_file(&path, &text, &mut view, &mut toast);
    }
}

fn select_tab(activate: On<Activate>, tabs: Query<&CodeTab>, mut view: ResMut<CodeView>) {
    if let Ok(tab) = tabs.get(activate.entity)
        && view.current != tab.0
    {
        view.current = tab.0;
        view.loaded = None;
    }
}

fn tab_looks(view: Res<CodeView>, mut tabs: Query<(&CodeTab, &Hovered, Has<Pressed>, &mut BackgroundColor)>) {
    for (tab, hovered, pressed, mut color) in &mut tabs {
        let wanted = if tab.0 == view.current || pressed { TAB_ON } else if hovered.get() { Color::srgb(0.2, 0.22, 0.27) } else { TAB };
        if color.0 != wanted {
            color.0 = wanted;
        }
    }
}

/// Molette sur la fenêtre du code : défilement de l'éditeur.
fn scroll_code(mut wheel: MessageReader<MouseWheel>, mut view: ResMut<CodeView>, windows: Query<&Window>, mut editors: Query<&mut EditableText, With<CodeEditor>>) {
    // Fichier chargé : Bevy met le curseur à la fin une fois le texte mis en page, on remonte ensuite
    if view.go_top > 0 {
        view.go_top -= 1;
        if view.go_top == 0
            && let Ok(mut editor) = editors.single_mut()
        {
            editor.queue_edit(TextEdit::TextStart(false));
            editor.queue_edit(TextEdit::ScrollTo(Vec2::ZERO));
        }
    }
    let over = view.window.and_then(|w| windows.get(w).ok()).is_some_and(|w| w.cursor_position().is_some());
    let (mut lines, mut pixels) = (0.0, 0.0);
    for event in wheel.read() {
        match event.unit {
            MouseScrollUnit::Line => lines -= event.y * 3.0,
            MouseScrollUnit::Pixel => pixels -= event.y,
        }
    }
    if !over || (lines == 0.0 && pixels == 0.0) {
        return;
    }
    if let Ok(mut editor) = editors.single_mut() {
        if lines != 0.0 {
            editor.queue_edit(TextEdit::ScrollByLines(lines));
        }
        if pixels != 0.0 {
            editor.queue_edit(TextEdit::ScrollBy(Vec2::new(0.0, pixels)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_options_round_trip() {
        let options = LaunchOptions { game: Some("orbit".into()), at: Some(IVec2::new(-10, 40)), size: Some(Vec2::new(512.0, 800.0)), split: true, aspect: Some(Aspect::Wide16x9), code: true };
        assert_eq!(LaunchOptions::parse(options.to_args()), options);
        assert_eq!(LaunchOptions::parse(["bubbles".to_string()]).game.as_deref(), Some("bubbles"));
        assert_eq!(LaunchOptions::parse(Vec::<String>::new()), LaunchOptions::default());
    }

    #[test]
    fn game_files_fall_back_to_the_studio() {
        let root = source_root().expect("depot present pendant les tests");
        assert!(files_for(&root, None)[0].ends_with("main.rs"));
        assert!(files_for(&root, Some("jeu-absent"))[0].ends_with("main.rs"));
        assert_ne!(sources_stamp(&root), 0);
    }
}
