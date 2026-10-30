//! Fenêtre Assets (F6) : les fichiers d'un jeu décompilé, modèles 3D et images aux formats
//! Nintendo standard, pour reconnaître et nommer les objets du jeu.
//!
//! - les jeux sont les dossiers `<projets>/<jeu>/extract/<version>/files` (NitroFS extrait par
//!   dsd) ; `<projets>` = `--projects=<dossier>`, la variable `NDS_DECOMP_PROJECTS`, ou
//!   `D:\... logiciel\decomp\decomp ds\projets` ;
//! - la liste montre chaque fichier avec son chemin dans NitroFS (celui que le code du jeu
//!   charge ; `archive.narc#n` pour le n-ième fichier d'une archive), reconnu par sa signature ;
//!   taper du texte filtre la liste, la molette la fait défiler ;
//! - clic = aperçu : `.nsbmd` en 3D (glisser = tourner, clic droit / milieu = déplacer, molette =
//!   zoom), textures `.nsbtx`, palettes `.nclr`, tuiles `.ncgr`/`.ncbr`, écrans `.nscr`, cellules
//!   `.ncer` ; la palette, les tuiles ou les textures qui vont avec sont devinées (même dossier,
//!   nom le plus proche) et se changent avec les flèches ;
//! - « Copier le chemin » met le chemin NitroFS dans le presse-papiers (pour le relier aux
//!   fonctions du jeu dans DS Decomp Studio) ;
//! - section « rom.bin » : les fichiers de l'archive numérotée rom.bin (Lego Star Wars II, Les
//!   Sims 2...) sortis par `studio/gametools/romarc.py extract` dans `<jeu>/build/romarc`, montrés
//!   `rom.bin#n` ; « Copier le chemin » y copie le n° n seul (clé de DS Decomp Studio) et les
//!   textures d'un modèle sont cherchées dans les `.nsbtx` aux numéros les plus proches.
//!
//! Lecture des formats : crate `nds-nitro` (code d'apicula pour la 3D).

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::RenderTarget;
use bevy::clipboard::Clipboard;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::mouse::MouseScrollUnit;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::picking::events::{PointerDrag, PointerScroll};
use bevy::picking::hover::Hovered;
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::render::view::Tonemapping;
use bevy::ui::Pressed;
use bevy::ui_widgets::{Activate, Button};
use bevy::window::{WindowRef, WindowResolution};
use nds_nitro::{g2d, model3d, narc, sniff, unpack, Kind, Rgba};

/// La fenêtre des assets (la fermer ne quitte pas le studio).
#[derive(Component)]
pub struct AssetsWindow;

/// Ouvrir ou fermer la fenêtre des assets (comme F6).
#[derive(Message)]
pub struct ToggleAssets;

const DEFAULT_PROJECTS: &str = r"D:\... logiciel\decomp\decomp ds\projets";
/// Calque des modèles 3D montrés (ni les jeux ni les écrans ne l'utilisent).
const LAYER: usize = 24;
const ROWS: usize = 36;
const ROW_H: f32 = 21.0;
/// Taille de l'image où la vue 3D est rendue.
const VIEW_W: u32 = 1280;
const VIEW_H: u32 = 960;

const BG: Color = Color::srgb(0.1, 0.11, 0.13);
const SIDEBAR: Color = Color::srgb(0.075, 0.08, 0.095);
const ITEM: Color = Color::srgb(0.16, 0.17, 0.2);
const ITEM_HOVER: Color = Color::srgb(0.22, 0.24, 0.28);
const ITEM_ON: Color = Color::srgb(0.24, 0.36, 0.62);
const ROW: Color = Color::NONE;
const TEXT: Color = Color::srgb(0.9, 0.91, 0.94);
const DIM: Color = Color::srgb(0.6, 0.63, 0.7);
const VIEW_BG: Color = Color::srgb(0.2, 0.21, 0.24);

struct GameDir {
    label: String,
    files: PathBuf,
}

struct Entry {
    /// Chemin dans NitroFS (sans « / » au début) ; `archive.narc#n` dans une archive.
    path: String,
    /// Nom dans l'archive, s'il y en a un.
    member_name: Option<String>,
    disk: PathBuf,
    member: Option<(usize, usize)>,
    kind: Kind,
    size: usize,
    /// Dossier (ou archive) : les fichiers qui vont ensemble y sont cherchés d'abord.
    dir: String,
    stem: String,
    /// N° dans l'archive rom.bin (fichiers sortis par `romarc.py extract` dans build/romarc).
    rom_index: Option<u32>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ctl {
    Palette,
    Tiles,
    Textures,
    Bank,
    Width,
    Cell,
    Model,
}

/// Choix de l'aperçu (fichiers qui vont avec, palette, cellule...).
#[derive(Default)]
struct Choices {
    palettes: Vec<usize>,
    palette: usize,
    tiles: Vec<usize>,
    tile: usize,
    /// Fichiers de textures pour un modèle ; `texture` 0 = aucun, n = `textures[n - 1]`.
    textures: Vec<usize>,
    texture: usize,
    bank: usize,
    banks: usize,
    width: u32,
    cell: usize,
    cells: usize,
    model: usize,
    models: usize,
}

#[derive(Resource)]
pub(crate) struct AssetBrowser {
    window: Option<Entity>,
    parts: Vec<Entity>,
    list: Option<Entity>,
    games_overlay: Option<Entity>,
    controls: Option<Entity>,
    image_box: Option<Entity>,
    image_node: Option<Entity>,
    camera3d: Option<Entity>,
    light: Option<Entity>,
    view3d: Handle<Image>,
    root: PathBuf,
    games: Vec<GameDir>,
    game: Option<usize>,
    games_open: bool,
    entries: Vec<Entry>,
    shown: Vec<usize>,
    filter: String,
    all_files: bool,
    scroll: usize,
    selected: Option<usize>,
    choices: Choices,
    info: String,
    status: String,
    /// Image montrée et sa taille (la vue 3D si `is3d`).
    image: Option<(Handle<Image>, UVec2)>,
    is3d: bool,
    orbit: Orbit,
    list_dirty: bool,
    preview_dirty: bool,
    controls_dirty: bool,
    /// Jeu et fichier à ouvrir au démarrage (`--assets=<jeu>`, `--asset=<chemin>`).
    start: Option<(String, Option<String>)>,
    /// Lecture des fichiers d'un jeu en cours (dans un fil à part : des milliers de fichiers).
    scanning: Option<Arc<Mutex<Option<Vec<Entry>>>>>,
    /// Fichier à montrer quand la lecture est finie.
    want_file: Option<String>,
}

impl Default for AssetBrowser {
    fn default() -> Self {
        AssetBrowser {
            window: None,
            parts: Vec::new(),
            list: None,
            games_overlay: None,
            controls: None,
            image_box: None,
            image_node: None,
            camera3d: None,
            light: None,
            view3d: Handle::default(),
            root: PathBuf::new(),
            games: Vec::new(),
            game: None,
            games_open: false,
            entries: Vec::new(),
            shown: Vec::new(),
            filter: String::new(),
            all_files: false,
            scroll: 0,
            selected: None,
            choices: Choices::default(),
            info: String::new(),
            status: String::new(),
            image: None,
            is3d: false,
            orbit: Orbit::default(),
            list_dirty: false,
            preview_dirty: false,
            controls_dirty: false,
            start: None,
            scanning: None,
            want_file: None,
        }
    }
}

struct Orbit {
    yaw: f32,
    pitch: f32,
    distance: f32,
    target: Vec3,
    radius: f32,
}

impl Default for Orbit {
    fn default() -> Self {
        Orbit { yaw: 0.6, pitch: 0.35, distance: 3.0, target: Vec3::ZERO, radius: 1.0 }
    }
}

#[derive(Component)]
struct ListRow(usize);

#[derive(Component)]
struct GameButton;

#[derive(Component)]
struct GameChoice(usize);

#[derive(Component)]
struct FilterButton;

#[derive(Component)]
struct CopyButton;

#[derive(Component)]
struct PageButton(i32);

#[derive(Component)]
struct StepButton(Ctl, i32);

#[derive(Component)]
struct ListTitle;

#[derive(Component)]
struct InfoText;

#[derive(Component)]
struct PathText;

#[derive(Component)]
struct Preview3d;

pub struct AssetBrowserPlugin;

impl Plugin for AssetBrowserPlugin {
    fn build(&self, app: &mut App) {
        let mut browser = AssetBrowser::default();
        let mut projects = std::env::var_os("NDS_DECOMP_PROJECTS").map(PathBuf::from);
        let (mut game, mut file, mut open) = (None, None, false);
        for arg in std::env::args().skip(1) {
            if let Some(v) = arg.strip_prefix("--projects=") {
                projects = Some(PathBuf::from(v));
            } else if arg == "--assets" {
                open = true;
            } else if let Some(v) = arg.strip_prefix("--assets=") {
                (open, game) = (true, Some(v.to_string()));
            } else if let Some(v) = arg.strip_prefix("--asset=") {
                file = Some(v.replace('\\', "/"));
            }
        }
        browser.root = projects.unwrap_or_else(|| PathBuf::from(DEFAULT_PROJECTS));
        if open {
            browser.start = Some((game.unwrap_or_default(), file));
        }
        app.insert_resource(browser).add_message::<ToggleAssets>().add_systems(
            Update,
            (toggle_window, poll_scan, type_filter, rebuild_games, rebuild_list, render_preview, rebuild_controls, refresh_texts, fit_image, place_camera, looks).chain(),
        );
    }
}

fn label(text: impl Into<String>, size: f32, color: Color) -> impl Bundle {
    (Text::new(text), TextFont::from_font_size(size), TextColor(color))
}

fn item_node() -> Node {
    Node { padding: UiRect::axes(px(12), px(6)), border_radius: BorderRadius::all(px(6)), justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() }
}

// --------------------------------------------------------------------------------------------
//  Jeux et fichiers
// --------------------------------------------------------------------------------------------

/// Les jeux qui ont un NitroFS extrait.
fn find_games(root: &Path) -> Vec<GameDir> {
    let mut games = Vec::new();
    let Ok(dirs) = std::fs::read_dir(root) else { return games };
    let mut dirs: Vec<PathBuf> = dirs.flatten().map(|d| d.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    for dir in dirs {
        let Ok(versions) = std::fs::read_dir(dir.join("extract")) else { continue };
        let mut versions: Vec<PathBuf> = versions.flatten().map(|d| d.path()).filter(|p| p.join("files").is_dir()).collect();
        versions.sort();
        let name = dir.file_name().unwrap_or_default().to_string_lossy().to_string();
        let several = versions.len() > 1;
        for v in versions {
            let version = v.file_name().unwrap_or_default().to_string_lossy().to_string();
            let label = if several { format!("{name} ({version})") } else { name.clone() };
            games.push(GameDir { label, files: v.join("files") });
        }
    }
    games
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut items: Vec<PathBuf> = read.flatten().map(|d| d.path()).collect();
    items.sort_by_key(|p| p.to_string_lossy().to_lowercase());
    for p in items {
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
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

/// Tous les fichiers du jeu, et ceux des archives NARC.
fn scan(files: &Path) -> Vec<Entry> {
    let mut paths = Vec::new();
    walk(files, &mut paths);
    let mut entries = Vec::new();
    for disk in paths {
        let path = disk.strip_prefix(files).unwrap_or(&disk).to_string_lossy().replace('\\', "/");
        let size = disk.metadata().map(|m| m.len() as usize).unwrap_or(0);
        let mut head = [0u8; 4];
        let n = std::fs::File::open(&disk).and_then(|mut f| f.read(&mut head)).unwrap_or(0);
        let small = size < 4 << 20;
        let kind = kind_of(&head[..n], || small.then(|| std::fs::read(&disk).ok()).flatten());
        let dir = path.rsplit_once('/').map_or(String::new(), |(d, _)| d.to_string());
        let stem = stem_of(&path);
        entries.push(Entry { path: path.clone(), member_name: None, disk: disk.clone(), member: None, kind, size, dir, stem, rom_index: None });
        if kind != Kind::Archive {
            continue;
        }
        let Ok(bytes) = std::fs::read(&disk).map(unpack) else { continue };
        let Ok(members) = narc::read(&bytes) else { continue };
        for m in members {
            let data = &bytes[m.start..m.end];
            let kind = kind_of(data, || Some(data.to_vec()));
            let member_path = format!("{path}#{}", m.index);
            let named = !m.name.chars().all(|c| c.is_ascii_digit());
            let stem = if named { stem_of(&m.name) } else { String::new() };
            entries.push(Entry { path: member_path, member_name: named.then(|| m.name.clone()), disk: disk.clone(), member: Some((m.start, m.end)), kind, size: m.end - m.start, dir: path.clone(), stem, rom_index: None });
        }
    }
    entries.extend(scan_romarc(files));
    entries
}

/// Dossier des fichiers sortis de rom.bin : `<jeu>/build/romarc` (files = `<jeu>/extract/<v>/files`).
fn romarc_dir(files: &Path) -> Option<PathBuf> {
    Some(files.parent()?.parent()?.parent()?.join("build").join("romarc"))
}

/// Les fichiers de l'archive numérotée rom.bin (jeux Traveller's Tales, EA...) une fois sortis
/// par `studio/gametools/romarc.py extract` : `<n sur 5 chiffres>.<ext>`.
fn scan_romarc(files: &Path) -> Vec<Entry> {
    let Some(dir) = romarc_dir(files) else { return Vec::new() };
    let mut paths = Vec::new();
    walk(&dir, &mut paths);
    let mut entries: Vec<Entry> = paths
        .into_iter()
        .filter_map(|disk| {
            let name = disk.file_name()?.to_string_lossy().to_string();
            let n: u32 = name.split('.').next()?.parse().ok()?;
            let size = disk.metadata().map(|m| m.len() as usize).unwrap_or(0);
            let mut head = [0u8; 4];
            let read = std::fs::File::open(&disk).and_then(|mut f| f.read(&mut head)).unwrap_or(0);
            let kind = kind_of(&head[..read], || (size < 4 << 20).then(|| std::fs::read(&disk).ok()).flatten());
            Some(Entry { path: format!("rom.bin#{n}"), member_name: Some(name), disk, member: None, kind, size, dir: "rom.bin".into(), stem: n.to_string(), rom_index: Some(n) })
        })
        .collect();
    entries.sort_by_key(|e| e.rom_index);
    entries
}

fn read_entry(entry: &Entry) -> Result<Vec<u8>, String> {
    let bytes = unpack(std::fs::read(&entry.disk).map_err(|e| e.to_string())?);
    Ok(match entry.member {
        Some((start, end)) => unpack(bytes.get(start..end).ok_or("archive changee depuis la lecture")?.to_vec()),
        None => bytes,
    })
}

impl AssetBrowser {
    fn open_game(&mut self, index: usize) {
        self.game = Some(index);
        self.games_open = false;
        self.entries.clear();
        let result = Arc::new(Mutex::new(None));
        let (files, out) = (self.games[index].files.clone(), result.clone());
        std::thread::spawn(move || {
            let entries = scan(&files);
            *out.lock().unwrap() = Some(entries);
        });
        self.scanning = Some(result);
        self.selected = None;
        self.scroll = 0;
        self.image = None;
        self.is3d = false;
        self.info.clear();
        self.choices = Choices::default();
        self.list_dirty = true;
        self.controls_dirty = true;
        self.preview_dirty = true;
        self.status = format!("Lecture des fichiers de {}...", self.games[index].label);
    }

    fn scan_done(&mut self, entries: Vec<Entry>) {
        self.entries = entries;
        self.scanning = None;
        self.list_dirty = true;
        let viewable = self.entries.iter().filter(|e| e.kind.viewable()).count();
        self.status = format!("{} fichiers, {} montrables (formats Nintendo standard)", self.entries.len(), viewable);
        let files = self.game.map(|g| self.games[g].files.clone()).unwrap_or_default();
        let romarc = self.entries.iter().filter(|e| e.rom_index.is_some()).count();
        if romarc > 0 {
            self.status += &format!(" ; rom.bin : {romarc} fichiers (build/romarc, filtre 'rom.bin')");
        } else if files.join("rom.bin").is_file() {
            self.status += " ; rom.bin pas encore extrait : dans le dossier du jeu, python ../../studio/gametools/romarc.py extract";
        }
        if let Some(i) = self.want_file.take().and_then(|f| self.entries.iter().position(|e| e.path.eq_ignore_ascii_case(&f))) {
            self.filter_entries();
            self.scroll = self.shown.iter().position(|&s| s == i).unwrap_or(0).saturating_sub(ROWS / 2);
            self.select(i);
        }
    }

    fn filter_entries(&mut self) {
        let words: Vec<String> = self.filter.to_lowercase().split_whitespace().map(String::from).collect();
        self.shown = (0..self.entries.len())
            .filter(|&i| {
                let e = &self.entries[i];
                if !self.all_files && !e.kind.viewable() {
                    return false;
                }
                let hay = format!("{} {} {}", e.path.to_lowercase(), e.member_name.as_deref().unwrap_or("").to_lowercase(), e.kind.label());
                words.iter().all(|w| hay.contains(w.as_str()))
            })
            .collect();
        self.scroll = self.scroll.min(self.shown.len().saturating_sub(ROWS));
    }

    /// Les fichiers d'une sorte qui vont le mieux avec `sel` : même dossier d'abord, puis le nom
    /// le plus proche ; tout le jeu s'il n'y en a aucun dans le dossier.
    fn companions(&self, sel: usize, kinds: &[Kind]) -> Vec<usize> {
        let me = &self.entries[sel];
        let closeness = |e: &Entry| {
            // rom.bin : les fichiers qui vont ensemble ont des numéros proches
            if let (Some(a), Some(b)) = (me.rom_index, e.rom_index) {
                return (false, std::cmp::Reverse(usize::MAX - a.abs_diff(b) as usize));
            }
            let common = me.stem.chars().zip(e.stem.chars()).take_while(|(a, b)| a == b).count();
            (e.stem != me.stem, std::cmp::Reverse(common))
        };
        let mut near: Vec<usize> = (0..self.entries.len()).filter(|&i| i != sel && kinds.contains(&self.entries[i].kind) && self.entries[i].dir == me.dir).collect();
        if near.is_empty() {
            near = (0..self.entries.len()).filter(|&i| i != sel && kinds.contains(&self.entries[i].kind)).collect();
        }
        near.sort_by_key(|&i| (closeness(&self.entries[i]), self.entries[i].path.clone()));
        near.truncate(400);
        near
    }

    fn select(&mut self, index: usize) {
        self.selected = Some(index);
        let kind = self.entries[index].kind;
        let mut c = Choices::default();
        match kind {
            Kind::Tiles | Kind::Screen | Kind::Cells => {
                c.palettes = self.companions(index, &[Kind::Palette]);
                if kind != Kind::Tiles {
                    c.tiles = self.companions(index, &[Kind::Tiles]);
                }
            }
            Kind::Model => {
                c.textures = self.companions(index, &[Kind::Textures]);
                c.texture = self.best_textures(index, &c.textures);
            }
            _ => {}
        }
        self.choices = c;
        self.orbit = Orbit::default();
        self.preview_dirty = true;
        self.controls_dirty = true;
    }

    /// Les textures d'un modèle sont souvent dans un `.nsbtx` à part : on prend le fichier qui
    /// donne le plus de textures trouvées (0 = aucun, le modèle a déjà les siennes).
    fn best_textures(&self, model: usize, candidates: &[usize]) -> usize {
        let Ok(bytes) = read_entry(&self.entries[model]) else { return 0 };
        let Ok(own) = model3d::read(&bytes) else { return 0 };
        let missing = |others: &[nds_nitro::nitro::Container]| model3d::build_models(&own, others).iter().map(|m| m.warnings.len()).sum::<usize>();
        let mut best = (missing(&[]), 0);
        if best.0 == 0 {
            return 0;
        }
        for (n, &i) in candidates.iter().take(40).enumerate() {
            let Ok(other) = read_entry(&self.entries[i]).and_then(|b| model3d::read(&b).map_err(|e| e.to_string())) else { continue };
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
}

// --------------------------------------------------------------------------------------------
//  Fenêtre
// --------------------------------------------------------------------------------------------

fn toggle_window(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    mut requests: MessageReader<ToggleAssets>,
    mut browser: ResMut<AssetBrowser>,
    mut images: ResMut<Assets<Image>>,
    windows: Query<(), With<Window>>,
) {
    if browser.window.is_some_and(|w| windows.get(w).is_err()) {
        browser.window = None;
        for part in std::mem::take(&mut browser.parts) {
            commands.entity(part).despawn();
        }
    }
    let start = browser.start.take();
    let requested = requests.read().count() % 2 == 1;
    if !keys.just_pressed(KeyCode::F6) && !requested && start.is_none() {
        return;
    }
    if let Some(window) = browser.window.take() {
        commands.entity(window).despawn();
        for part in std::mem::take(&mut browser.parts) {
            commands.entity(part).despawn();
        }
        return;
    }
    let window = commands.spawn((Window { title: "Nds Game Studio - Assets (F6)".into(), resolution: WindowResolution::new(1500, 920), ..default() }, AssetsWindow)).id();
    let camera = commands
        .spawn((Camera2d, Camera { order: 7, clear_color: ClearColorConfig::Custom(BG), ..default() }, RenderTarget::Window(WindowRef::Entity(window)), RenderLayers::none()))
        .id();
    // Vue 3D : rendue dans une image montrée dans le panneau d'aperçu
    let view3d = images.add(Image::new_target_texture(VIEW_W, VIEW_H, TextureFormat::Bgra8UnormSrgb, None));
    let camera3d = commands
        .spawn((
            Camera3d::default(),
            Camera { order: -3, clear_color: ClearColorConfig::Custom(VIEW_BG), is_active: false, ..default() },
            RenderTarget::Image(view3d.clone().into()),
            Tonemapping::None,
            AmbientLight { color: Color::WHITE, brightness: 900.0, ..default() },
            Transform::from_xyz(0.0, 0.0, 3.0).looking_at(Vec3::ZERO, Vec3::Y),
            RenderLayers::layer(LAYER),
        ))
        .id();
    let light = commands.spawn((DirectionalLight { illuminance: 4000.0, ..default() }, Transform::default(), RenderLayers::layer(LAYER))).id();

    let mut ids = HashMap::new();
    let root = commands
        .spawn((Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Row, ..default() }, UiTargetCamera(camera)))
        .with_children(|root| {
            // Colonne de gauche : jeu, filtre, liste des fichiers
            root.spawn((Node { width: px(560), height: percent(100), flex_direction: FlexDirection::Column, padding: UiRect::all(px(10)), row_gap: px(6), ..default() }, BackgroundColor(SIDEBAR)))
                .with_children(|side| {
                    side.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(8), align_items: AlignItems::Center, ..default() }).with_children(|row| {
                        row.spawn(label("Jeu :", 15.0, DIM));
                        row.spawn((Button, Hovered::default(), Node { flex_grow: 1.0, justify_content: JustifyContent::FlexStart, ..item_node() }, BackgroundColor(ITEM), GameButton))
                            .observe(|_: On<Activate>, mut b: ResMut<AssetBrowser>| {
                                b.games_open = !b.games_open;
                            })
                            .with_child((label("", 15.0, TEXT), GameButton));
                    });
                    side.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(8), align_items: AlignItems::Center, ..default() }).with_children(|row| {
                        row.spawn((Button, Hovered::default(), item_node(), BackgroundColor(ITEM), FilterButton))
                            .observe(|_: On<Activate>, mut b: ResMut<AssetBrowser>| {
                                b.all_files = !b.all_files;
                                b.list_dirty = true;
                            })
                            .with_child((label("", 14.0, TEXT), FilterButton));
                        row.spawn((Button, Hovered::default(), item_node(), BackgroundColor(ITEM), PageButton(-1))).observe(page).with_child(label("^", 14.0, TEXT));
                        row.spawn((Button, Hovered::default(), item_node(), BackgroundColor(ITEM), PageButton(1))).observe(page).with_child(label("v", 14.0, TEXT));
                    });
                    side.spawn((label("", 13.0, DIM), ListTitle));
                    let list = side
                        .spawn((Node { flex_grow: 1.0, flex_direction: FlexDirection::Column, overflow: Overflow::clip(), ..default() }, Pickable::default()))
                        .observe(|scroll: On<PointerScroll>, mut b: ResMut<AssetBrowser>| {
                            let lines = match scroll.unit {
                                MouseScrollUnit::Line => scroll.y * 3.0,
                                MouseScrollUnit::Pixel => scroll.y / ROW_H,
                            };
                            let max = b.shown.len().saturating_sub(ROWS) as i64;
                            let wanted = (b.scroll as i64 - lines.round() as i64).clamp(0, max.max(0)) as usize;
                            if wanted != b.scroll {
                                b.scroll = wanted;
                                b.list_dirty = true;
                            }
                        })
                        .id();
                    ids.insert("list", list);
                });
            // Aperçu
            root.spawn(Node { flex_grow: 1.0, height: percent(100), flex_direction: FlexDirection::Column, padding: UiRect::all(px(14)), row_gap: px(8), ..default() }).with_children(|panel| {
                panel.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(10), align_items: AlignItems::Center, ..default() }).with_children(|row| {
                    row.spawn((Button, Hovered::default(), item_node(), BackgroundColor(ITEM), CopyButton))
                        .observe(|_: On<Activate>, mut b: ResMut<AssetBrowser>, mut clipboard: ResMut<Clipboard>| {
                            let Some(sel) = b.selected else { return };
                            // rom.bin : le n° seul, la clé de DS Decomp Studio (onglet Data, « Archive (rom.bin) »)
                            let path = b.entries[sel].rom_index.map_or_else(|| b.entries[sel].path.clone(), |n| n.to_string());
                            b.status = match clipboard.set_text(path.clone()) {
                                Ok(()) => format!("Copie : {path}"),
                                Err(e) => format!("Presse-papiers indisponible : {e:?}"),
                            };
                        })
                        .with_child(label("Copier le chemin", 14.0, TEXT));
                    row.spawn((label("", 18.0, TEXT), PathText));
                });
                panel.spawn((label("", 13.0, DIM), InfoText));
                let controls = panel.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(14), row_gap: px(6), flex_wrap: FlexWrap::Wrap, ..default() }).id();
                ids.insert("controls", controls);
                let image_box = panel
                    .spawn((Node { flex_grow: 1.0, width: percent(100), justify_content: JustifyContent::Center, align_items: AlignItems::Center, overflow: Overflow::clip(), ..default() }, BackgroundColor(Color::srgb(0.13, 0.14, 0.16))))
                    .with_children(|b| {
                        let node = b
                            .spawn((ImageNode::default(), Node { width: px(1), height: px(1), ..default() }, Visibility::Hidden, Pickable::default()))
                            .observe(orbit_drag)
                            .observe(orbit_zoom)
                            .id();
                        ids.insert("image", node);
                    })
                    .id();
                ids.insert("image_box", image_box);
            });
        })
        .id();
    // Liste des jeux (par-dessus tout, ouverte par le bouton « Jeu »)
    let overlay = commands
        .spawn((
            Node { position_type: PositionType::Absolute, top: px(48), left: px(10), width: px(1100), flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, column_gap: px(6), row_gap: px(6), padding: UiRect::all(px(10)), border_radius: BorderRadius::all(px(8)), display: Display::None, ..default() },
            BackgroundColor(Color::srgb(0.13, 0.14, 0.17)),
            GlobalZIndex(10),
            UiTargetCamera(camera),
        ))
        .id();
    browser.window = Some(window);
    browser.parts = vec![camera, camera3d, light, root, overlay];
    browser.list = ids.get("list").copied();
    browser.controls = ids.get("controls").copied();
    browser.image_box = ids.get("image_box").copied();
    browser.image_node = ids.get("image").copied();
    browser.camera3d = Some(camera3d);
    browser.light = Some(light);
    browser.games_overlay = Some(overlay);
    browser.view3d = view3d;
    browser.list_dirty = true;
    browser.controls_dirty = true;
    browser.preview_dirty = true;
    if browser.games.is_empty() {
        browser.games = find_games(&browser.root);
        let root_dir = browser.root.display().to_string();
        browser.status = if browser.games.is_empty() {
            format!("Aucun jeu dans {root_dir} (--projects=<dossier> ou NDS_DECOMP_PROJECTS)")
        } else {
            format!("{} jeux dans {root_dir} : choisissez un jeu", browser.games.len())
        };
    }
    browser.games_open = browser.game.is_none() && !browser.games.is_empty();
    if let Some((game, file)) = start {
        let wanted = game.to_lowercase();
        if let Some(i) = browser.games.iter().position(|g| g.label.to_lowercase() == wanted).or_else(|| browser.games.iter().position(|g| !wanted.is_empty() && g.label.to_lowercase().starts_with(&wanted))) {
            browser.open_game(i);
            browser.want_file = file;
        }
    }
}

fn poll_scan(mut b: ResMut<AssetBrowser>) {
    let done = b.scanning.as_ref().and_then(|r| r.lock().ok()?.take());
    if let Some(entries) = done {
        b.scan_done(entries);
    }
}

fn page(activate: On<Activate>, pages: Query<&PageButton>, mut b: ResMut<AssetBrowser>) {
    let Ok(PageButton(dir)) = pages.get(activate.entity) else { return };
    let max = b.shown.len().saturating_sub(ROWS) as i64;
    b.scroll = (b.scroll as i64 + *dir as i64 * (ROWS as i64 - 2)).clamp(0, max.max(0)) as usize;
    b.list_dirty = true;
}

/// Taper du texte dans la fenêtre filtre la liste (Retour arrière efface, Échap vide).
fn type_filter(mut b: ResMut<AssetBrowser>, mut typed: MessageReader<KeyboardInput>, windows: Query<&Window>) {
    let Some(window) = b.window else { return };
    let focused = windows.get(window).is_ok_and(|w| w.focused);
    let mut changed = false;
    for key in typed.read() {
        if !focused || key.window != window || !key.state.is_pressed() {
            continue;
        }
        match &key.logical_key {
            Key::Backspace => changed |= b.filter.pop().is_some(),
            Key::Escape => {
                changed |= !b.filter.is_empty();
                b.filter.clear();
            }
            _ => {
                if let Some(text) = &key.text {
                    let text: String = text.chars().filter(|c| !c.is_control()).collect();
                    if !text.is_empty() {
                        b.filter.push_str(&text);
                        changed = true;
                    }
                }
            }
        }
    }
    if changed {
        b.scroll = 0;
        b.list_dirty = true;
    }
}

fn rebuild_games(mut commands: Commands, mut b: ResMut<AssetBrowser>, mut nodes: Query<&mut Node>, children: Query<&Children>, mut built: Local<bool>) {
    let Some(overlay) = b.games_overlay else {
        *built = false;
        return;
    };
    if let Ok(mut node) = nodes.get_mut(overlay) {
        let display = if b.games_open { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
    }
    if *built && children.get(overlay).is_ok_and(|c| !c.is_empty()) {
        return;
    }
    *built = true;
    let labels: Vec<String> = b.games.iter().map(|g| g.label.clone()).collect();
    commands.entity(overlay).with_children(|list| {
        for (i, name) in labels.into_iter().enumerate() {
            list.spawn((Button, Hovered::default(), item_node(), BackgroundColor(ITEM), GameChoice(i)))
                .observe(|activate: On<Activate>, choices: Query<&GameChoice>, mut b: ResMut<AssetBrowser>| {
                    if let Ok(choice) = choices.get(activate.entity) {
                        b.open_game(choice.0);
                    }
                })
                .with_child(label(name, 14.0, TEXT));
        }
    });
    b.list_dirty = true;
}

fn rebuild_list(mut commands: Commands, mut b: ResMut<AssetBrowser>, children: Query<&Children>) {
    let Some(list) = b.list else { return };
    if !b.list_dirty {
        return;
    }
    b.list_dirty = false;
    b.filter_entries();
    if let Ok(old) = children.get(list) {
        for child in old.iter() {
            commands.entity(child).despawn();
        }
    }
    let rows: Vec<(usize, String, String)> = b.shown.iter().skip(b.scroll).take(ROWS).map(|&i| {
        let e = &b.entries[i];
        let name = match &e.member_name {
            Some(n) => format!("{}  ({n})", e.path),
            None => e.path.clone(),
        };
        (i, name, format!("{}  {}", e.kind.label(), size_text(e.size)))
    }).collect();
    commands.entity(list).with_children(|list| {
        for (i, name, kind) in rows {
            list.spawn((Button, Hovered::default(), Node { height: px(ROW_H), flex_shrink: 0.0, padding: UiRect::horizontal(px(6)), justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::Center, border_radius: BorderRadius::all(px(4)), ..default() }, BackgroundColor(ROW), ListRow(i)))
                .observe(|activate: On<Activate>, rows: Query<&ListRow>, mut b: ResMut<AssetBrowser>| {
                    if let Ok(row) = rows.get(activate.entity) {
                        b.select(row.0);
                    }
                })
                .with_children(|row| {
                    row.spawn((label(name, 13.0, TEXT), Pickable::IGNORE));
                    row.spawn((label(kind, 12.0, DIM), Pickable::IGNORE));
                });
        }
    });
}

fn size_text(size: usize) -> String {
    if size >= 1 << 20 {
        format!("{:.1} Mo", size as f64 / (1 << 20) as f64)
    } else if size >= 1024 {
        format!("{} Ko", size / 1024)
    } else {
        format!("{size} o")
    }
}

fn refresh_texts(
    b: Res<AssetBrowser>,
    mut texts: Query<(&mut Text, Option<&GameButton>, Option<&FilterButton>, Option<&ListTitle>, Option<&InfoText>, Option<&PathText>), Or<(With<GameButton>, With<FilterButton>, With<ListTitle>, With<InfoText>, With<PathText>)>>,
) {
    if b.window.is_none() {
        return;
    }
    for (mut text, game, filter, title, info, path) in &mut texts {
        let wanted = if game.is_some() {
            format!("{}   {}", b.game.map_or("(choisir)", |g| b.games[g].label.as_str()), if b.games_open { "^" } else { "v" })
        } else if filter.is_some() {
            if b.all_files { "Tous les fichiers".into() } else { "Fichiers montrables".into() }
        } else if title.is_some() {
            let range = if b.shown.is_empty() { "0".to_string() } else { format!("{}-{}", b.scroll + 1, (b.scroll + ROWS).min(b.shown.len())) };
            let filter = if b.filter.is_empty() { "tapez pour filtrer".to_string() } else { format!("filtre : {}_", b.filter) };
            format!("{range} / {}   ({filter})", b.shown.len())
        } else if info.is_some() {
            if b.info.is_empty() { b.status.clone() } else { format!("{}\n{}", b.info, b.status) }
        } else if path.is_some() {
            b.selected.map_or(String::new(), |s| match b.entries[s].rom_index {
                Some(n) => format!("{}   (n = {n})", b.entries[s].path),
                None => b.entries[s].path.clone(),
            })
        } else {
            continue;
        };
        if text.0 != wanted {
            text.0 = wanted;
        }
    }
}

fn looks(
    b: Res<AssetBrowser>,
    mut items: Query<(&Hovered, Has<Pressed>, &mut BackgroundColor, Option<&ListRow>, Option<&GameChoice>, Option<&FilterButton>), Or<(With<ListRow>, With<GameChoice>, With<GameButton>, With<FilterButton>, With<CopyButton>, With<PageButton>, With<StepButton>)>>,
) {
    if b.window.is_none() {
        return;
    }
    for (hovered, pressed, mut color, row, game, filter) in &mut items {
        let selected = row.is_some_and(|r| Some(r.0) == b.selected) || game.is_some_and(|g| Some(g.0) == b.game) || (filter.is_some() && b.all_files);
        let idle = if row.is_some() { ROW } else { ITEM };
        let wanted = if selected || pressed { ITEM_ON } else if hovered.get() { ITEM_HOVER } else { idle };
        if color.0 != wanted {
            color.0 = wanted;
        }
    }
}

// --------------------------------------------------------------------------------------------
//  Aperçu
// --------------------------------------------------------------------------------------------

fn rebuild_controls(mut commands: Commands, mut b: ResMut<AssetBrowser>, children: Query<&Children>) {
    let Some(controls) = b.controls else { return };
    if !b.controls_dirty {
        return;
    }
    b.controls_dirty = false;
    if let Ok(old) = children.get(controls) {
        for child in old.iter() {
            commands.entity(child).despawn();
        }
    }
    let c = &b.choices;
    let name = |list: &[usize], i: usize| list.get(i).map_or("aucun".to_string(), |&e| b.entries[e].path.clone());
    let mut steps: Vec<(Ctl, String)> = Vec::new();
    if !c.tiles.is_empty() {
        steps.push((Ctl::Tiles, format!("Tuiles : {}", name(&c.tiles, c.tile))));
    }
    if !c.palettes.is_empty() {
        steps.push((Ctl::Palette, format!("Palette : {}", name(&c.palettes, c.palette))));
    }
    if c.banks > 1 {
        steps.push((Ctl::Bank, format!("Couleurs : {} / {}", c.bank + 1, c.banks)));
    }
    if c.width > 0 {
        steps.push((Ctl::Width, format!("Largeur : {} tuiles", c.width)));
    }
    if c.cells > 0 {
        steps.push((Ctl::Cell, format!("Cellule : {} / {}", c.cell + 1, c.cells)));
    }
    if c.models > 1 {
        steps.push((Ctl::Model, format!("Modele : {} / {}", c.model + 1, c.models)));
    }
    if b.selected.is_some_and(|s| b.entries[s].kind == Kind::Model) {
        let tex = if c.texture == 0 { "celles du modele".to_string() } else { name(&c.textures, c.texture - 1) };
        steps.push((Ctl::Textures, format!("Textures : {tex}")));
    }
    commands.entity(controls).with_children(|row| {
        for (ctl, text) in steps {
            row.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(4), align_items: AlignItems::Center, ..default() }).with_children(|step| {
                step.spawn((Button, Hovered::default(), item_node(), BackgroundColor(ITEM), StepButton(ctl, -1))).observe(step_choice).with_child(label("<", 14.0, TEXT));
                step.spawn(label(text, 14.0, TEXT));
                step.spawn((Button, Hovered::default(), item_node(), BackgroundColor(ITEM), StepButton(ctl, 1))).observe(step_choice).with_child(label(">", 14.0, TEXT));
            });
        }
    });
}

fn step_choice(activate: On<Activate>, steps: Query<&StepButton>, mut b: ResMut<AssetBrowser>) {
    let Ok(&StepButton(ctl, dir)) = steps.get(activate.entity) else { return };
    let wrap = |v: usize, n: usize| if n == 0 { 0 } else { (v as i64 + dir as i64).rem_euclid(n as i64) as usize };
    let c = &mut b.choices;
    match ctl {
        Ctl::Palette => c.palette = wrap(c.palette, c.palettes.len()),
        Ctl::Tiles => c.tile = wrap(c.tile, c.tiles.len()),
        Ctl::Textures => c.texture = wrap(c.texture, c.textures.len() + 1),
        Ctl::Bank => c.bank = wrap(c.bank, c.banks),
        Ctl::Width => c.width = (c.width as i64 + dir as i64).clamp(1, 256) as u32,
        Ctl::Cell => c.cell = wrap(c.cell, c.cells),
        Ctl::Model => c.model = wrap(c.model, c.models),
    }
    b.preview_dirty = true;
    b.controls_dirty = true;
}

fn to_image(rgba: &Rgba, sampler: ImageSampler) -> Image {
    let mut image = Image::new(
        Extent3d { width: rgba.width.max(1), height: rgba.height.max(1), depth_or_array_layers: 1 },
        TextureDimension::D2,
        if rgba.data.is_empty() { vec![0; 4] } else { rgba.data.clone() },
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = sampler;
    image
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

#[allow(clippy::too_many_arguments)]
fn render_preview(
    mut commands: Commands,
    mut b: ResMut<AssetBrowser>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    old: Query<Entity, With<Preview3d>>,
    mut cameras: Query<&mut Camera>,
) {
    if b.window.is_none() || !b.preview_dirty {
        return;
    }
    b.preview_dirty = false;
    for e in &old {
        commands.entity(e).despawn();
    }
    b.is3d = false;
    b.image = None;
    b.info.clear();
    if let Some(sel) = b.selected {
        let result = preview(&mut b, sel, &mut commands, &mut images, &mut meshes, &mut materials);
        if let Err(e) = result {
            b.info = format!("{}  :  impossible de l'afficher : {e}", b.entries[sel].kind.label());
        }
    }
    if let Some(mut camera) = b.camera3d.and_then(|c| cameras.get_mut(c).ok()) {
        camera.is_active = b.is3d;
    }
}

fn preview(b: &mut AssetBrowser, sel: usize, commands: &mut Commands, images: &mut Assets<Image>, meshes: &mut Assets<Mesh>, materials: &mut Assets<StandardMaterial>) -> Result<(), String> {
    let bytes = read_entry(&b.entries[sel])?;
    let kind = b.entries[sel].kind;
    let load = |b: &AssetBrowser, list: &[usize], i: usize| list.get(i).map(|&e| read_entry(&b.entries[e])).transpose();
    let size = b.entries[sel].size;
    let mut show = |b: &mut AssetBrowser, rgba: &Rgba, info: String| {
        b.image = Some((images.add(to_image(rgba, ImageSampler::nearest())), UVec2::new(rgba.width, rgba.height)));
        b.info = format!("{}  ({})  -  {info}", kind.label(), size_text(size));
    };
    match kind {
        Kind::Palette => {
            let p = g2d::read_palette(&bytes).map_err(|e| e.to_string())?;
            show(b, &p.swatches(), format!("{} couleurs", p.colors.len()));
        }
        Kind::Tiles | Kind::Screen | Kind::Cells => {
            let tiles_bytes = if kind == Kind::Tiles { Some(bytes.clone()) } else { load(b, &b.choices.tiles, b.choices.tile)? };
            let tiles = g2d::read_tiles(tiles_bytes.as_deref().ok_or("aucun fichier de tuiles (.ncgr) trouve")?).map_err(|e| e.to_string())?;
            let palette = match load(b, &b.choices.palettes, b.choices.palette)? {
                Some(p) => g2d::read_palette(&p).map_err(|e| e.to_string())?,
                // Sans palette : niveaux de gris
                None => g2d::Palette { colors: (0..256).map(|i| if tiles.bpp8 { [i as u8, i as u8, i as u8, 255] } else { let v = (i % 16 * 17) as u8; [v, v, v, 255] }).collect() },
            };
            let c = &mut b.choices;
            c.banks = if tiles.bpp8 { 1 } else { palette.banks() };
            c.bank = c.bank.min(c.banks.saturating_sub(1));
            let what = format!("{} tuiles {} bpp{}", tiles.count(), if tiles.bpp8 { 8 } else { 4 }, if tiles.linear { " (bitmap)" } else { "" });
            match kind {
                Kind::Tiles => {
                    if c.width == 0 {
                        c.width = tiles.default_width();
                    }
                    let img = g2d::render_tiles(&tiles, &palette, c.bank, c.width);
                    show(b, &img, what);
                }
                Kind::Screen => {
                    let screen = g2d::read_screen(&bytes).map_err(|e| e.to_string())?;
                    let img = g2d::render_screen(&screen, &tiles, &palette);
                    show(b, &img, format!("ecran {}x{}, {what}", screen.width, screen.height));
                }
                _ => {
                    let cells = g2d::read_cells(&bytes).map_err(|e| e.to_string())?;
                    c.cells = cells.cells.len();
                    c.cell = c.cell.min(c.cells.saturating_sub(1));
                    let objs = cells.cells.get(c.cell).map_or(0, |o| o.len());
                    let img = g2d::render_cell(&cells, c.cell, &tiles, &palette);
                    show(b, &img, format!("{} cellules ({objs} OBJ), {what}", cells.cells.len()));
                }
            }
        }
        Kind::Textures => {
            let cont = model3d::read(&bytes).map_err(|e| e.to_string())?;
            let all = model3d::texture_sheet(&cont);
            let ok: Vec<Rgba> = all.iter().filter_map(|(_, r)| r.as_ref().ok().cloned()).collect();
            let names: Vec<String> = all.iter().map(|(n, _)| n.clone()).take(12).collect();
            let more = if all.len() > 12 { ", ..." } else { "" };
            show(b, &sheet(&ok), format!("{} textures, {} palettes : {}{more}", cont.textures.len(), cont.palettes.len(), names.join(", ")));
        }
        Kind::Model => {
            let own = model3d::read(&bytes).map_err(|e| e.to_string())?;
            let others = match b.choices.texture {
                0 => Vec::new(),
                n => match load(b, &b.choices.textures, n - 1)? {
                    Some(t) => vec![model3d::read(&t).map_err(|e| e.to_string())?],
                    None => Vec::new(),
                },
            };
            let models = model3d::build_models(&own, &others);
            b.choices.models = models.len();
            b.choices.model = b.choices.model.min(models.len().saturating_sub(1));
            let m = models.get(b.choices.model).ok_or("aucun modele dans le fichier")?;
            spawn_model(m, commands, images, meshes, materials);
            let center = (Vec3::from(m.min) + Vec3::from(m.max)) / 2.0;
            let radius = ((Vec3::from(m.max) - Vec3::from(m.min)).length() / 2.0).max(0.01);
            b.orbit = Orbit { target: center, radius, distance: radius * 2.6, ..Orbit::default() };
            b.is3d = true;
            b.image = Some((b.view3d.clone(), UVec2::new(VIEW_W, VIEW_H)));
            let warn = if m.warnings.is_empty() { String::new() } else { format!("\n! {}", m.warnings.join(" ; ")) };
            b.info = format!(
                "modele 3D ({})  -  {} : {} triangles, {} sommets, {} materiaux, {} textures dans le fichier{warn}",
                size_text(size),
                m.name,
                m.triangle_count,
                m.vertex_count,
                m.materials.len(),
                own.textures.len()
            );
        }
        _ => return Err("format non pris en charge".into()),
    }
    Ok(())
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn spawn_model(m: &model3d::ModelOut, commands: &mut Commands, images: &mut Assets<Image>, meshes: &mut Assets<Mesh>, materials: &mut Assets<StandardMaterial>) {
    let mut cache: HashMap<(usize, bool), Handle<StandardMaterial>> = HashMap::new();
    let textures: Vec<Option<(Handle<Image>, bool, bool)>> = m
        .materials
        .iter()
        .map(|mat| {
            mat.texture.as_ref().map(|t| {
                let mode = |repeat: bool, mirror: bool| if !repeat { ImageAddressMode::ClampToEdge } else if mirror { ImageAddressMode::MirrorRepeat } else { ImageAddressMode::Repeat };
                let sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                    address_mode_u: mode(mat.repeat_s, mat.mirror_s),
                    address_mode_v: mode(mat.repeat_t, mat.mirror_t),
                    mag_filter: ImageFilterMode::Nearest,
                    min_filter: ImageFilterMode::Nearest,
                    ..default()
                });
                let alphas = t.data.chunks_exact(4).map(|p| p[3]);
                let partial = alphas.clone().any(|a| a > 0 && a < 255);
                let holes = alphas.clone().any(|a| a == 0);
                (images.add(to_image(t, sampler)), partial, holes)
            })
        })
        .collect();
    for part in &m.parts {
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, part.positions.clone());
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, part.normals.clone());
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, part.uvs.clone());
        let colors: Vec<[f32; 4]> = part.colors.iter().map(|c| [srgb_to_linear(c[0]), srgb_to_linear(c[1]), srgb_to_linear(c[2]), c[3]]).collect();
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
        mesh.insert_indices(Indices::U32(part.indices.clone()));
        let material = cache
            .entry((part.material, part.lit))
            .or_insert_with(|| {
                let mat = m.materials.get(part.material);
                let alpha = mat.map_or(1.0, |m| m.alpha);
                let tex = textures.get(part.material).cloned().flatten();
                let alpha_mode = match &tex {
                    _ if alpha < 0.99 => AlphaMode::Blend,
                    Some((_, true, _)) => AlphaMode::Blend,
                    Some((_, false, true)) => AlphaMode::Mask(0.5),
                    _ => AlphaMode::Opaque,
                };
                materials.add(StandardMaterial {
                    base_color: Color::srgba(1.0, 1.0, 1.0, alpha),
                    base_color_texture: tex.map(|t| t.0),
                    unlit: !part.lit,
                    alpha_mode,
                    cull_mode: None,
                    double_sided: true,
                    perceptual_roughness: 1.0,
                    reflectance: 0.0,
                    ..default()
                })
            })
            .clone();
        commands.spawn((Mesh3d(meshes.add(mesh)), MeshMaterial3d(material), Transform::default(), RenderLayers::layer(LAYER), Preview3d));
    }
}

/// L'image (2D, ou la vue 3D) remplit le panneau en gardant ses proportions ; les petites
/// images 2D sont agrandies d'un nombre entier de fois.
fn fit_image(b: Res<AssetBrowser>, boxes: Query<&ComputedNode>, mut nodes: Query<(&mut Node, &mut ImageNode, &mut Visibility)>) {
    let (Some(image_box), Some(image_node)) = (b.image_box, b.image_node) else { return };
    let Ok(area) = boxes.get(image_box) else { return };
    let Ok((mut node, mut image, mut visibility)) = nodes.get_mut(image_node) else { return };
    let Some((handle, size)) = &b.image else {
        if *visibility != Visibility::Hidden {
            *visibility = Visibility::Hidden;
        }
        return;
    };
    let avail = area.size() * area.inverse_scale_factor() - Vec2::splat(8.0);
    let size = size.as_vec2().max(Vec2::ONE);
    let mut scale = (avail.x / size.x).min(avail.y / size.y).max(0.01);
    if !b.is3d && scale >= 1.0 {
        scale = scale.floor().min(8.0);
    }
    let (w, h) = (px(size.x * scale), px(size.y * scale));
    if node.width != w || node.height != h {
        node.width = w;
        node.height = h;
    }
    if image.image != *handle {
        image.image = handle.clone();
    }
    if *visibility != Visibility::Inherited {
        *visibility = Visibility::Inherited;
    }
}

fn orbit_drag(drag: On<PointerDrag>, mut b: ResMut<AssetBrowser>) {
    if !b.is3d {
        return;
    }
    let d = drag.delta;
    let o = &mut b.orbit;
    match drag.button {
        PointerButton::Primary => {
            o.yaw -= d.x * 0.008;
            o.pitch = (o.pitch + d.y * 0.008).clamp(-1.55, 1.55);
        }
        _ => {
            let rot = Quat::from_euler(EulerRot::YXZ, o.yaw, -o.pitch, 0.0);
            let step = o.distance * 0.0015;
            o.target += rot * Vec3::new(-d.x * step, d.y * step, 0.0);
        }
    }
}

fn orbit_zoom(scroll: On<PointerScroll>, mut b: ResMut<AssetBrowser>) {
    if !b.is3d {
        return;
    }
    let lines = match scroll.unit {
        MouseScrollUnit::Line => scroll.y,
        MouseScrollUnit::Pixel => scroll.y / 40.0,
    };
    let o = &mut b.orbit;
    o.distance = (o.distance * 0.88f32.powf(lines)).clamp(o.radius * 0.05, o.radius * 50.0);
}

fn place_camera(b: Res<AssetBrowser>, mut cameras: Query<(&mut Transform, &mut Projection), With<Camera3d>>, mut lights: Query<&mut Transform, (With<DirectionalLight>, Without<Camera3d>)>) {
    let (Some(camera), Some(light)) = (b.camera3d, b.light) else { return };
    if !b.is3d {
        return;
    }
    let o = &b.orbit;
    let rot = Quat::from_euler(EulerRot::YXZ, o.yaw, -o.pitch, 0.0);
    let eye = o.target + rot * Vec3::new(0.0, 0.0, o.distance);
    if let Ok((mut transform, mut projection)) = cameras.get_mut(camera) {
        *transform = Transform::from_translation(eye).looking_at(o.target, Vec3::Y);
        let near = (o.radius * 0.01).max(0.0005);
        let wanted = Projection::Perspective(PerspectiveProjection { near, far: o.radius * 200.0 + o.distance, ..default() });
        if let (Projection::Perspective(p), Projection::Perspective(w)) = (&*projection, &wanted) {
            if p.near != w.near || p.far != w.far {
                *projection = wanted;
            }
        }
    }
    // La lumière vient de derrière l'épaule de la caméra
    if let Ok(mut transform) = lights.get_mut(light) {
        *transform = Transform::from_rotation(rot * Quat::from_euler(EulerRot::YXZ, 0.5, -0.6, 0.0));
    }
}
