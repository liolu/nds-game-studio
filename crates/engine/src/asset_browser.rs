//! Fenêtre Assets (F6) : les fichiers d'un jeu, lus dans sa rom, modèles 3D et images aux formats
//! Nintendo standard, pour reconnaître et nommer les objets du jeu.
//!
//! - les jeux sont les roms du dossier `rom` du launcher (`library`) ;
//! - la liste montre chaque fichier avec son chemin dans NitroFS (celui que le code du jeu
//!   charge ; `archive.narc#n` pour le n-ième fichier d'une archive), reconnu par sa signature ;
//!   taper du texte filtre la liste, la molette la fait défiler ;
//! - clic = aperçu : `.nsbmd` en 3D (glisser = tourner, clic droit / milieu = déplacer, molette =
//!   zoom), textures `.nsbtx`, palettes `.nclr`, tuiles `.ncgr`/`.ncbr`, écrans `.nscr`, cellules
//!   `.ncer` ; la palette, les tuiles ou les textures qui vont avec sont devinées (même dossier,
//!   nom le plus proche) et se changent avec les flèches ;
//! - le menu des jeux a une recherche et donne le pourcentage extrait et regardable de chaque
//!   jeu ; le menu des formats liste toutes les extensions des jeux, cochées si l'app les montre
//!   (voir `asset_scan`) ;
//! - « Copier le chemin » met le chemin NitroFS dans le presse-papiers (pour le relier aux
//!   fonctions du jeu dans DS Decomp Studio) ;
//! - section « rom.bin » : les fichiers de l'archive numérotée rom.bin (Lego Star Wars II, Les
//!   Sims 2...), lus et dépaquetés depuis la rom (`romarc`), montrés `rom.bin#n` ; « Copier le chemin » y copie le n° n seul (clé de DS Decomp Studio) et les
//!   textures d'un modèle sont cherchées dans les `.nsbtx` aux numéros les plus proches.
//!
//! Lecture des formats : crate `nds-nitro` (code d'apicula pour la 3D).

use std::collections::HashMap;
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
use nds_nitro::{model3d, Kind, Rgba};

use crate::i18n::{t, tr, Language, Tr};
use crate::library;
use crate::asset_scan::{all_formats, choices_for, decode, find_games, run_index, scan, kind_text, size_text, Choices, Decoded, Entry, FormatStat, GameDir, IndexState};

/// La fenêtre des assets (la fermer ne quitte pas le studio).
#[derive(Component)]
pub struct AssetsWindow;

/// Ouvrir ou fermer la fenêtre des assets (comme F6).
#[derive(Message)]
pub struct ToggleAssets;

/// Calque des modèles 3D montrés (ni les jeux ni les écrans ne l'utilisent).
const LAYER: usize = 24;
const ROWS: usize = 36;
const ROW_H: f32 = 21.0;
/// Menus déroulants : largeur, lignes visibles, hauteur d'une ligne, largeur d'une colonne.
const MENU_W: f32 = 860.0;
const MENU_ROWS: usize = 28;
const MENU_ROW_H: f32 = 23.0;
const MENU_COLUMN: f32 = 100.0;
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

/// Le menu déroulant ouvert.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Menu {
    None,
    Games,
    Formats,
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

#[derive(Resource)]
pub(crate) struct AssetBrowser {
    window: Option<Entity>,
    parts: Vec<Entity>,
    list: Option<Entity>,
    menu_overlay: Option<Entity>,
    controls: Option<Entity>,
    image_box: Option<Entity>,
    image_node: Option<Entity>,
    camera3d: Option<Entity>,
    light: Option<Entity>,
    view3d: Handle<Image>,
    games: Vec<GameDir>,
    game: Option<usize>,
    menu: Menu,
    menu_query: String,
    menu_scroll: usize,
    menu_len: usize,
    menu_dirty: bool,
    index: Arc<Mutex<IndexState>>,
    index_started: bool,
    index_seen: u64,
    entries: Vec<Entry>,
    shown: Vec<usize>,
    filter: String,
    /// Format choisi dans le menu des formats : seuls ses fichiers sont listés.
    format_filter: Option<String>,
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
    start_menu: Option<(Menu, String)>,
}

impl Default for AssetBrowser {
    fn default() -> Self {
        AssetBrowser {
            window: None,
            parts: Vec::new(),
            list: None,
            menu_overlay: None,
            controls: None,
            image_box: None,
            image_node: None,
            camera3d: None,
            light: None,
            view3d: Handle::default(),
            games: Vec::new(),
            game: None,
            menu: Menu::None,
            menu_query: String::new(),
            menu_scroll: 0,
            menu_len: 0,
            menu_dirty: false,
            index: Arc::default(),
            index_started: false,
            index_seen: 0,
            entries: Vec::new(),
            shown: Vec::new(),
            filter: String::new(),
            format_filter: None,
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
            start_menu: None,
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
struct FormatButton;

/// L'extension du format, et si tous ses fichiers sont regardables.
#[derive(Component)]
struct FormatChoice(String, bool);

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
        let (mut game, mut file, mut open) = (None, None, false);
        for arg in std::env::args().skip(1) {
            if arg == "--assets" {
                open = true;
            } else if let Some(v) = arg.strip_prefix("--assets=") {
                (open, game) = (true, Some(v.to_string()));
            } else if let Some(v) = arg.strip_prefix("--asset=") {
                file = Some(v.replace('\\', "/"));
            } else if let Some(v) = arg.strip_prefix("--assets-menu=") {
                // Menu ouvert au lancement : `games` ou `formats`, et sa recherche : `formats:nsb`
                let (menu, query) = v.split_once(':').unwrap_or((v, ""));
                browser.start_menu = Some((if menu == "formats" { Menu::Formats } else { Menu::Games }, query.to_string()));
            }
        }
        if open {
            browser.start = Some((game.unwrap_or_default(), file));
        }
        app.insert_resource(browser).add_message::<ToggleAssets>().add_systems(
            Update,
            (redo_on_language, toggle_window, poll_scan, type_filter, rebuild_menu, rebuild_list, render_preview, rebuild_controls, refresh_texts, fit_image, place_camera, looks).chain(),
        );
    }
}

fn label(text: impl Into<String>, size: f32, color: Color) -> impl Bundle {
    (Text::new(text), TextFont::from_font_size(size), TextColor(color))
}

fn item_node() -> Node {
    Node { padding: UiRect::axes(px(12), px(6)), border_radius: BorderRadius::all(px(6)), justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() }
}

fn search_words(query: &str) -> Vec<String> {
    query.to_lowercase().split_whitespace().map(String::from).collect()
}

fn matches_words(text: &str, words: &[String]) -> bool {
    let text = text.to_lowercase();
    words.iter().all(|w| text.contains(w.as_str()))
}

// --------------------------------------------------------------------------------------------
//  Jeux et fichiers
// --------------------------------------------------------------------------------------------

impl AssetBrowser {
    /// Ouvre la fenêtre (si besoin) sur cette rom (nom de fichier).
    pub(crate) fn show_game(&mut self, file: &str) {
        self.start = Some((file.to_string(), None));
    }

    /// Une rom par son nom de fichier ou son libellé (« Titre (CODE) », ou son début).
    fn find_game(&self, game: &str) -> Option<usize> {
        let wanted = game.to_lowercase();
        let file_is = |g: &GameDir| g.rom.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase() == wanted);
        self.games.iter().position(file_is).or_else(|| self.games.iter().position(|g| g.label.to_lowercase() == wanted)).or_else(|| self.games.iter().position(|g| !wanted.is_empty() && g.label.to_lowercase().starts_with(&wanted)))
    }

    /// Relit le dossier des roms (une rom a pu être ajoutée dans le launcher). Si la liste a changé,
    /// l'index est complété dès que le calcul en cours est fini.
    fn refresh_games(&mut self) {
        let current = self.game.map(|g| self.games[g].rom.clone());
        let found = find_games(&library::rom_dir());
        let same = found.len() == self.games.len() && found.iter().zip(&self.games).all(|(a, b)| a.rom == b.rom);
        self.games = found;
        self.game = current.as_ref().and_then(|rom| self.games.iter().position(|g| g.rom == *rom));
        if current.is_some() && self.game.is_none() {
            self.entries.clear();
            self.selected = None;
            self.image = None;
            self.is3d = false;
            self.info.clear();
            self.list_dirty = true;
            self.controls_dirty = true;
            self.preview_dirty = true;
        }
        if !same {
            let index = self.index.lock().unwrap();
            if index.done >= index.total {
                self.index_started = false;
            }
            drop(index);
        }
    }

    fn open_menu(&mut self, menu: Menu) {
        self.menu = if self.menu == menu { Menu::None } else { menu };
        self.menu_query.clear();
        self.menu_scroll = 0;
        self.menu_dirty = true;
    }

    fn close_menu(&mut self) {
        self.menu = Menu::None;
        self.menu_dirty = true;
    }

    /// Les jeux qui correspondent à la recherche, de haut en bas.
    fn matching_games(&self) -> Vec<usize> {
        let words = search_words(&self.menu_query);
        (0..self.games.len()).filter(|&i| matches_words(&self.games[i].label, &words)).collect()
    }

    fn matching_formats(&self) -> Vec<(String, FormatStat)> {
        let words = search_words(&self.menu_query);
        let index = self.index.lock().unwrap();
        all_formats(&index.stats).into_iter().filter(|(ext, _)| matches_words(ext, &words)).collect()
    }

    fn pick_format(&mut self, ext: &str, all_viewable: bool) {
        self.close_menu();
        if self.game.is_none() {
            self.status = tr("Choose a game first: the format filters its files", "Choisissez d'abord un jeu : le format filtre ses fichiers").into();
            return;
        }
        self.filter.clear();
        self.format_filter = Some(ext.to_string());
        self.all_files = !all_viewable;
        self.scroll = 0;
        self.list_dirty = true;
    }

    fn open_game(&mut self, index: usize) {
        self.game = Some(index);
        self.close_menu();
        self.entries.clear();
        self.format_filter = None;
        let result = Arc::new(Mutex::new(None));
        let (rom, out) = (self.games[index].rom.clone(), result.clone());
        std::thread::spawn(move || {
            let entries = scan(&rom);
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
        self.status = t!("Reading the files of {}...", "Lecture des fichiers de {}...", self.games[index].label);
    }

    fn scan_done(&mut self, entries: Vec<Entry>) {
        self.entries = entries;
        self.scanning = None;
        self.list_dirty = true;
        let viewable = self.entries.iter().filter(|e| e.kind.viewable()).count();
        self.status = t!("{} files, {} viewable (standard Nintendo formats)", "{} fichiers, {} montrables (formats Nintendo standard)", self.entries.len(), viewable);
        let romarc = self.entries.iter().filter(|e| e.rom_index.is_some()).count();
        if romarc > 0 {
            self.status += &t!(" ; rom.bin: {romarc} files (filter 'rom.bin')", " ; rom.bin : {romarc} fichiers (filtre 'rom.bin')");
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
                if self.format_filter.as_ref().is_some_and(|f| *f != e.format()) {
                    return false;
                }
                let hay = format!("{} {} {}", e.path.to_lowercase(), e.member_name.as_deref().unwrap_or("").to_lowercase(), e.kind.label());
                words.iter().all(|w| hay.contains(w.as_str()))
            })
            .collect();
        self.scroll = self.scroll.min(self.shown.len().saturating_sub(ROWS));
    }

    fn select(&mut self, index: usize) {
        self.selected = Some(index);
        self.choices = choices_for(&self.entries, index);
        self.orbit = Orbit::default();
        self.preview_dirty = true;
        self.controls_dirty = true;
    }
}

// --------------------------------------------------------------------------------------------
//  Fenêtre
// --------------------------------------------------------------------------------------------

/// Les listes sont refaites dans la nouvelle langue.
fn redo_on_language(language: Res<Language>, mut browser: ResMut<AssetBrowser>) {
    if language.is_changed() && !language.is_added() {
        browser.menu_dirty = true;
        browser.list_dirty = true;
        browser.controls_dirty = true;
        browser.preview_dirty = true;
    }
}

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
    // Déjà ouverte et un jeu demandé (launcher) : elle reste ouverte et montre ce jeu
    if let (Some(_), Some((game, _))) = (browser.window, &start) {
        browser.refresh_games();
        if let Some(i) = browser.find_game(game) {
            browser.menu = Menu::None;
            browser.menu_dirty = true;
            browser.open_game(i);
        }
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
            root.spawn((Node { width: px(560), flex_shrink: 0.0, height: percent(100), flex_direction: FlexDirection::Column, padding: UiRect::all(px(10)), row_gap: px(6), ..default() }, BackgroundColor(SIDEBAR)))
                .with_children(|side| {
                    side.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(8), align_items: AlignItems::Center, ..default() }).with_children(|row| {
                        row.spawn((label(tr("Game:", "Jeu :"), 15.0, DIM), Tr("Game:", "Jeu :")));
                        row.spawn((Button, Hovered::default(), Node { flex_grow: 1.0, justify_content: JustifyContent::FlexStart, ..item_node() }, BackgroundColor(ITEM), GameButton))
                            .observe(|_: On<Activate>, mut b: ResMut<AssetBrowser>| {
                                b.open_menu(Menu::Games);
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
                        row.spawn((Button, Hovered::default(), item_node(), BackgroundColor(ITEM), FormatButton))
                            .observe(|_: On<Activate>, mut b: ResMut<AssetBrowser>| {
                                b.open_menu(Menu::Formats);
                            })
                            .with_child((label("", 14.0, TEXT), FormatButton));
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
                                Ok(()) => t!("Copied: {path}", "Copie : {path}"),
                                Err(e) => t!("Clipboard unavailable: {e:?}", "Presse-papiers indisponible : {e:?}"),
                            };
                        })
                        .with_child((label(tr("Copy the path", "Copier le chemin"), 14.0, TEXT), Tr("Copy the path", "Copier le chemin")));
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
    // Menus déroulants des jeux et des formats (par-dessus tout)
    let overlay = commands
        .spawn((
            Node { position_type: PositionType::Absolute, top: px(48), left: px(10), width: px(MENU_W), flex_direction: FlexDirection::Column, row_gap: px(2), padding: UiRect::all(px(10)), border_radius: BorderRadius::all(px(8)), display: Display::None, ..default() },
            BackgroundColor(Color::srgb(0.13, 0.14, 0.17)),
            GlobalZIndex(10),
            UiTargetCamera(camera),
            Pickable::default(),
        ))
        .observe(|scroll: On<PointerScroll>, mut b: ResMut<AssetBrowser>| {
            let lines = match scroll.unit {
                MouseScrollUnit::Line => scroll.y * 3.0,
                MouseScrollUnit::Pixel => scroll.y / MENU_ROW_H,
            };
            let max = b.menu_len.saturating_sub(MENU_ROWS) as i64;
            let wanted = (b.menu_scroll as i64 - lines.round() as i64).clamp(0, max.max(0)) as usize;
            if wanted != b.menu_scroll {
                b.menu_scroll = wanted;
                b.menu_dirty = true;
            }
        })
        .id();
    browser.window = Some(window);
    browser.parts = vec![camera, camera3d, light, root, overlay];
    browser.list = ids.get("list").copied();
    browser.controls = ids.get("controls").copied();
    browser.image_box = ids.get("image_box").copied();
    browser.image_node = ids.get("image").copied();
    browser.camera3d = Some(camera3d);
    browser.light = Some(light);
    browser.menu_overlay = Some(overlay);
    browser.view3d = view3d;
    browser.list_dirty = true;
    browser.controls_dirty = true;
    browser.preview_dirty = true;
    browser.refresh_games();
    if browser.game.is_none() {
        browser.status = if browser.games.is_empty() {
            tr("No games: drag a rom into the launcher", "Aucun jeu : glissez une rom dans le launcher").to_string()
        } else {
            t!("{} games: choose a game", "{} jeux : choisissez un jeu", browser.games.len())
        };
    }
    browser.menu = if browser.game.is_none() && !browser.games.is_empty() { Menu::Games } else { Menu::None };
    browser.menu_query.clear();
    browser.menu_scroll = 0;
    browser.menu_dirty = true;
    // Pourcentages et formats de tous les jeux, calculés une fois puis gardés
    if !browser.index_started {
        browser.index_started = true;
        let state = browser.index.clone();
        std::thread::spawn(move || run_index(&library::rom_dir(), &state, false));
    }
    if let Some((game, file)) = start {
        if let Some(i) = browser.find_game(&game) {
            browser.open_game(i);
            browser.want_file = file;
        }
    }
    if let Some((menu, query)) = browser.start_menu.take() {
        browser.menu = menu;
        browser.menu_query = query;
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

/// Taper du texte : recherche du menu ouvert, sinon filtre de la liste (Retour arrière efface,
/// Échap vide puis ferme le menu, Entrée prend le premier résultat du menu).
fn type_filter(mut b: ResMut<AssetBrowser>, mut typed: MessageReader<KeyboardInput>, windows: Query<&Window>) {
    let Some(window) = b.window else { return };
    let focused = windows.get(window).is_ok_and(|w| w.focused);
    let in_menu = b.menu != Menu::None;
    let mut changed = false;
    for key in typed.read() {
        if !focused || key.window != window || !key.state.is_pressed() {
            continue;
        }
        let b = &mut *b;
        let query = if in_menu { &mut b.menu_query } else { &mut b.filter };
        match &key.logical_key {
            Key::Backspace => changed |= query.pop().is_some(),
            Key::Escape => {
                if in_menu && query.is_empty() {
                    b.close_menu();
                    return;
                }
                changed |= !query.is_empty();
                query.clear();
                if !in_menu {
                    changed |= b.format_filter.take().is_some();
                }
            }
            Key::Enter => {
                if in_menu {
                    match b.menu {
                        Menu::Games => {
                            if let Some(&first) = b.matching_games().first() {
                                b.open_game(first);
                            }
                        }
                        _ => {
                            if let Some((ext, stat)) = b.matching_formats().first() {
                                b.pick_format(ext, stat.viewable == stat.count);
                            }
                        }
                    }
                    return;
                }
            }
            _ => {
                if let Some(text) = &key.text {
                    let text: String = text.chars().filter(|c| !c.is_control()).collect();
                    if !text.is_empty() {
                        query.push_str(&text);
                        changed = true;
                    }
                }
            }
        }
    }
    if changed {
        if in_menu {
            b.menu_scroll = 0;
            b.menu_dirty = true;
        } else {
            b.scroll = 0;
            b.list_dirty = true;
        }
    }
}

fn percent_text(value: f64) -> String {
    if value >= 99.95 || value == 0.0 { format!("{value:.0} %") } else { format!("{value:.1} %") }
}

/// Dessine le menu ouvert : barre de recherche, puis une ligne par jeu ou par format.
fn rebuild_menu(mut commands: Commands, mut b: ResMut<AssetBrowser>, mut nodes: Query<&mut Node>, children: Query<&Children>) {
    let Some(overlay) = b.menu_overlay else { return };
    let (revision, done, total) = {
        let index = b.index.lock().unwrap();
        (index.revision, index.done, index.total)
    };
    if revision != b.index_seen {
        b.index_seen = revision;
        b.menu_dirty = true;
    }
    if !b.menu_dirty {
        return;
    }
    b.menu_dirty = false;
    if let Ok(mut node) = nodes.get_mut(overlay) {
        node.display = if b.menu == Menu::None { Display::None } else { Display::Flex };
        node.top = px(if b.menu == Menu::Formats { 80 } else { 48 });
    }
    if let Ok(old) = children.get(overlay) {
        for child in old.iter() {
            commands.entity(child).despawn();
        }
    }
    if b.menu == Menu::None {
        return;
    }
    let query = if b.menu_query.is_empty() { tr("type to search", "tapez pour chercher").to_string() } else { format!("{}_", b.menu_query) };
    let progress = if done < total { t!("analyzing the games: {done} / {total}", "analyse des jeux : {done} / {total}") } else { String::new() };
    // Une ligne : (coche et sa couleur, nom, colonnes de droite, action)
    enum Pick {
        Game(usize),
        Format(String, bool),
    }
    let mut rows: Vec<(Option<(&'static str, Color)>, String, Vec<String>, Pick)> = Vec::new();
    let (columns, legend): (Vec<&str>, &str) = match b.menu {
        Menu::Games => {
            let index = b.index.lock().unwrap();
            for &i in b.matching_games().iter() {
                let label = b.games[i].label.clone();
                let cells = match index.stats.get(&label) {
                    Some(stat) => vec![percent_text(stat.extracted_percent()), percent_text(stat.viewable_percent())],
                    None => vec!["...".into(), "...".into()],
                };
                rows.push((None, label, cells, Pick::Game(i)));
            }
            (vec![tr("extracted", "extrait"), tr("viewable", "regardable")], tr("extracted = data bytes readable file by file ; viewable = files the app can show", "extrait = octets de donnees lisibles fichier par fichier ; regardable = fichiers que l'app sait montrer"))
        }
        _ => {
            for (ext, stat) in b.matching_formats() {
                let check = stat.check();
                let color = match check {
                    "[x]" => Color::srgb(0.4, 0.85, 0.45),
                    "[~]" => Color::srgb(0.9, 0.75, 0.3),
                    "[!]" => Color::srgb(0.95, 0.4, 0.35),
                    _ => DIM,
                };
                let name = if ext.starts_with('(') { ext.clone() } else { format!(".{ext}") };
                let all_viewable = stat.viewable == stat.count;
                rows.push((Some((check, color)), name, vec![stat.count.to_string(), stat.viewable.to_string(), stat.games.to_string()], Pick::Format(ext, all_viewable)));
            }
            (vec![tr("files", "fichiers"), tr("viewable", "regardables"), tr("games", "jeux")], tr("[x] viewable   [~] partly   [!] the test failed   [ ] not in the app   ;  click = filter the open game", "[x] regardable   [~] en partie   [!] l'essai a echoue   [ ] pas dans l'app   ;  clic = filtrer le jeu ouvert"))
        }
    };
    b.menu_len = rows.len();
    b.menu_scroll = b.menu_scroll.min(rows.len().saturating_sub(MENU_ROWS));
    let (first, count) = (b.menu_scroll + usize::from(!rows.is_empty()), rows.len());
    let last = (b.menu_scroll + MENU_ROWS).min(count);
    let scroll = b.menu_scroll;
    commands.entity(overlay).with_children(|menu| {
        // Barre de recherche
        menu.spawn((Node { padding: UiRect::axes(px(10), px(6)), border_radius: BorderRadius::all(px(6)), margin: UiRect::bottom(px(4)), ..default() }, BackgroundColor(Color::srgb(0.07, 0.075, 0.09)))).with_child(label(t!("Search: {query}", "Recherche : {query}"), 15.0, TEXT));
        menu.spawn(label(format!("{first}-{last} / {count}   {progress}"), 12.0, DIM));
        menu.spawn(label(legend, 12.0, DIM));
        menu.spawn(Node { flex_direction: FlexDirection::Row, justify_content: JustifyContent::FlexEnd, column_gap: px(8), padding: UiRect::horizontal(px(6)), ..default() }).with_children(|head| {
            for column in &columns {
                head.spawn((label(*column, 12.0, DIM), Node { width: px(MENU_COLUMN), ..default() }));
            }
        });
        for (check, name, cells, pick) in rows.into_iter().skip(scroll).take(MENU_ROWS) {
            let mut row = menu.spawn((Button, Hovered::default(), Node { height: px(MENU_ROW_H), flex_shrink: 0.0, padding: UiRect::horizontal(px(6)), align_items: AlignItems::Center, column_gap: px(8), border_radius: BorderRadius::all(px(4)), ..default() }, BackgroundColor(ROW)));
            match pick {
                Pick::Game(i) => {
                    row.insert(GameChoice(i)).observe(|activate: On<Activate>, choices: Query<&GameChoice>, mut b: ResMut<AssetBrowser>| {
                        if let Ok(choice) = choices.get(activate.entity) {
                            b.open_game(choice.0);
                        }
                    });
                }
                Pick::Format(ext, all_viewable) => {
                    row.insert(FormatChoice(ext, all_viewable)).observe(|activate: On<Activate>, choices: Query<&FormatChoice>, mut b: ResMut<AssetBrowser>| {
                        if let Ok(choice) = choices.get(activate.entity) {
                            b.pick_format(&choice.0, choice.1);
                        }
                    });
                }
            }
            row.with_children(|row| {
                if let Some((check, color)) = check {
                    row.spawn((label(check, 14.0, color), Pickable::IGNORE));
                }
                row.spawn((label(name, 14.0, TEXT), Node { flex_grow: 1.0, ..default() }, Pickable::IGNORE));
                for cell in cells {
                    row.spawn((label(cell, 13.0, TEXT), Node { width: px(MENU_COLUMN), ..default() }, Pickable::IGNORE));
                }
            });
        }
    });
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
        (i, name, format!("{}  {}", kind_text(e.kind), size_text(e.size)))
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

fn refresh_texts(
    b: Res<AssetBrowser>,
    mut texts: Query<(&mut Text, Option<&GameButton>, Option<&FilterButton>, Option<&ListTitle>, Option<&InfoText>, Option<&PathText>, Option<&FormatButton>), Or<(With<GameButton>, With<FilterButton>, With<ListTitle>, With<InfoText>, With<PathText>, With<FormatButton>)>>,
) {
    if b.window.is_none() {
        return;
    }
    for (mut text, game, filter, title, info, path, formats) in &mut texts {
        let wanted = if game.is_some() {
            format!("{}   {}", b.game.map_or(tr("(choose)", "(choisir)"), |g| b.games[g].label.as_str()), if b.menu == Menu::Games { "^" } else { "v" })
        } else if formats.is_some() {
            format!("Formats   {}", if b.menu == Menu::Formats { "^" } else { "v" })
        } else if filter.is_some() {
            if b.all_files { tr("All files", "Tous les fichiers").into() } else { tr("Viewable files", "Fichiers montrables").into() }
        } else if title.is_some() {
            let range = if b.shown.is_empty() { "0".to_string() } else { format!("{}-{}", b.scroll + 1, (b.scroll + ROWS).min(b.shown.len())) };
            let filter = if b.filter.is_empty() { tr("type to filter", "tapez pour filtrer").to_string() } else { t!("filter: {}_", "filtre : {}_", b.filter) };
            let format = b.format_filter.as_ref().map_or(String::new(), |f| t!("   format: {f} (Esc = all)", "   format : {f} (Echap = tous)"));
            format!("{range} / {}   ({filter}){format}", b.shown.len())
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
    mut items: Query<(&Hovered, Has<Pressed>, &mut BackgroundColor, Option<&ListRow>, Option<&GameChoice>, Option<&FilterButton>, Has<FormatChoice>), Or<(With<ListRow>, With<GameChoice>, With<FormatChoice>, With<GameButton>, With<FormatButton>, With<FilterButton>, With<CopyButton>, With<PageButton>, With<StepButton>)>>,
) {
    if b.window.is_none() {
        return;
    }
    for (hovered, pressed, mut color, row, game, filter, format) in &mut items {
        let selected = row.is_some_and(|r| Some(r.0) == b.selected) || game.is_some_and(|g| Some(g.0) == b.game) || (filter.is_some() && b.all_files);
        let idle = if row.is_some() || game.is_some() || format { ROW } else { ITEM };
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
    let name = |list: &[usize], i: usize| list.get(i).map_or(tr("none", "aucun").to_string(), |&e| b.entries[e].path.clone());
    let mut steps: Vec<(Ctl, String)> = Vec::new();
    if !c.tiles.is_empty() {
        steps.push((Ctl::Tiles, t!("Tiles: {}", "Tuiles : {}", name(&c.tiles, c.tile))));
    }
    if !c.palettes.is_empty() {
        steps.push((Ctl::Palette, t!("Palette: {}", "Palette : {}", name(&c.palettes, c.palette))));
    }
    if c.banks > 1 {
        steps.push((Ctl::Bank, t!("Colors: {} / {}", "Couleurs : {} / {}", c.bank + 1, c.banks)));
    }
    if c.width > 0 {
        steps.push((Ctl::Width, t!("Width: {} tiles", "Largeur : {} tuiles", c.width)));
    }
    if c.cells > 0 {
        steps.push((Ctl::Cell, t!("Cell: {} / {}", "Cellule : {} / {}", c.cell + 1, c.cells)));
    }
    if c.models > 1 {
        steps.push((Ctl::Model, t!("Model: {} / {}", "Modele : {} / {}", c.model + 1, c.models)));
    }
    if b.selected.is_some_and(|s| b.entries[s].kind == Kind::Model) {
        let tex = if c.texture == 0 { tr("the model's own", "celles du modele").to_string() } else { name(&c.textures, c.texture - 1) };
        steps.push((Ctl::Textures, t!("Textures: {tex}", "Textures : {tex}")));
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
            b.info = t!("{}  :  cannot show it: {e}", "{}  :  impossible de l'afficher : {e}", kind_text(b.entries[sel].kind));
        }
    }
    if let Some(mut camera) = b.camera3d.and_then(|c| cameras.get_mut(c).ok()) {
        camera.is_active = b.is3d;
    }
}

fn preview(b: &mut AssetBrowser, sel: usize, commands: &mut Commands, images: &mut Assets<Image>, meshes: &mut Assets<Mesh>, materials: &mut Assets<StandardMaterial>) -> Result<(), String> {
    let (kind, size) = (b.entries[sel].kind, b.entries[sel].size);
    let mut choices = std::mem::take(&mut b.choices);
    let decoded = decode(&b.entries, sel, &mut choices);
    b.choices = choices;
    match decoded? {
        Decoded::Image { rgba, info } => {
            b.image = Some((images.add(to_image(&rgba, ImageSampler::nearest())), UVec2::new(rgba.width, rgba.height)));
            b.info = format!("{}  ({})  -  {info}", kind_text(kind), size_text(size));
        }
        Decoded::Model { model: m, own_textures } => {
            spawn_model(&m, commands, images, meshes, materials);
            let center = (Vec3::from(m.min) + Vec3::from(m.max)) / 2.0;
            let radius = ((Vec3::from(m.max) - Vec3::from(m.min)).length() / 2.0).max(0.01);
            b.orbit = Orbit { target: center, radius, distance: radius * 2.6, ..Orbit::default() };
            b.is3d = true;
            b.image = Some((b.view3d.clone(), UVec2::new(VIEW_W, VIEW_H)));
            let warn = if m.warnings.is_empty() { String::new() } else { format!("\n! {}", m.warnings.join(" ; ")) };
            b.info = t!("3D model ({})  -  {}: {} triangles, {} vertices, {} materials, {own_textures} textures in the file{warn}", "modele 3D ({})  -  {} : {} triangles, {} sommets, {} materiaux, {own_textures} textures dans le fichier{warn}", size_text(size), m.name, m.triangle_count, m.vertex_count, m.materials.len());
        }
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
