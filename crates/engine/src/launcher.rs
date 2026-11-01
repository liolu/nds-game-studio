//! Le launcher : ce qu'on voit quand aucun jeu ne tourne, à la place des deux écrans DS.
//!
//! - à gauche : le logo, le bouton de la vue des jeux (4 carrés) et les jeux favoris ;
//! - la vue des jeux, en grille ou en liste, avec les dossiers de jeux (les sous-dossiers de `rom`) : clic pour
//!   entrer, Retour ou Echap pour remonter, « Nouveau dossier » ; un jeu se range en le glissant sur un dossier
//!   (ou sur Retour), ou depuis ses Parametres ; barre de recherche (Ctrl+F, dans tous les dossiers) et tri ;
//! - la page d'un jeu : son image, son nom, le bouton favori et quatre boutons qui changent la zone
//!   du bas (Jouer, Moding, Description, Parametres) ;
//! - les textes suivent la langue (`i18n`) ; la zone est refaite quand elle change ;
//! - le bouton Options, toujours en bas à droite, même quand on fait défiler ;
//! - à droite de la vue des jeux, le panneau d'activité : ce que l'app fait, où en est la décompilation et
//!   l'historique (`activity`). Il n'est jamais refait avec la zone, pour garder sa position.
//!
//! Sans rom, aucun jeu n'est montré : seulement le message qui dit de déposer une rom dans la
//! fenêtre ou de la choisir avec « Ajouter des jeux ». Elle est copiée dans le dossier `rom` à côté du launcher
//! (voir `library`).
//!
//! Il n'y a pas d'émulation : « Jouer » lance le module natif du jeu (décompilation + runtime),
//! reconnu par le `game_code` de son game.toml. Sans module, le bouton le dit.
//!
//! Toute la zone est refaite quand `Launcher::dirty` est vrai, comme les pages des options.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::picking::events::PointerDrag;
use bevy::picking::hover::{HoverMap, Hovered};
use bevy::picking::Pickable;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::ui::{Pressed, ScrollPosition};
use bevy::ui_widgets::{Activate, Button};
use bevy::window::FileDragAndDrop;

use crate::activity::{self, What};
use crate::asset_browser::AssetBrowser;
use crate::i18n::{self, t, tr, Lang, Language, Tr};
use crate::game_meta::{self, GameMeta, MetaDb};
use crate::library::{self, Imported, LibraryPrefs, LibraryView, RomInfo, SortCell, SortKey};
use crate::options::ToggleOptions;
use crate::saves_page::format_size;
use crate::screens::{DisplayCamera, DsScreens};
use crate::studio::{ActiveGame, ActiveRom, AutoStart, GameRegistry, StudioFooter, StudioState};
use crate::titlebar::TITLE_H;

const BG: Color = Color::srgb(0.09, 0.1, 0.12);
const SIDEBAR: Color = Color::srgb(0.065, 0.07, 0.085);
const PANEL: Color = Color::srgb(0.12, 0.13, 0.16);
const ITEM: Color = Color::srgb(0.17, 0.18, 0.22);
const ITEM_HOVER: Color = Color::srgb(0.24, 0.26, 0.31);
const ITEM_ON: Color = Color::srgb(0.24, 0.36, 0.62);
const PLAY: Color = Color::srgb(0.2, 0.55, 0.32);
const PLAY_HOVER: Color = Color::srgb(0.25, 0.65, 0.39);
const DANGER: Color = Color::srgb(0.7, 0.2, 0.2);
const DANGER_HOVER: Color = Color::srgb(0.82, 0.26, 0.26);
const BORDER: Color = Color::srgb(0.27, 0.29, 0.35);
const TEXT: Color = Color::srgb(0.92, 0.93, 0.95);
const DIM: Color = Color::srgb(0.6, 0.63, 0.7);
const GOLD: Color = Color::srgb(0.98, 0.78, 0.25);
const GOOD: Color = Color::srgb(0.35, 0.78, 0.48);
const BAD: Color = Color::srgb(0.95, 0.42, 0.38);
const INFO: Color = Color::srgb(0.45, 0.62, 0.95);

const SIDEBAR_W: f32 = 170.0;
/// Panneau d'activité : largeur, fenêtre en dessous de laquelle il s'efface, lignes d'historique, longueur d'une ligne de la préparation.
const ACTIVITY_W: f32 = 340.0;
const ACTIVITY_MIN_WINDOW: f32 = 1100.0;
const ACTIVITY_ROWS: usize = 100;
const ACTIVITY_LINE: usize = 60;
const COVER: f32 = 300.0;
const MAIN_PAD: f32 = 24.0;
/// Hauteur au-dessus de la zone du bas ; le reste de l'image déborde dans la zone.
const HEADER_H: f32 = 126.0;
const STATUS_SECONDS: f64 = 5.0;
const POLL_EVERY: f64 = 2.0;
const CONFIRM_SECONDS: f64 = 3.0;
/// Un jeu ne part qu'après ces pixels : un clic où la souris bouge un peu reste un clic.
const DRAG_FROM: f32 = 8.0;
/// Pixels défilés par cran de molette.
const WHEEL_LINE: f32 = 70.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Tab {
    #[default]
    Play,
    Modding,
    Description,
    Settings,
}

impl Tab {
    const ALL: [Tab; 4] = [Tab::Play, Tab::Modding, Tab::Description, Tab::Settings];

    fn label(self) -> &'static str {
        match self {
            Tab::Play => tr("Play", "Jouer"),
            Tab::Modding => tr("Modding", "Moding"),
            Tab::Description => "Description",
            Tab::Settings => tr("Settings", "Parametres"),
        }
    }
}

struct Rom {
    info: RomInfo,
    icon: Option<Handle<Image>>,
    /// Jaquette et son rapport largeur / hauteur.
    art: Option<(Handle<Image>, f32)>,
    tint: Color,
    /// Ce que dit GameTDB (`game_meta`).
    meta: GameMeta,
}

#[derive(Resource, Default)]
struct Launcher {
    roms: Vec<Rom>,
    cover_job: Option<Arc<Mutex<crate::covers::Progress>>>,
    /// Codes déjà demandés depuis le démarrage : un jeu absent de GameTDB n'est pas redemandé à chaque image.
    cover_asked: std::collections::HashSet<String>,
    /// Options > Launcher : préparer aussi les jeux expérimentaux.
    prepare_all: bool,
    prefs: LibraryPrefs,
    /// Jeu ouvert (nom de fichier de la rom) ; sans, la vue des jeux.
    open: Option<String>,
    /// Tous les dossiers de jeux, et celui qui est montré (vide = la racine).
    folders: Vec<String>,
    folder: String,
    /// Nom du dossier en train d'être tapé.
    naming: Option<String>,
    /// La vue vient de changer de dossier : elle repart du haut.
    scroll_reset: bool,
    tab: Tab,
    dirty: bool,
    /// La zone qui est refaite, et le texte des messages.
    content: Option<Entity>,
    star: Handle<Image>,
    font: Fonts,
    stamp: Vec<(String, u64)>,
    next_poll: f64,
    /// Position de la vue des jeux, retrouvée en revenant d'un jeu.
    grid_scroll: f32,
    status: String,
    status_at: f64,
    /// Copies de rom en cours et leurs résultats.
    copying: usize,
    imported: Arc<Mutex<Vec<Result<Imported, String>>>>,
    /// Premier clic sur « Retirer ce jeu ».
    confirm_remove: Option<f64>,
    /// Le lien d'un mod en train d'être tapé (onglet Moding).
    mod_link: Option<String>,
    /// Une seule copie à la fois : deux roms de même nom se gêneraient (`import_rom`).
    import_lock: Arc<Mutex<()>>,
    tally: ImportTally,
    /// Sélecteur de fichiers ouvert, et ce qu'il a rendu (vide si annulé).
    picking: bool,
    picked: Arc<Mutex<Option<Vec<PathBuf>>>>,
    /// Texte cherché, et si les touches vont dans la barre.
    search: String,
    search_focus: bool,
    /// Le tag en train d'être tapé (Parametres d'un jeu).
    tag_input: Option<String>,
    /// La base de GameTDB, et le fil qui la lit. Téléchargement demandé une fois par lancement.
    meta: MetaDb,
    meta_job: Option<Arc<Mutex<Option<MetaDb>>>>,
    meta_started: bool,
    meta_fetched: bool,
    /// Le jeu (chemin relatif) en train d'être glissé vers un dossier.
    dragging: Option<String>,
    /// Ce qu'on tape là est l'adresse de l'hôte d'une partie en réseau.
    typing_host: bool,
}

/// Bilan d'un ajout de plusieurs roms : chaque message remplace le précédent, une erreur passerait
/// inaperçue.
#[derive(Default)]
struct ImportTally {
    added: usize,
    already: usize,
    failed: usize,
    first_error: Option<String>,
}

impl ImportTally {
    fn record(&mut self, result: &Result<Imported, String>) {
        match result {
            Ok(Imported { already: true, .. }) => self.already += 1,
            Ok(Imported { already: false, .. }) => self.added += 1,
            Err(e) => {
                self.failed += 1;
                self.first_error.get_or_insert_with(|| e.clone());
            }
        }
    }

    /// Rien pour une seule rom : son message suffit.
    fn summary(&self) -> Option<String> {
        let Self { added, already, failed, .. } = self;
        if added + already + failed < 2 {
            return None;
        }
        let mut parts = Vec::new();
        if *added > 0 {
            parts.push(t!("{added} added", "{added} ajoute(s)"));
        }
        if *already > 0 {
            parts.push(t!("{already} already in the launcher", "{already} deja dans le launcher"));
        }
        if let Some(error) = &self.first_error {
            parts.push(t!("{failed} failed: {error}", "{failed} en echec : {error}"));
        }
        Some(parts.join(", "))
    }
}

/// Couleurs d'un bouton : `Look` donne celles au repos et au survol.
#[derive(Component)]
struct Look {
    normal: Color,
    hover: Color,
}

/// Zone qui défile à la molette quand la souris est dessus.
#[derive(Component)]
struct Scrollable;

/// La vue des jeux, dont on garde la position.
#[derive(Component)]
struct GridScroll;

#[derive(Component)]
struct StatusText;

#[derive(Component)]
struct DropOverlay;

#[derive(Component)]
struct ViewChoice(LibraryView);

#[derive(Component)]
struct SearchBox;

/// Déposer un jeu ici le range dans ce dossier (vide = la racine).
#[derive(Component)]
struct DropFolder(String);

/// Ce qui suit la souris pendant qu'on glisse un jeu, et sa ligne qui dit où il ira.
#[derive(Component)]
struct DragGhost;

#[derive(Component)]
struct DragHint;

#[derive(Component)]
struct VersionText;

#[derive(Component)]
struct CoverChoice(bool);

#[derive(Component)]
struct UpdateBanner;

/// Une mise à jour trouvée par l'application : version et notes.
#[derive(Clone)]
pub struct UpdateInfo {
    pub version: String,
    pub notes: String,
}

/// Rempli par l'application (qui connaît son canal) quand une version plus récente existe.
#[derive(Resource, Clone, Default)]
pub struct UpdateOffer(pub Arc<Mutex<Option<UpdateInfo>>>);

const UPDATE_LATER_SECS: u64 = 3 * 24 * 3600;

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[derive(Component)]
struct CoverStatus;

#[derive(Component)]
struct LangChoice(Lang);

#[derive(Component)]
struct ActivityPanel;

/// Les trois parties du panneau refaites quand leur contenu change.
#[derive(Component)]
struct ActivityNow;

#[derive(Component)]
struct ActivityDecomp;

#[derive(Component)]
struct ActivityHistory;

pub struct LauncherPlugin;

impl Plugin for LauncherPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Launcher>()
            .add_systems(Startup, setup)
            .add_systems(OnEnter(StudioState::Menu), spawn_launcher.before(crate::studio::auto_start))
            .add_systems(Update, (take_drops, poll_picker, poll_copies, poll_disk, back_key, drag_game, type_folder_name, type_mod_link, type_search, type_tag, leave_search, expire_confirm, log_clicks, rebuild, show_status).chain().run_if(in_state(StudioState::Menu)))
            .add_systems(Update, (activity_visibility, activity_now, activity_decomp, activity_history, note_update, poll_mods).run_if(in_state(StudioState::Menu)))
            .add_systems(Update, (wheel, (looks, drop_looks).chain(), view_choice_looks, lang_choice_looks, base_mods_looks, version_text, redo_on_language))
            .init_resource::<UpdateOffer>()
            .add_systems(Update, (poll_covers, poll_meta, cover_choice_looks, prepare_games, update_banner.run_if(in_state(StudioState::Menu))));
    }
}

fn setup(mut lau: ResMut<Launcher>, mut images: ResMut<Assets<Image>>, mut fonts: ResMut<Assets<Font>>) {
    lau.prefs = LibraryPrefs::load();
    lau.star = images.add(star_image());
    lau.font = Fonts {
        latin: system_font(&mut fonts, &["segoeui.ttf"], &["Supplemental/Arial.ttf", "Helvetica.ttc"]),
        japanese: system_font(&mut fonts, &["YuGothR.ttc", "meiryo.ttc", "msgothic.ttc"], &[ARIAL_UNICODE, "ヒラギノ角ゴシック W3.ttc"]),
    };
}

/// Polices du système (accents, ™ et japonais des titres de jeux) ; sans elles, celle de Bevy.
#[derive(Clone, Default)]
struct Fonts {
    latin: Option<Handle<Font>>,
    japanese: Option<Handle<Font>>,
}

impl Fonts {
    fn for_text(&self, value: &str) -> Option<&Handle<Font>> {
        // Kana et kanji commencent à U+2E80 ; le ™ et les accents restent dans la police latine
        let japanese = value.chars().any(|c| c >= '\u{2E80}');
        if japanese { self.japanese.as_ref().or(self.latin.as_ref()) } else { self.latin.as_ref() }
    }
}

/// Police de macOS qui a aussi le japonais.
pub(crate) const ARIAL_UNICODE: &str = "Supplemental/Arial Unicode.ttf";

/// La première police trouvée : ces noms-là sous Windows, ceux-là sous macOS.
pub(crate) fn system_font(fonts: &mut Assets<Font>, windows: &[&str], macos: &[&str]) -> Option<Handle<Font>> {
    let (dir, names) = if cfg!(target_os = "macos") {
        (PathBuf::from("/System/Library/Fonts"), macos)
    } else {
        (std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows")).join("Fonts"), windows)
    };
    let bytes = names.iter().find_map(|name| std::fs::read(dir.join(name)).ok())?;
    Some(fonts.add(Font::from_bytes(bytes)))
}

fn rgba_image(width: u32, height: u32, data: Vec<u8>, sampler: ImageSampler) -> Image {
    let mut image = Image::new(Extent3d { width, height, depth_or_array_layers: 1 }, TextureDimension::D2, data, TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::default());
    image.sampler = sampler;
    image
}

/// Étoile blanche à 5 branches (la police n'en a pas), teintée par `ImageNode::color`.
fn star_image() -> Image {
    const SIZE: usize = 64;
    const SAMPLES: usize = 4;
    let points: Vec<Vec2> = (0..10)
        .map(|i| {
            let radius = if i % 2 == 0 { 30.0 } else { 12.5 };
            let angle = std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
            Vec2::new(32.0 + radius * angle.cos(), 33.0 - radius * angle.sin())
        })
        .collect();
    let inside = |p: Vec2| {
        let mut odd = false;
        for (i, a) in points.iter().enumerate() {
            let b = points[(i + 1) % points.len()];
            if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
                odd = !odd;
            }
        }
        odd
    };
    let mut data = vec![255u8; SIZE * SIZE * 4];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let mut covered = 0;
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let p = Vec2::new(x as f32 + (sx as f32 + 0.5) / SAMPLES as f32, y as f32 + (sy as f32 + 0.5) / SAMPLES as f32);
                    covered += usize::from(inside(p));
                }
            }
            data[(y * SIZE + x) * 4 + 3] = (covered * 255 / (SAMPLES * SAMPLES)) as u8;
        }
    }
    rgba_image(SIZE as u32, SIZE as u32, data, ImageSampler::linear())
}

// ─────────────────────────────────────────────────────────────────────────
//  La fenêtre fixe : fond, zone refaite, bouton Options, message, dépôt d'un fichier
// ─────────────────────────────────────────────────────────────────────────

fn spawn_launcher(mut commands: Commands, mut lau: ResMut<Launcher>, mut images: ResMut<Assets<Image>>, screens: Res<DsScreens>, cameras: Query<(Entity, &DisplayCamera)>, auto: Option<Res<AutoStart>>) {
    if auto.is_some() {
        return;
    }
    let Some((camera, _)) = cameras.iter().find(|(_, d)| d.0 == screens.main_window()) else { return };
    let font = lau.font.clone();
    reload_roms(&mut lau, &mut images);
    let progress: Vec<(String, f32)> = lau.roms.iter().filter_map(|r| Some((r.info.code.clone(), crate::native_game::game_state(&r.info.code)?.code?))).collect();
    activity::compare_progress(&progress);
    let mut content = None;
    commands
        .spawn((
            Node { position_type: PositionType::Absolute, top: px(TITLE_H), left: px(0), right: px(0), bottom: px(0), flex_direction: FlexDirection::Row, ..default() },
            BackgroundColor(BG),
            UiTargetCamera(camera),
            DespawnOnExit(StudioState::Menu),
        ))
        .with_children(|root| {
            content = Some(root.spawn(Node { flex_grow: 1.0, flex_basis: px(0), min_width: px(0), height: percent(100), flex_direction: FlexDirection::Row, ..default() }).id());
            spawn_activity_panel(root, &font);
            // Fixe : ne bouge pas quand on fait défiler
            root.spawn((
                Button,
                Hovered::default(),
                Node { position_type: PositionType::Absolute, right: px(24), bottom: px(20), padding: UiRect::axes(px(22), px(11)), border_radius: BorderRadius::all(px(8)), ..default() },
                BackgroundColor(ITEM_ON),
                Look { normal: ITEM_ON, hover: Color::srgb(0.3, 0.44, 0.74) },
                GlobalZIndex(5),
            ))
            .observe(|_: On<Activate>, mut options: MessageWriter<ToggleOptions>| {
                options.write(ToggleOptions);
            })
            .with_child((text(&font, tr("Settings", "Options"), 16.0, TEXT), Tr("Settings", "Options")));
            root.spawn((text(&font, "", 14.0, TEXT), Node { position_type: PositionType::Absolute, left: px(SIDEBAR_W + MAIN_PAD), bottom: px(28), ..default() }, GlobalZIndex(5), StatusText));
            root.spawn((
                Node { position_type: PositionType::Absolute, left: px(0), right: px(0), top: px(0), bottom: px(0), justify_content: JustifyContent::Center, align_items: AlignItems::Center, display: Display::None, ..default() },
                BackgroundColor(Color::srgba(0.1, 0.2, 0.42, 0.88)),
                GlobalZIndex(10),
                DropOverlay,
            ))
            .with_child((text(&font, tr("Drop the rom to add it", "Deposez la rom pour l'ajouter"), 30.0, TEXT), Tr("Drop the rom to add it", "Deposez la rom pour l'ajouter")));
        });
    lau.content = content;
    lau.dirty = true;
    info!("launcher ouvert : {} {}, {} roms dans {}", std::env::consts::OS, std::env::consts::ARCH, lau.roms.len(), library::rom_dir().display());
}

fn reload_roms(lau: &mut Launcher, images: &mut Assets<Image>) {
    let dir = library::rom_dir();
    lau.stamp = library::folder_stamp(&dir);
    lau.folders = library::folders_in(&lau.stamp);
    info!("roms relues : {:?}", lau.stamp);
    lau.roms = library::scan_roms(&dir)
        .into_iter()
        .map(|info| {
            let icon = (!info.icon.is_empty()).then(|| images.add(rgba_image(32, 32, info.icon.clone(), ImageSampler::nearest())));
            let [r, g, b] = info.average_color();
            let art = if lau.prefs.covers { load_art(&info.code, images) } else { None };
            let meta = lau.meta.lookup(&info.code);
            Rom { info, icon, art, tint: Color::srgb(r * 0.45, g * 0.45, b * 0.45), meta }
        })
        .collect();
    lau.dirty = true;
}

/// Prépare les jeux en tâche de fond, et rafraîchit la page du jeu ouvert quand son état change.
fn prepare_games(mut lau: ResMut<Launcher>, state: Res<State<StudioState>>, time: Res<Time>, mut next: Local<f32>, mut shown: Local<String>) {
    if time.elapsed_secs() < *next {
        return;
    }
    *next = time.elapsed_secs() + 1.0;
    // Pas de compilation pendant qu'on joue
    if *state.get() == StudioState::Menu {
        crate::native_game::prepare_next(lau.roms.iter().map(|r| (r.info.code.as_str(), r.info.path.as_path())), lau.prepare_all);
    }
    let Some(open) = lau.open.clone() else { return };
    let Some(rom) = lau.roms.iter().find(|r| r.info.file == open) else { return };
    let now = match crate::native_game::preparation(&rom.info.code) {
        crate::native_game::Preparation::Ready => "pret".to_string(),
        crate::native_game::Preparation::Running(m) => m,
        crate::native_game::Preparation::Failed(e) => e,
        crate::native_game::Preparation::NotStarted => String::new(),
    };
    if *shown != now {
        *shown = now;
        lau.dirty = true;
    }
}

/// La mise à jour proposée : « Plus tard » l'écarte trois jours (idée reprise du launcher d'OpenRAC).
fn update_banner(
    mut commands: Commands,
    offer: Res<UpdateOffer>,
    mut lau: ResMut<Launcher>,
    screens: Res<DsScreens>,
    cameras: Query<(Entity, &DisplayCamera)>,
    existing: Query<(), With<UpdateBanner>>,
) {
    let _ = &mut lau;
    if !existing.is_empty() {
        return;
    }
    let Some(info) = offer.0.lock().unwrap().clone() else { return };
    if lau.prefs.update_later == info.version && now_secs() < lau.prefs.update_later_until {
        return;
    }
    let Some((camera, _)) = cameras.iter().find(|(_, d)| d.0 == screens.main_window()) else { return };
    let font = lau.font.clone();
    let version = info.version.clone();
    commands
        .spawn((
            UpdateBanner,
            Node { position_type: PositionType::Absolute, left: px(190), bottom: px(16), max_width: px(620), padding: UiRect::all(px(12)), column_gap: px(10), align_items: AlignItems::Center, border_radius: BorderRadius::all(px(8)), ..default() },
            BackgroundColor(ITEM_ON),
            GlobalZIndex(5),
            UiTargetCamera(camera),
            DespawnOnExit(StudioState::Menu),
        ))
        .with_children(|b| {
            b.spawn((text(&font, t!("New version: {}. {}", "Nouvelle version : {}. {}", info.version, info.notes), 14.0, TEXT), Node { flex_shrink: 1.0, ..default() }));
            button(b, Node { flex_shrink: 0.0, ..button_node() }, PLAY, PLAY_HOVER).observe(start_update).with_child(text(&font, tr("Update", "Mettre a jour"), 14.0, TEXT));
            button(b, Node { flex_shrink: 0.0, ..button_node() }, ITEM, ITEM_HOVER)
                .observe(move |_: On<Activate>, mut commands: Commands, mut lau: ResMut<Launcher>, banners: Query<Entity, With<UpdateBanner>>| {
                    lau.prefs.update_later = version.clone();
                    lau.prefs.update_later_until = now_secs() + UPDATE_LATER_SECS;
                    lau.prefs.store();
                    for banner in &banners {
                        commands.entity(banner).despawn();
                    }
                })
                .with_child(text(&font, tr("Later", "Plus tard"), 14.0, TEXT));
        });
}

/// Le programme de mise à jour remplace le studio : il installe la version, puis relance le studio.
fn start_update(_: On<Activate>, mut exit: MessageWriter<AppExit>, mut toasts: MessageWriter<crate::link::ShowToast>) {
    let exe = std::env::current_exe().unwrap_or_default();
    let updater = exe.with_file_name(format!("nds-game-studio-launcher{}", std::env::consts::EXE_SUFFIX));
    match std::process::Command::new(&updater).spawn() {
        Ok(_) => {
            exit.write(AppExit::Success);
        }
        Err(e) => {
            toasts.write(crate::link::ShowToast(format!("{} : {e}", updater.display())));
        }
    }
}

fn load_art(code: &str, images: &mut Assets<Image>) -> Option<(Handle<Image>, f32)> {
    let (width, height, pixels) = crate::covers::load(code)?;
    Some((images.add(rgba_image(width, height, pixels, ImageSampler::linear())), width as f32 / height.max(1) as f32))
}

/// Demande les jaquettes qui manquent et montre celles qui arrivent.
fn poll_covers(mut lau: ResMut<Launcher>, mut images: ResMut<Assets<Image>>, mut status: Query<&mut Text, With<CoverStatus>>) {
    if lau.prefs.covers && lau.cover_job.is_none() {
        let asked = &lau.cover_asked;
        let mut missing: Vec<String> = lau.roms.iter().map(|r| r.info.code.clone()).filter(|c| !asked.contains(c) && !crate::covers::path(c).exists()).collect();
        missing.sort();
        missing.dedup();
        if !missing.is_empty() {
            lau.cover_asked.extend(missing.iter().cloned());
            lau.cover_job = Some(crate::covers::start(missing));
        }
    }
    let Some(job) = lau.cover_job.clone() else {
        for mut text in &mut status {
            let wanted = if lau.prefs.covers {
                let shown = lau.roms.iter().filter(|r| r.art.is_some()).count();
                t!("{shown} of {} games have a cover.", "{shown} jeux sur {} ont une jaquette.", lau.roms.len())
            } else {
                String::new()
            };
            if text.0 != wanted {
                text.0 = wanted;
            }
        }
        return;
    };
    let (arrived, done, total, found) = {
        let mut p = job.lock().unwrap();
        // Refaire la zone du launcher coûte cher : par paquets, et à la fin
        let ready = p.arrived.len() >= 8 || p.done == p.total;
        (if ready { std::mem::take(&mut p.arrived) } else { Vec::new() }, p.done, p.total, p.found)
    };
    for code in &arrived {
        for rom in lau.roms.iter_mut().filter(|r| r.info.code == *code) {
            rom.art = load_art(code, &mut images);
        }
    }
    if !arrived.is_empty() {
        lau.dirty = true;
    }
    for mut text in &mut status {
        text.0 = t!("Downloading covers: {done} of {total}, {found} found.", "Telechargement des jaquettes : {done} sur {total}, {found} trouvees.");
    }
    if done == total {
        info!("jaquettes : {found} trouvees sur {total}");
        if found > 0 {
            activity::record(What::Covers { found, total });
        }
        lau.cover_job = None;
    }
}

/// Lit la base des infos, et la télécharge si l'option des jaquettes le permet.
fn poll_meta(mut lau: ResMut<Launcher>) {
    let fetch = lau.prefs.covers && !lau.meta_fetched;
    if lau.meta_job.is_none() && (!lau.meta_started || fetch) {
        lau.meta_started = true;
        lau.meta_fetched |= fetch;
        lau.meta_job = Some(game_meta::start(fetch));
    }
    let Some(job) = lau.meta_job.clone() else { return };
    let arrived = job.lock().unwrap().take();
    if let Some(db) = arrived {
        let lau = &mut *lau;
        lau.meta = db;
        for rom in &mut lau.roms {
            rom.meta = lau.meta.lookup(&rom.info.code);
        }
        lau.dirty = true;
    }
    // Le fil garde sa copie du pointeur tant qu'il travaille
    if Arc::strong_count(&job) <= 2 {
        lau.meta_job = None;
    }
}

fn set_covers(on: bool) -> impl FnMut(On<Activate>, ResMut<Launcher>, ResMut<Assets<Image>>) {
    move |_, mut lau, mut images| {
        lau.prefs.covers = on;
        lau.prefs.store();
        let lau = &mut *lau;
        for rom in &mut lau.roms {
            rom.art = if on { load_art(&rom.info.code, &mut images) } else { None };
        }
        lau.dirty = true;
    }
}

fn cover_choice_looks(lau: Res<Launcher>, mut buttons: Query<(&CoverChoice, &Hovered, &mut BackgroundColor)>) {
    for (choice, hovered, mut color) in &mut buttons {
        let wanted = if lau.prefs.covers == choice.0 { ITEM_ON } else if hovered.get() { ITEM_HOVER } else { ITEM };
        if color.0 != wanted {
            color.0 = wanted;
        }
    }
}

fn say(lau: &mut Launcher, time: &Time<Real>, message: impl Into<String>) {
    lau.status = message.into();
    info!("message : {}", lau.status);
    lau.status_at = time.elapsed_secs_f64();
}

// ─────────────────────────────────────────────────────────────────────────
//  Ajouter une rom (glisser-déposer ou sélecteur de fichiers), suivre le dossier
// ─────────────────────────────────────────────────────────────────────────

fn take_drops(mut drops: MessageReader<FileDragAndDrop>, mut lau: ResMut<Launcher>, time: Res<Time<Real>>, mut overlays: Query<&mut Node, With<DropOverlay>>) {
    for drop in drops.read() {
        let show = match drop {
            FileDragAndDrop::HoveredFile { .. } => true,
            FileDragAndDrop::HoveredFileCanceled { .. } => false,
            FileDragAndDrop::DroppedFile { path_buf, .. } => {
                // Sur la page Moding d'un jeu, ce qu'on dépose est un mod
                let modding = lau.tab == Tab::Modding && lau.open.is_some();
                let game = lau.open.as_ref().and_then(|file| lau.roms.iter().find(|r| r.info.file == *file)).map(|r| (r.info.code.clone(), r.info.path.clone()));
                match game.filter(|_| modding) {
                    Some((code, rom)) => crate::mod_packs::add_path(&code, &rom, path_buf),
                    None => {
                        start_copy(&mut lau, path_buf.clone());
                        say(&mut lau, &time, tr("Copying the rom...", "Copie de la rom..."));
                    }
                }
                false
            }
        };
        for mut node in &mut overlays {
            node.display = if show { Display::Flex } else { Display::None };
        }
    }
}

/// Le bouton « Ajouter des jeux ». Le dialogue du système (Windows, macOS, Linux) bloque jusqu'à sa fermeture, d'où
/// le fil ; sur macOS, rfd l'ouvre de lui-même sur le fil principal.
fn pick_roms(_: On<Activate>, mut lau: ResMut<Launcher>) {
    if lau.picking {
        return;
    }
    lau.picking = true;
    let picked = lau.picked.clone();
    std::thread::spawn(move || {
        let files = rfd::FileDialog::new().set_title(tr("Add games", "Ajouter des jeux")).add_filter(tr("DS roms", "Roms DS"), &["nds", "dsi"]).pick_files().unwrap_or_default();
        *picked.lock().unwrap() = Some(files);
    });
}

fn poll_picker(mut lau: ResMut<Launcher>, time: Res<Time<Real>>) {
    let picked = lau.picked.lock().unwrap().take();
    let Some(files) = picked else { return };
    lau.picking = false;
    let count = files.len();
    for file in files {
        start_copy(&mut lau, file);
    }
    match count {
        0 => {}
        1 => say(&mut lau, &time, tr("Copying the rom...", "Copie de la rom...")),
        n => say(&mut lau, &time, t!("Copying {n} roms...", "Copie de {n} roms...")),
    }
}

fn add_games_button(c: &mut ChildSpawnerCommands, font: &Fonts, normal: Color, hover: Color, size: f32) {
    button(c, button_node(), normal, hover).observe(pick_roms).with_child(text(font, tr("Add games", "Ajouter des jeux"), size, TEXT));
}

fn start_copy(lau: &mut Launcher, source: PathBuf) {
    lau.copying += 1;
    let results = lau.imported.clone();
    let lock = lau.import_lock.clone();
    let target = library::folder_path(&library::rom_dir(), &lau.folder);
    std::thread::spawn(move || {
        let _one_at_a_time = lock.lock();
        let result = library::import_rom(&source, &target);
        results.lock().unwrap().push(result);
    });
}

fn poll_copies(mut lau: ResMut<Launcher>, mut images: ResMut<Assets<Image>>, time: Res<Time<Real>>) {
    let results = std::mem::take(&mut *lau.imported.lock().unwrap());
    if results.is_empty() {
        return;
    }
    lau.copying -= results.len();
    for result in results {
        lau.tally.record(&result);
        let message = match result {
            Ok(Imported { title, already: true }) => t!("{title} is already in the launcher", "{title} est deja dans le launcher"),
            Ok(Imported { title, already: false }) => {
                activity::record(What::RomAdded { title: title.clone() });
                t!("{title} added", "{title} ajoute")
            }
            Err(e) => {
                activity::record(What::RomFailed { error: e.clone() });
                e
            }
        };
        say(&mut lau, &time, message);
    }
    if lau.copying == 0 {
        if let Some(summary) = std::mem::take(&mut lau.tally).summary() {
            say(&mut lau, &time, summary);
        }
    }
    reload_roms(&mut lau, &mut images);
}

/// Une rom copiée ou retirée à la main dans le dossier apparaît ou disparaît toute seule.
fn poll_disk(mut lau: ResMut<Launcher>, mut images: ResMut<Assets<Image>>, time: Res<Time<Real>>) {
    let now = time.elapsed_secs_f64();
    if now < lau.next_poll {
        return;
    }
    lau.next_poll = now + POLL_EVERY;
    if lau.copying == 0 && library::folder_stamp(&library::rom_dir()) != lau.stamp {
        reload_roms(&mut lau, &mut images);
    }
}

fn back_key(keys: Res<ButtonInput<KeyCode>>, mut lau: ResMut<Launcher>, screens: Res<DsScreens>, windows: Query<&Window>) {
    let focused = windows.get(screens.main_window()).is_ok_and(|w| w.focused);
    if !focused || !keys.just_pressed(KeyCode::Escape) || lau.naming.is_some() || lau.mod_link.is_some() || lau.tag_input.is_some() || lau.search_focus || lau.dragging.is_some() {
        return;
    }
    if lau.open.is_some() {
        lau.open = None;
        lau.dirty = true;
    } else if !lau.search.is_empty() {
        lau.search.clear();
        lau.scroll_reset = true;
        lau.dirty = true;
    } else if !lau.folder.is_empty() {
        let parent = library::parent_of(&lau.folder).to_string();
        enter_folder(&mut lau, parent);
    }
}

fn enter_folder(lau: &mut Launcher, folder: String) {
    lau.folder = folder;
    lau.naming = None;
    lau.search.clear();
    lau.search_focus = false;
    lau.scroll_reset = true;
    lau.dirty = true;
}

/// Le nom du nouveau dossier : Entrée le crée, Échap annule.
fn type_folder_name(mut typed: MessageReader<KeyboardInput>, mut lau: ResMut<Launcher>, mut images: ResMut<Assets<Image>>, time: Res<Time<Real>>, screens: Res<DsScreens>) {
    let main = screens.main_window();
    for key in typed.read() {
        if lau.naming.is_none() || key.window != main || !key.state.is_pressed() {
            continue;
        }
        match &key.logical_key {
            Key::Enter => create_named_folder(&mut lau, &mut images, &time),
            Key::Escape => lau.naming = None,
            Key::Backspace => {
                if let Some(name) = &mut lau.naming {
                    name.pop();
                }
            }
            _ => {
                if let (Some(text), Some(name)) = (&key.text, &mut lau.naming) {
                    name.extend(text.chars().filter(|c| !c.is_control()));
                }
            }
        }
        lau.dirty = true;
    }
}

/// Le lien d'un mod : Entrée l'installe, Échap annule, Ctrl+V colle.
fn type_mod_link(mut typed: MessageReader<KeyboardInput>, mut lau: ResMut<Launcher>, keys: Res<ButtonInput<KeyCode>>, mut clipboard: ResMut<bevy::clipboard::Clipboard>, screens: Res<DsScreens>) {
    let main = screens.main_window();
    let command = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight, KeyCode::SuperLeft, KeyCode::SuperRight]);
    for key in typed.read() {
        if lau.mod_link.is_none() || key.window != main || !key.state.is_pressed() {
            continue;
        }
        match &key.logical_key {
            Key::Enter => send_mod_link(&mut lau),
            Key::Escape => {
                lau.mod_link = None;
                lau.typing_host = false;
            }
            Key::Backspace => {
                if let Some(link) = &mut lau.mod_link {
                    link.pop();
                }
            }
            Key::Character(letter) if command && letter.eq_ignore_ascii_case("v") => {
                if let (Some(Ok(text)), Some(link)) = (clipboard.fetch_text().poll_result(), &mut lau.mod_link) {
                    link.push_str(text.trim());
                }
            }
            _ if command => {}
            _ => {
                if let (Some(text), Some(link)) = (&key.text, &mut lau.mod_link) {
                    link.extend(text.chars().filter(|c| !c.is_control()));
                }
            }
        }
        lau.dirty = true;
    }
}

/// Une touche dans un champ de texte, hors Entrée et Échap : effacer, coller (Ctrl+V ou Cmd+V), écrire.
fn edit_field(key: &KeyboardInput, field: &mut String, command: bool, clipboard: &mut bevy::clipboard::Clipboard) {
    match &key.logical_key {
        Key::Backspace => {
            field.pop();
        }
        Key::Character(letter) if command && letter.eq_ignore_ascii_case("v") => {
            if let Some(Ok(text)) = clipboard.fetch_text().poll_result() {
                field.extend(text.chars().filter(|c| !c.is_control()));
            }
        }
        _ if command => {}
        _ => {
            if let Some(text) = &key.text {
                field.extend(text.chars().filter(|c| !c.is_control()));
            }
        }
    }
}

/// Ctrl+F (Cmd+F) ouvre la barre de recherche ; les touches y vont jusqu'à Entrée, Échap (qui vide) ou un clic ailleurs.
fn type_search(mut typed: MessageReader<KeyboardInput>, mut lau: ResMut<Launcher>, keys: Res<ButtonInput<KeyCode>>, mut clipboard: ResMut<bevy::clipboard::Clipboard>, screens: Res<DsScreens>) {
    let main = screens.main_window();
    let command = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight, KeyCode::SuperLeft, KeyCode::SuperRight]);
    for key in typed.read() {
        if key.window != main || !key.state.is_pressed() {
            continue;
        }
        if !lau.search_focus {
            let free = lau.open.is_none() && lau.naming.is_none() && lau.mod_link.is_none() && lau.tag_input.is_none();
            if free && command && matches!(&key.logical_key, Key::Character(letter) if letter.eq_ignore_ascii_case("f")) {
                lau.search_focus = true;
                lau.dirty = true;
            }
            continue;
        }
        match &key.logical_key {
            Key::Enter => lau.search_focus = false,
            Key::Escape => {
                lau.search.clear();
                lau.search_focus = false;
            }
            _ => edit_field(key, &mut lau.search, command, &mut clipboard),
        }
        lau.scroll_reset = true;
        lau.dirty = true;
    }
}

/// Après le relâchement, pour ne pas refaire la zone sous un clic en cours : le clic serait perdu.
fn leave_search(mut lau: ResMut<Launcher>, buttons: Res<ButtonInput<MouseButton>>, boxes: Query<&Hovered, With<SearchBox>>) {
    if lau.search_focus && buttons.just_released(MouseButton::Left) && !boxes.iter().any(|hovered| hovered.get()) {
        lau.search_focus = false;
        lau.dirty = true;
    }
}

/// Le nouveau tag du jeu ouvert : Entrée l'ajoute, Échap annule.
fn type_tag(mut typed: MessageReader<KeyboardInput>, mut lau: ResMut<Launcher>, keys: Res<ButtonInput<KeyCode>>, mut clipboard: ResMut<bevy::clipboard::Clipboard>, time: Res<Time<Real>>, screens: Res<DsScreens>) {
    let main = screens.main_window();
    let command = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight, KeyCode::SuperLeft, KeyCode::SuperRight]);
    for key in typed.read() {
        if lau.tag_input.is_none() || key.window != main || !key.state.is_pressed() {
            continue;
        }
        match &key.logical_key {
            Key::Enter => add_typed_tag(&mut lau, &time),
            Key::Escape => lau.tag_input = None,
            _ => {
                if let Some(mut tag) = lau.tag_input.take() {
                    edit_field(key, &mut tag, command, &mut clipboard);
                    lau.tag_input = Some(tag);
                }
            }
        }
        lau.dirty = true;
    }
}

fn add_typed_tag(lau: &mut Launcher, time: &Time<Real>) {
    let tag = lau.tag_input.clone().unwrap_or_default();
    let Some(code) = lau.open.as_ref().and_then(|file| lau.roms.iter().find(|r| r.info.file == *file)).map(|r| r.info.code.clone()) else { return };
    match lau.prefs.add_tag(&code, &tag) {
        Ok(()) => {
            lau.tag_input = None;
            lau.prefs.store();
        }
        // Le nom reste à corriger
        Err(e) => say(lau, time, e),
    }
    lau.dirty = true;
}

/// Sur une carte de jeu : au-delà de quelques pixels, le jeu part avec la souris.
fn start_drag(file: String) -> impl FnMut(On<PointerDrag>, Commands, ResMut<Launcher>, Res<DsScreens>, Query<(Entity, &DisplayCamera)>) {
    move |drag, mut commands, mut lau, screens, cameras| {
        if lau.dragging.is_some() || drag.button != bevy::picking::pointer::PointerButton::Primary || drag.distance.length() < DRAG_FROM {
            return;
        }
        let Some((camera, _)) = cameras.iter().find(|(_, d)| d.0 == screens.main_window()) else { return };
        let Some(rom) = lau.roms.iter().find(|r| r.info.file == file) else { return };
        let font = lau.font.clone();
        commands
            .spawn((
                Node { position_type: PositionType::Absolute, flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(10), padding: UiRect::all(px(8)), max_width: px(360), border: UiRect::all(px(2)), border_radius: BorderRadius::all(px(8)), ..default() },
                BackgroundColor(PANEL),
                BorderColor::all(ITEM_ON),
                GlobalZIndex(20),
                // Sinon il cacherait les dossiers à la souris
                Pickable::IGNORE,
                UiTargetCamera(camera),
                DespawnOnExit(StudioState::Menu),
                DragGhost,
            ))
            .with_children(|ghost| {
                cover(ghost, rom, 44.0);
                ghost.spawn((Node { flex_direction: FlexDirection::Column, row_gap: px(2), ..default() }, Pickable::IGNORE)).with_children(|lines| {
                    lines.spawn((text(&font, rom.info.title.clone(), 14.0, TEXT), Pickable::IGNORE));
                    lines.spawn((text(&font, "", 12.0, DIM), Pickable::IGNORE, DragHint));
                });
            });
        lau.dragging = Some(file.clone());
    }
}

/// Le jeu glissé : suit la souris, dit où il ira, se range au relâchement. Lu ici et pas sur les cartes : la zone peut
/// être refaite pendant qu'on glisse (une jaquette arrive), ce qui effacerait la carte de départ. Échap annule.
#[allow(clippy::too_many_arguments)]
fn drag_game(
    mut commands: Commands,
    mut lau: ResMut<Launcher>,
    mut images: ResMut<Assets<Image>>,
    time: Res<Time<Real>>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    screens: Res<DsScreens>,
    windows: Query<&Window>,
    targets: Query<(&DropFolder, &Hovered)>,
    mut ghosts: Query<(Entity, &mut Node), With<DragGhost>>,
    mut hints: Query<&mut Text, With<DragHint>>,
) {
    let Some(file) = lau.dragging.clone() else { return };
    let target = targets.iter().find(|(_, hovered)| hovered.get()).map(|(folder, _)| folder.0.clone());
    let released = !buttons.pressed(MouseButton::Left);
    if released || keys.just_pressed(KeyCode::Escape) {
        lau.dragging = None;
        for (ghost, _) in &ghosts {
            commands.entity(ghost).despawn();
        }
        if let Some(folder) = target.filter(|_| released) {
            drop_game(&mut lau, &mut images, &time, &file, &folder);
        }
        return;
    }
    if let Some(cursor) = windows.get(screens.main_window()).ok().and_then(Window::cursor_position) {
        for (_, mut node) in &mut ghosts {
            node.left = px(cursor.x + 14.0);
            node.top = px(cursor.y + 14.0);
        }
    }
    let wanted = match &target {
        Some(folder) if folder.is_empty() => t!("Move to the main folder", "Ranger a la racine"),
        Some(folder) => t!("Move to {folder}", "Ranger dans {folder}"),
        None => tr("Drop it on a folder, Esc = cancel", "Deposez-le sur un dossier, Echap = annuler").to_string(),
    };
    for mut hint in &mut hints {
        if hint.0 != wanted {
            hint.0.clone_from(&wanted);
        }
    }
}

/// Le dossier sous le jeu glissé s'allume.
fn drop_looks(lau: Res<Launcher>, mut targets: Query<(&Hovered, &mut BackgroundColor), With<DropFolder>>) {
    if lau.dragging.is_none() {
        return;
    }
    for (hovered, mut color) in &mut targets {
        if hovered.get() && color.0 != ITEM_ON {
            color.0 = ITEM_ON;
        }
    }
}

fn send_mod_link(lau: &mut Launcher) {
    let game = lau.open.as_ref().and_then(|file| lau.roms.iter().find(|r| r.info.file == *file)).map(|r| (r.info.code.clone(), r.info.path.clone()));
    if std::mem::take(&mut lau.typing_host) {
        if let (Some((code, _)), Some(address)) = (game, lau.mod_link.take()) {
            crate::mod_packs::set_host_address(&code, &address);
        }
        lau.dirty = true;
        return;
    }
    let Some(link) = lau.mod_link.take().filter(|l| !l.trim().is_empty()) else { return };
    if let Some((code, _)) = game {
        crate::mod_packs::find(&code, &link);
    }
    lau.dirty = true;
}

fn create_named_folder(lau: &mut Launcher, images: &mut Assets<Image>, time: &Time<Real>) {
    let name = lau.naming.clone().unwrap_or_default();
    match library::create_folder(&library::rom_dir(), &lau.folder, &name) {
        Ok(_) => {
            lau.naming = None;
            reload_roms(lau, images);
        }
        // Le nom reste à corriger
        Err(e) => say(lau, time, e),
    }
    lau.dirty = true;
}

/// Chaque clic dans le journal : où il tombe et ce qu'il y a dessous. Sert à comprendre un clic sans
/// effet sur une machine qu'on n'a pas (Mac).
fn log_clicks(buttons: Res<ButtonInput<MouseButton>>, screens: Res<DsScreens>, windows: Query<&Window>, hovers: Res<HoverMap>, clickable: Query<Has<Button>>) {
    let pressed = buttons.just_pressed(MouseButton::Left);
    if !pressed && !buttons.just_released(MouseButton::Left) {
        return;
    }
    let Ok(window) = windows.get(screens.main_window()) else { return };
    let under: Vec<(Entity, bool)> = hovers.values().flat_map(|hits| hits.keys()).map(|e| (*e, clickable.get(*e).unwrap_or(false))).collect();
    info!(
        "clic {} : curseur {:?}, fenetre {}x{} echelle {}, sous le curseur (entite, bouton) {under:?}",
        if pressed { "enfonce" } else { "relache" },
        window.cursor_position(),
        window.width(),
        window.height(),
        window.scale_factor(),
    );
}

fn expire_confirm(mut lau: ResMut<Launcher>, time: Res<Time<Real>>) {
    if lau.confirm_remove.is_some_and(|t| time.elapsed_secs_f64() - t > CONFIRM_SECONDS) {
        lau.confirm_remove = None;
        lau.dirty = true;
    }
}

fn show_status(lau: Res<Launcher>, time: Res<Time<Real>>, mut texts: Query<&mut Text, With<StatusText>>) {
    let wanted = if lau.copying > 0 || time.elapsed_secs_f64() - lau.status_at < STATUS_SECONDS { lau.status.as_str() } else { "" };
    for mut text in &mut texts {
        if text.0 != wanted {
            text.0 = wanted.to_string();
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
//  Boutons et textes
// ─────────────────────────────────────────────────────────────────────────

fn text(font: &Fonts, value: impl Into<String>, size: f32, color: Color) -> (Text, TextFont, TextColor) {
    let value = value.into();
    let text_font = match font.for_text(&value) {
        Some(f) => TextFont { font: f.clone().into(), font_size: FontSize::Px(size), ..default() },
        None => TextFont::from_font_size(size),
    };
    (Text::new(value), text_font, TextColor(color))
}

fn button_node() -> Node {
    Node { padding: UiRect::axes(px(16), px(9)), border_radius: BorderRadius::all(px(6)), justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() }
}

fn button<'a>(c: &'a mut ChildSpawnerCommands, node: Node, normal: Color, hover: Color) -> EntityCommands<'a> {
    c.spawn((Button, Hovered::default(), node, BackgroundColor(normal), Look { normal, hover }))
}

/// L'icône de la rom sur le fond de sa couleur (les icônes DS font 32x32 : agrandies sans lissage).
fn cover(c: &mut ChildSpawnerCommands, rom: &Rom, size: f32) {
    c.spawn((
        Node { width: px(size), height: px(size), flex_shrink: 0.0, justify_content: JustifyContent::Center, align_items: AlignItems::Center, border_radius: BorderRadius::all(px(size.min(80.0) / 8.0)), ..default() },
        BackgroundColor(rom.tint),
    ))
    .with_children(|b| {
        if let Some((art, aspect)) = &rom.art {
            let (width, height) = if *aspect > 1.0 { (size, size / aspect) } else { (size * aspect, size) };
            b.spawn((ImageNode::new(art.clone()), Node { width: px(width), height: px(height), ..default() }));
        } else if let Some(icon) = &rom.icon {
            let side = (size * 0.8 / 32.0).floor().max(1.0) * 32.0;
            b.spawn((ImageNode::new(icon.clone()), Node { width: px(side), height: px(side), ..default() }));
        }
    });
}

fn star(c: &mut ChildSpawnerCommands, lau: &Launcher, file: &str, size: f32) {
    let color = if lau.prefs.is_favorite(file) { GOLD } else { Color::srgb(0.38, 0.4, 0.46) };
    c.spawn((ImageNode::new(lau.star.clone()).with_color(color), Node { width: px(size), height: px(size), ..default() }));
}

fn toggle_favorite(lau: &mut Launcher, file: &str) {
    lau.prefs.toggle_favorite(file);
    lau.prefs.store();
    lau.dirty = true;
}

// ─────────────────────────────────────────────────────────────────────────
//  La zone refaite
// ─────────────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn rebuild(mut commands: Commands, mut lau: ResMut<Launcher>, registry: Res<GameRegistry>, footer: Option<Res<StudioFooter>>, children: Query<&Children>, grids: Query<&ScrollPosition, With<GridScroll>>) {
    if !lau.dirty {
        return;
    }
    lau.dirty = false;
    info!("zone du launcher refaite");
    let Some(content) = lau.content else { return };
    if std::mem::take(&mut lau.scroll_reset) {
        lau.grid_scroll = 0.0;
    } else if let Ok(position) = grids.single() {
        lau.grid_scroll = position.0.y;
    }
    // Dossier retiré à la main : retour à la racine
    if !lau.folder.is_empty() && !lau.folders.contains(&lau.folder) {
        lau.folder.clear();
    }
    if let Ok(old) = children.get(content) {
        for child in old.iter() {
            commands.entity(child).despawn();
        }
    }
    if lau.open.as_ref().is_some_and(|file| !lau.roms.iter().any(|r| r.info.file == *file)) {
        lau.open = None;
    }
    let lau = &*lau;
    let footer = footer.map(|f| f.0.clone()).unwrap_or_default();
    commands.entity(content).with_children(|c| {
        spawn_sidebar(c, lau, &footer);
        c.spawn(Node { flex_grow: 1.0, flex_basis: px(0), min_width: px(0), height: percent(100), flex_direction: FlexDirection::Column, position_type: PositionType::Relative, ..default() }).with_children(|main| {
            match lau.open.as_ref().and_then(|file| lau.roms.iter().find(|r| r.info.file == *file)) {
                Some(rom) => spawn_game(main, lau, rom, &registry),
                None if lau.roms.is_empty() && lau.folders.is_empty() => spawn_empty(main, lau),
                None => spawn_library(main, lau),
            }
        });
    });
}

fn spawn_sidebar(c: &mut ChildSpawnerCommands, lau: &Launcher, footer: &str) {
    let font = &lau.font;
    c.spawn((Node { width: px(SIDEBAR_W), flex_shrink: 0.0, height: percent(100), flex_direction: FlexDirection::Column, align_items: AlignItems::Center, padding: UiRect::axes(px(0), px(18)), row_gap: px(14), ..default() }, BackgroundColor(SIDEBAR)))
        .with_children(|side| {
            // Logo : une DS (deux écrans l'un sur l'autre), qui ramène aussi à la vue des jeux
            button(side, Node { width: px(110), height: px(110), flex_direction: FlexDirection::Column, align_items: AlignItems::Center, justify_content: JustifyContent::Center, row_gap: px(5), border_radius: BorderRadius::all(px(12)), ..default() }, ITEM_ON, Color::srgb(0.3, 0.44, 0.74))
                .observe(show_library)
                .with_children(|logo| {
                    for _ in 0..2 {
                        logo.spawn((Node { width: px(62), height: px(38), border: UiRect::all(px(3)), border_radius: BorderRadius::all(px(3)), ..default() }, BorderColor::all(TEXT), BackgroundColor(Color::srgb(0.08, 0.1, 0.16))));
                    }
                });
            // Vue des jeux : 4 carrés
            let selected = lau.open.is_none();
            let (normal, hover) = if selected { (ITEM_ON, ITEM_ON) } else { (ITEM, ITEM_HOVER) };
            button(side, Node { width: px(52), height: px(52), flex_direction: FlexDirection::Column, align_items: AlignItems::Center, justify_content: JustifyContent::Center, row_gap: px(6), border_radius: BorderRadius::all(px(8)), ..default() }, normal, hover)
                .observe(show_library)
                .with_children(|grid| {
                    for _ in 0..2 {
                        grid.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(6), ..default() }).with_children(|row| {
                            for _ in 0..2 {
                                row.spawn((Node { width: px(13), height: px(13), border_radius: BorderRadius::all(px(2)), ..default() }, BackgroundColor(TEXT)));
                            }
                        });
                    }
                });
            // Favoris
            side.spawn((
                Node { flex_grow: 1.0, flex_basis: px(0), min_height: px(0), width: percent(100), flex_direction: FlexDirection::Column, align_items: AlignItems::Center, row_gap: px(10), padding: UiRect::vertical(px(2)), overflow: Overflow::scroll_y(), ..default() },
                ScrollPosition::default(),
                Hovered::default(),
                Scrollable,
            ))
            .with_children(|favorites| {
                for rom in lau.prefs.favorites.iter().filter_map(|file| lau.roms.iter().find(|r| r.info.file == *file)) {
                    let file = rom.info.file.clone();
                    let shown = lau.open.as_deref() == Some(file.as_str());
                    let (normal, hover) = if shown { (ITEM_ON, ITEM_ON) } else { (Color::NONE, ITEM_HOVER) };
                    button(favorites, Node { padding: UiRect::all(px(4)), flex_shrink: 0.0, border_radius: BorderRadius::all(px(8)), ..default() }, normal, hover)
                        .observe(open_game(file))
                        .with_children(|b| cover(b, rom, 96.0));
                }
            });
            side.spawn((text(font, footer, 10.0, DIM), Node { max_width: px(SIDEBAR_W - 16.0), ..default() }));
        });
}

fn show_library(_: On<Activate>, mut lau: ResMut<Launcher>) {
    lau.open = None;
    lau.tag_input = None;
    lau.dirty = true;
}

fn open_game(file: String) -> impl FnMut(On<Activate>, ResMut<Launcher>) {
    move |_, mut lau| {
        // Relâché sur sa propre carte après l'avoir glissé : ce n'est pas un clic
        if lau.dragging.is_some() {
            return;
        }
        info!("jeu ouvert : {file}");
        lau.open = Some(file.clone());
        lau.tab = Tab::Play;
        lau.confirm_remove = None;
        lau.tag_input = None;
        lau.search_focus = false;
        lau.dirty = true;
    }
}

fn spawn_empty(main: &mut ChildSpawnerCommands, lau: &Launcher) {
    let font = &lau.font;
    main.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Column, justify_content: JustifyContent::Center, align_items: AlignItems::Center, row_gap: px(14), padding: UiRect::all(px(40)), ..default() }).with_children(|empty| {
        empty.spawn(text(font, tr("No games", "Aucun jeu"), 32.0, TEXT));
        empty.spawn(text(font, tr("Drag a DS rom (.nds, .dsi) into this window or choose it to add it to the launcher.", "Glissez une rom DS (.nds, .dsi) dans cette fenetre ou choisissez-la pour l'ajouter au launcher."), 18.0, TEXT));
        add_games_button(empty, font, ITEM_ON, Color::srgb(0.3, 0.44, 0.74), 16.0);
        empty.spawn(text(font, t!("It will be copied to {}", "Elle sera copiee dans {}", library::rom_dir().display()), 14.0, DIM));
    });
}

fn spawn_library(main: &mut ChildSpawnerCommands, lau: &Launcher) {
    let font = &lau.font;
    let roms = shown_roms(lau);
    let folders = shown_folders(lau);
    let count = roms.len();
    let searching = !lau.search.trim().is_empty();
    let games = if count == 1 { tr("1 game", "1 jeu").to_string() } else { t!("{count} games", "{count} jeux") };
    let empty_folder = !searching && count == 0 && !lau.folder.is_empty() && folders.is_empty();
    main.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), padding: UiRect::new(px(MAIN_PAD), px(MAIN_PAD), px(16), px(8)), ..default() }).with_children(|bar| {
        if !lau.folder.is_empty() {
            button(bar, button_node(), ITEM, ITEM_HOVER)
                .insert(DropFolder(library::parent_of(&lau.folder).to_string()))
                .observe(|_: On<Activate>, mut lau: ResMut<Launcher>| {
                    let parent = library::parent_of(&lau.folder).to_string();
                    enter_folder(&mut lau, parent);
                })
                .with_child(text(font, tr("<- Back", "<- Retour"), 14.0, TEXT));
        }
        let title = if lau.folder.is_empty() || searching { games } else { format!("{}   -   {games}", lau.folder) };
        bar.spawn((text(font, title, 16.0, DIM), Node { flex_grow: 1.0, ..default() }));
        if empty_folder {
            button(bar, button_node(), ITEM, ITEM_HOVER).observe(delete_folder).with_child(text(font, tr("Delete this empty folder", "Supprimer ce dossier vide"), 14.0, TEXT));
        }
        button(bar, button_node(), ITEM, ITEM_HOVER)
            .observe(|_: On<Activate>, mut lau: ResMut<Launcher>| {
                lau.naming = Some(String::new());
                lau.dirty = true;
            })
            .with_child(text(font, tr("New folder", "Nouveau dossier"), 14.0, TEXT));
        add_games_button(bar, font, ITEM_ON, Color::srgb(0.3, 0.44, 0.74), 14.0);
        for (view, name) in view_names() {
            view_button(bar, font, view, name);
        }
    });
    if let Some(name) = &lau.naming {
        main.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), padding: UiRect::new(px(MAIN_PAD), px(MAIN_PAD), px(0), px(8)), ..default() }).with_children(|row| {
            row.spawn((Node { padding: UiRect::axes(px(12), px(8)), min_width: px(320), border: UiRect::all(px(2)), border_radius: BorderRadius::all(px(6)), ..default() }, BorderColor::all(ITEM_ON), BackgroundColor(PANEL))).with_child(text(font, format!("{name}_"), 15.0, TEXT));
            button(row, button_node(), ITEM_ON, ITEM_HOVER)
                .observe(|_: On<Activate>, mut lau: ResMut<Launcher>, mut images: ResMut<Assets<Image>>, time: Res<Time<Real>>| create_named_folder(&mut lau, &mut images, &time))
                .with_child(text(font, tr("Create", "Creer"), 14.0, TEXT));
            button(row, button_node(), ITEM, ITEM_HOVER)
                .observe(|_: On<Activate>, mut lau: ResMut<Launcher>| {
                    lau.naming = None;
                    lau.dirty = true;
                })
                .with_child(text(font, tr("Cancel", "Annuler"), 14.0, TEXT));
            row.spawn(text(font, tr("Type the name, Enter = create, Esc = cancel", "Tapez le nom, Entree = creer, Echap = annuler"), 13.0, DIM));
        });
    }
    spawn_search_and_sort(main, lau);
    let mut scroll = main.spawn((
        Node { flex_grow: 1.0, flex_basis: px(0), min_height: px(0), width: percent(100), overflow: Overflow::scroll_y(), ..default() },
        ScrollPosition(Vec2::new(0.0, lau.grid_scroll)),
        Hovered::default(),
        Scrollable,
        GridScroll,
    ));
    scroll.with_children(|area| {
        if searching && roms.is_empty() && folders.is_empty() {
            area.spawn((text(font, t!("Nothing matches \"{}\"", "Rien ne correspond a \"{}\"", lau.search.trim()), 16.0, DIM), Node { margin: UiRect::all(px(MAIN_PAD)), ..default() }));
            return;
        }
        match lau.prefs.view {
            LibraryView::Grid => spawn_grid(area, lau, &roms, &folders),
            LibraryView::List => spawn_list(area, lau, &roms, &folders),
        }
    });
}

/// La barre de recherche, puis les boutons de tri.
fn spawn_search_and_sort(main: &mut ChildSpawnerCommands, lau: &Launcher) {
    let font = &lau.font;
    main.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), padding: UiRect::new(px(MAIN_PAD), px(MAIN_PAD), px(0), px(8)), ..default() }).with_children(|row| {
        let (border, shown, color) = match (lau.search_focus, lau.search.is_empty()) {
            (true, _) => (ITEM_ON, format!("{}_", lau.search), TEXT),
            (false, false) => (BORDER, lau.search.clone(), TEXT),
            (false, true) => (BORDER, tr("Search: name, publisher, year, genre, tag... (Ctrl+F)", "Rechercher : nom, editeur, annee, genre, tag... (Ctrl+F)").to_string(), DIM),
        };
        button(row, Node { flex_grow: 1.0, flex_basis: px(0), min_width: px(0), justify_content: JustifyContent::FlexStart, padding: UiRect::axes(px(12), px(8)), border: UiRect::all(px(2)), border_radius: BorderRadius::all(px(6)), ..default() }, PANEL, PANEL)
            .insert((BorderColor::all(border), SearchBox))
            .observe(|_: On<Activate>, mut lau: ResMut<Launcher>| {
                lau.search_focus = true;
                lau.dirty = true;
            })
            .with_child(text(font, shown, 15.0, color));
        if !lau.search.is_empty() {
            button(row, button_node(), ITEM, ITEM_HOVER)
                .observe(|_: On<Activate>, mut lau: ResMut<Launcher>| {
                    lau.search.clear();
                    lau.search_focus = false;
                    lau.scroll_reset = true;
                    lau.dirty = true;
                })
                .with_child(text(font, tr("Clear", "Effacer"), 14.0, TEXT));
        }
    });
    main.spawn(Node { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, align_items: AlignItems::Center, column_gap: px(4), row_gap: px(4), padding: UiRect::new(px(MAIN_PAD), px(MAIN_PAD), px(0), px(8)), ..default() }).with_children(|row| {
        row.spawn((text(font, tr("Sort:", "Trier :"), 13.0, DIM), Node { margin: UiRect::right(px(4)), ..default() }));
        for key in SortKey::ALL {
            let active = lau.prefs.sort == key;
            let (normal, hover) = if active { (ITEM_ON, ITEM_ON) } else { (ITEM, ITEM_HOVER) };
            // Pas de fleches dans toutes les polices
            let label = if !active { key.label().to_string() } else if lau.prefs.sort_reverse { format!("{} v", key.label()) } else { format!("{} ^", key.label()) };
            button(row, Node { padding: UiRect::axes(px(9), px(5)), ..button_node() }, normal, hover)
                .observe(move |_: On<Activate>, mut lau: ResMut<Launcher>| {
                    // Le même bouton une seconde fois inverse l'ordre
                    lau.prefs.sort_reverse = if lau.prefs.sort == key { !lau.prefs.sort_reverse } else { key.starts_reversed() };
                    lau.prefs.sort = key;
                    lau.prefs.store();
                    lau.scroll_reset = true;
                    lau.dirty = true;
                })
                .with_child(text(font, label, 13.0, TEXT));
        }
    });
}

fn genres_of(lau: &Launcher, rom: &Rom) -> String {
    lau.meta.genre_names(&rom.meta).join(", ")
}

/// L'éditeur écrit dans la rom ; GameTDB seulement si elle n'en dit rien.
fn publisher_of(rom: &Rom) -> &str {
    if rom.info.publisher.is_empty() { &rom.meta.publisher } else { &rom.info.publisher }
}

/// La valeur comparée par le tri choisi ; rien si on ne la connaît pas.
fn sort_cell(lau: &Launcher, rom: &Rom) -> Option<SortCell> {
    let label = |value: &str| (!value.is_empty()).then(|| SortCell::Text(library::fold(value)));
    match lau.prefs.sort {
        SortKey::Title => label(&rom.info.title),
        SortKey::Publisher => label(publisher_of(rom)),
        SortKey::Developer => label(&rom.meta.developer),
        SortKey::Year => rom.meta.year.map(|year| SortCell::Number(f32::from(year))),
        SortKey::Genre => label(&genres_of(lau, rom)),
        SortKey::Tag => label(&lau.prefs.tags_of(&rom.info.code).join(", ")),
        SortKey::Region => label(rom.info.region()),
        SortKey::Size => Some(SortCell::Number(rom.info.size as f32)),
        SortKey::Decomp => decompiled(&rom.info.code).map(SortCell::Number),
    }
}

/// Ce que montre un jeu sous son titre : l'éditeur, ou ce qu'on trie quand la carte ne le dit pas déjà.
fn sort_detail(lau: &Launcher, rom: &Rom) -> String {
    match lau.prefs.sort {
        SortKey::Developer => rom.meta.developer.clone(),
        SortKey::Year => rom.meta.year.map_or_else(String::new, |year| year.to_string()),
        SortKey::Genre => genres_of(lau, rom),
        SortKey::Tag => lau.prefs.tags_of(&rom.info.code).join(", "),
        SortKey::Region => rom.info.region().to_string(),
        SortKey::Size => format_size(rom.info.size),
        SortKey::Title | SortKey::Publisher | SortKey::Decomp => publisher_of(rom).to_string(),
    }
}

/// Tout ce que la recherche regarde.
fn search_text(lau: &Launcher, rom: &Rom) -> String {
    let info = &rom.info;
    let year = rom.meta.year.map_or_else(String::new, |year| year.to_string());
    let tags = lau.prefs.tags_of(&info.code).join(" ");
    let parts = [info.title.as_str(), publisher_of(rom), &rom.meta.publisher, &rom.meta.developer, &year, &genres_of(lau, rom), &rom.meta.genres.join(" "), &tags, info.region(), &info.code, &info.internal_name, &info.file];
    library::fold(&parts.join(" "))
}

/// Les jeux montrés, triés : ceux du dossier, ou ceux de tous les dossiers pendant une recherche.
fn shown_roms(lau: &Launcher) -> Vec<&Rom> {
    let query = library::fold(lau.search.trim());
    let games = lau.roms.iter().filter(|r| if query.is_empty() { library::parent_of(&r.info.file) == lau.folder } else { library::matches_search(&search_text(lau, r), &query) });
    library::sort_games(games.map(|r| (sort_cell(lau, r), library::fold(&r.info.title), r)).collect(), lau.prefs.sort_reverse)
}

/// Les dossiers montrés : ceux du dossier, ou ceux dont le nom correspond pendant une recherche.
fn shown_folders(lau: &Launcher) -> Vec<&String> {
    let query = library::fold(lau.search.trim());
    lau.folders.iter().filter(|f| if query.is_empty() { library::parent_of(f) == lau.folder } else { library::matches_search(&library::fold(f), &query) }).collect()
}

/// Les jeux d'un dossier, ses sous-dossiers compris.
fn games_in_folder(lau: &Launcher, folder: &str) -> String {
    let prefix = format!("{folder}/");
    let count = lau.roms.iter().filter(|r| r.info.file.starts_with(&prefix)).count();
    if count == 1 { tr("1 game", "1 jeu").to_string() } else { t!("{count} games", "{count} jeux") }
}

fn open_folder(folder: String) -> impl FnMut(On<Activate>, ResMut<Launcher>) {
    move |_, mut lau| enter_folder(&mut lau, folder.clone())
}

fn delete_folder(_: On<Activate>, mut lau: ResMut<Launcher>, mut images: ResMut<Assets<Image>>, time: Res<Time<Real>>) {
    let folder = lau.folder.clone();
    match library::remove_folder(&library::rom_dir(), &folder) {
        Ok(()) => {
            let parent = library::parent_of(&folder).to_string();
            enter_folder(&mut lau, parent);
            reload_roms(&mut lau, &mut images);
        }
        Err(e) => say(&mut lau, &time, e),
    }
}

/// Un dossier dessiné : son onglet et son corps.
fn folder_icon(c: &mut ChildSpawnerCommands, size: f32) {
    c.spawn(Node { width: px(size), height: px(size), flex_shrink: 0.0, flex_direction: FlexDirection::Column, justify_content: JustifyContent::Center, ..default() }).with_children(|icon| {
        icon.spawn((Node { width: percent(42), height: px(size * 0.12), border_radius: BorderRadius::top(px(size * 0.05)), ..default() }, BackgroundColor(GOLD)));
        icon.spawn((Node { width: percent(100), height: px(size * 0.62), border_radius: BorderRadius::all(px(size * 0.05)), ..default() }, BackgroundColor(GOLD)));
    });
}

fn view_names() -> [(LibraryView, &'static str); 2] {
    [(LibraryView::Grid, tr("Grid", "Grille")), (LibraryView::List, tr("List", "Liste"))]
}

fn view_button(c: &mut ChildSpawnerCommands, font: &Fonts, view: LibraryView, name: &str) {
    c.spawn((Button, Hovered::default(), button_node(), BackgroundColor(ITEM), ViewChoice(view)))
        .observe(move |_: On<Activate>, mut lau: ResMut<Launcher>| {
            if lau.prefs.view != view {
                lau.prefs.view = view;
                lau.prefs.store();
                lau.grid_scroll = 0.0;
                lau.dirty = true;
            }
        })
        .with_child(text(font, name, 14.0, TEXT));
}

fn spawn_grid(area: &mut ChildSpawnerCommands, lau: &Launcher, roms: &[&Rom], folders: &[&String]) {
    let font = &lau.font;
    area.spawn(Node { width: percent(100), height: auto(), flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, align_content: AlignContent::FlexStart, column_gap: px(20), row_gap: px(20), padding: UiRect::new(px(MAIN_PAD), px(MAIN_PAD), px(6), px(84)), ..default() }).with_children(|grid| {
        for folder in folders.iter().copied() {
            button(grid, Node { width: px(190), height: px(268), flex_shrink: 0.0, flex_direction: FlexDirection::Column, align_items: AlignItems::Center, padding: UiRect::all(px(10)), row_gap: px(8), border_radius: BorderRadius::all(px(10)), overflow: Overflow::clip(), ..default() }, PANEL, ITEM_HOVER)
                .insert(DropFolder(folder.clone()))
                .observe(open_folder(folder.clone()))
                .with_children(|card| {
                    folder_icon(card, 170.0);
                    card.spawn((text(font, library::last_name(folder), 14.0, TEXT), Node { width: percent(100), ..default() }));
                    card.spawn((text(font, games_in_folder(lau, folder), 12.0, DIM), Node { width: percent(100), ..default() }));
                });
        }
        for rom in roms.iter().copied() {
            button(grid, Node { width: px(190), height: px(268), flex_shrink: 0.0, flex_direction: FlexDirection::Column, align_items: AlignItems::Center, padding: UiRect::all(px(10)), row_gap: px(8), border_radius: BorderRadius::all(px(10)), overflow: Overflow::clip(), ..default() }, PANEL, ITEM_HOVER)
                .observe(open_game(rom.info.file.clone()))
                .observe(start_drag(rom.info.file.clone()))
                .with_children(|card| {
                    cover(card, rom, 170.0);
                    if lau.prefs.is_favorite(&rom.info.file) {
                        card.spawn(Node { position_type: PositionType::Absolute, top: px(16), right: px(16), ..default() }).with_children(|b| star(b, lau, &rom.info.file, 22.0));
                    }
                    card.spawn((text(font, rom.info.title.clone(), 14.0, TEXT), Node { width: percent(100), ..default() }));
                    card.spawn((text(font, sort_detail(lau, rom), 12.0, DIM), Node { width: percent(100), ..default() }));
                    if let Some(code) = decompiled(&rom.info.code) {
                        progress_bar(card, code);
                    }
                });
        }
    });
}

/// Le pourcentage de code décompilé de ce jeu.
fn decompiled(code: &str) -> Option<f32> {
    crate::native_game::game_state(code)?.code
}

fn progress_bar(c: &mut ChildSpawnerCommands, value: f32) {
    c.spawn((Node { width: percent(100), height: px(5), flex_shrink: 0.0, border_radius: BorderRadius::all(px(3)), overflow: Overflow::clip(), ..default() }, BackgroundColor(ITEM))).with_child((Node { width: percent(value.clamp(0.0, 100.0)), height: percent(100), ..default() }, BackgroundColor(GOOD)));
}

fn spawn_list(area: &mut ChildSpawnerCommands, lau: &Launcher, roms: &[&Rom], folders: &[&String]) {
    let font = &lau.font;
    // Ce qu'on trie remplace le code quand aucune colonne ne le montre
    let sorted_column = matches!(lau.prefs.sort, SortKey::Developer | SortKey::Year | SortKey::Genre | SortKey::Tag);
    let (publisher_width, second): (f32, (&str, f32)) = if sorted_column { (110.0, (lau.prefs.sort.label(), 120.0)) } else { (170.0, ("Code", 60.0)) };
    let columns: [(&str, f32); 5] = [(tr("Publisher", "Editeur"), publisher_width), second, ("Region", 70.0), (tr("Size", "Taille"), 70.0), ("Decomp", 70.0)];
    area.spawn(Node { width: percent(100), height: auto(), flex_direction: FlexDirection::Column, row_gap: px(4), padding: UiRect::new(px(MAIN_PAD), px(MAIN_PAD), px(6), px(84)), ..default() }).with_children(|list| {
        list.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(14), padding: UiRect::axes(px(8), px(4)), ..default() }).with_children(|head| {
            head.spawn(Node { width: px(44), ..default() });
            head.spawn((text(font, tr("Game", "Jeu"), 13.0, DIM), Node { flex_grow: 1.0, flex_basis: px(0), ..default() }));
            for (name, width) in columns {
                head.spawn((text(font, name, 13.0, DIM), Node { width: px(width), ..default() }));
            }
            head.spawn(Node { width: px(22), ..default() });
        });
        for folder in folders.iter().copied() {
            button(list, Node { flex_direction: FlexDirection::Row, flex_shrink: 0.0, align_items: AlignItems::Center, column_gap: px(14), padding: UiRect::axes(px(8), px(6)), border_radius: BorderRadius::all(px(6)), ..default() }, PANEL, ITEM_HOVER)
                .insert(DropFolder(folder.clone()))
                .observe(open_folder(folder.clone()))
                .with_children(|row| {
                    folder_icon(row, 44.0);
                    row.spawn((text(font, library::last_name(folder), 15.0, TEXT), Node { flex_grow: 1.0, flex_basis: px(0), min_width: px(0), ..default() }));
                    row.spawn(text(font, games_in_folder(lau, folder), 14.0, DIM));
                });
        }
        for rom in roms.iter().copied() {
            let info = &rom.info;
            button(list, Node { flex_direction: FlexDirection::Row, flex_shrink: 0.0, align_items: AlignItems::Center, column_gap: px(14), padding: UiRect::axes(px(8), px(6)), border_radius: BorderRadius::all(px(6)), ..default() }, PANEL, ITEM_HOVER)
                .observe(open_game(info.file.clone()))
                .observe(start_drag(info.file.clone()))
                .with_children(|row| {
                    cover(row, rom, 44.0);
                    row.spawn((text(font, info.title.clone(), 15.0, TEXT), Node { flex_grow: 1.0, flex_basis: px(0), min_width: px(0), ..default() }));
                    let decomp = decompiled(&info.code).map_or_else(|| "-".to_string(), |v| format!("{v:.1} %"));
                    let second = if sorted_column { sort_detail(lau, rom) } else { info.code.clone() };
                    let values = [publisher_of(rom).to_string(), second, info.region().to_string(), format_size(info.size), decomp];
                    for ((_, width), value) in columns.iter().zip(values) {
                        row.spawn((text(font, value, 14.0, DIM), Node { width: px(*width), ..default() }));
                    }
                    star(row, lau, &info.file, 22.0);
                });
        }
    });
}

// ─────────────────────────────────────────────────────────────────────────
//  Le panneau d'activité
// ─────────────────────────────────────────────────────────────────────────

fn spawn_activity_panel(root: &mut ChildSpawnerCommands, font: &Fonts) {
    root.spawn((
        Node { width: px(ACTIVITY_W), flex_shrink: 0.0, height: percent(100), flex_direction: FlexDirection::Column, row_gap: px(8), padding: UiRect::new(px(16), px(16), px(16), px(76)), ..default() },
        BackgroundColor(SIDEBAR),
        ActivityPanel,
    ))
    .with_children(|panel| {
        let heading = |panel: &mut ChildSpawnerCommands, en: &'static str, fr: &'static str| {
            panel.spawn((text(font, tr(en, fr), 14.0, DIM), Tr(en, fr), Node { flex_shrink: 0.0, margin: UiRect::top(px(6)), ..default() }));
        };
        heading(panel, "Now", "En ce moment");
        panel.spawn((Node { flex_shrink: 0.0, flex_direction: FlexDirection::Column, row_gap: px(4), ..default() }, ActivityNow));
        heading(panel, "Decompilation", "Decompilation");
        panel.spawn((Node { flex_shrink: 0.0, flex_direction: FlexDirection::Column, row_gap: px(6), ..default() }, ActivityDecomp));
        heading(panel, "History", "Historique");
        panel.spawn((
            Node { flex_grow: 1.0, flex_basis: px(0), min_height: px(0), flex_direction: FlexDirection::Column, row_gap: px(8), overflow: Overflow::scroll_y(), ..default() },
            ScrollPosition::default(),
            Hovered::default(),
            Scrollable,
            ActivityHistory,
        ));
    });
}

/// Le panneau va avec la vue des jeux, et laisse la place aux jeux dans une petite fenêtre.
fn activity_visibility(lau: Res<Launcher>, screens: Res<DsScreens>, windows: Query<&Window>, mut panels: Query<&mut Node, With<ActivityPanel>>) {
    let wide = windows.get(screens.main_window()).is_ok_and(|w| w.width() >= ACTIVITY_MIN_WINDOW);
    let wanted = if lau.open.is_none() && wide { Display::Flex } else { Display::None };
    for mut node in &mut panels {
        if node.display != wanted {
            node.display = wanted;
        }
    }
}

/// Une fois par seconde : ces textes changent lentement et les refaire coûte plus que les comparer.
fn second_passed(time: &Time<Real>, next: &mut f64) -> bool {
    if time.elapsed_secs_f64() < *next {
        return false;
    }
    *next = time.elapsed_secs_f64() + 1.0;
    true
}

fn game_title(lau: &Launcher, code: &str) -> String {
    lau.roms.iter().find(|r| r.info.code == code).map_or_else(|| code.to_string(), |r| r.info.title.clone())
}

fn shorten(line: &str, limit: usize) -> String {
    if line.chars().count() <= limit { line.to_string() } else { line.chars().take(limit).chain("...".chars()).collect() }
}

fn clear_children(commands: &mut Commands, parent: Entity, children: &Query<&Children>) {
    if let Ok(old) = children.get(parent) {
        for child in old.iter() {
            commands.entity(child).despawn();
        }
    }
}

/// Ce que l'app fait maintenant : la préparation d'un jeu avec ses dernières lignes, les jaquettes, les copies.
#[allow(clippy::too_many_arguments)]
fn activity_now(mut commands: Commands, lau: Res<Launcher>, time: Res<Time<Real>>, mut next: Local<f64>, mut shown: Local<Vec<(String, f32, bool)>>, boxes: Query<Entity, With<ActivityNow>>, children: Query<&Children>) {
    if !second_passed(&time, &mut next) {
        return;
    }
    // (texte, taille, ligne principale)
    let mut lines: Vec<(String, f32, bool)> = Vec::new();
    if let Some(job) = crate::native_game::current_job() {
        lines.push((t!("Preparing {} ({} s)", "Preparation de {} ({} s)", game_title(&lau, &job.code), job.seconds), 15.0, true));
        if !job.message.is_empty() {
            lines.push((job.message, 12.0, false));
        }
        lines.extend(job.lines.iter().map(|line| (shorten(line, ACTIVITY_LINE), 11.0, false)));
    }
    if let Some(job) = &lau.cover_job {
        let p = job.lock().unwrap();
        lines.push((t!("Downloading covers: {} of {}", "Telechargement des jaquettes : {} sur {}", p.done, p.total), 14.0, true));
    }
    if lau.copying > 0 {
        lines.push((t!("Copying roms: {}", "Copie de roms : {}", lau.copying), 14.0, true));
    }
    if lines.is_empty() {
        lines.push((tr("Nothing running.", "Rien en cours.").to_string(), 14.0, false));
    }
    if *shown == lines {
        return;
    }
    let font = lau.font.clone();
    for parent in &boxes {
        clear_children(&mut commands, parent, &children);
        commands.entity(parent).with_children(|c| {
            for (line, size, main) in &lines {
                c.spawn(text(&font, line.clone(), *size, if *main { TEXT } else { DIM }));
            }
        });
    }
    *shown = lines;
}

/// La moyenne des jeux installés et où en est leur préparation.
fn activity_decomp(mut commands: Commands, lau: Res<Launcher>, time: Res<Time<Real>>, mut next: Local<f64>, mut shown: Local<Option<(i32, i32, i32, crate::native_game::Overview, usize)>>, boxes: Query<Entity, With<ActivityDecomp>>, children: Query<&Children>) {
    if !second_passed(&time, &mut next) {
        return;
    }
    let states: Vec<_> = lau.roms.iter().filter_map(|r| crate::native_game::game_state(&r.info.code)).collect();
    let average = |pick: fn(&crate::native_game::GameState) -> Option<f32>| {
        let values: Vec<f32> = states.iter().filter_map(pick).collect();
        // En dixièmes de point : comparé à ce qui est montré
        if values.is_empty() { 0 } else { (values.iter().sum::<f32>() / values.len() as f32 * 10.0).round() as i32 }
    };
    let (code, c_code, data) = (average(|s| s.code), average(|s| s.c), average(|s| s.data));
    let overview = crate::native_game::overview(lau.roms.iter().map(|r| r.info.code.as_str()), lau.prepare_all);
    let state = Some((code, c_code, data, overview, lau.roms.len()));
    if *shown == state {
        return;
    }
    *shown = state;
    let font = lau.font.clone();
    let games = lau.roms.len();
    for parent in &boxes {
        clear_children(&mut commands, parent, &children);
        commands.entity(parent).with_children(|c| {
            c.spawn(text(&font, t!("Code matched: {:.1} %", "Code decompile : {:.1} %", code as f32 / 10.0), 15.0, TEXT));
            progress_bar(c, code as f32 / 10.0);
            c.spawn(text(&font, t!("In C: {:.1} %   Data: {:.1} %", "En C : {:.1} %   Donnees : {:.1} %", c_code as f32 / 10.0, data as f32 / 10.0), 13.0, DIM));
            c.spawn(text(&font, t!("Prepared: {} of {games} games", "Prepares : {} jeux sur {games}", overview.ready), 13.0, DIM));
            if overview.waiting > 0 {
                c.spawn(text(&font, t!("Waiting: {}", "En attente : {}", overview.waiting), 13.0, DIM));
            }
            if overview.failed > 0 {
                c.spawn(text(&font, t!("Errors: {}", "En erreur : {}", overview.failed), 13.0, BAD));
            }
        });
    }
}

/// L'historique : refait quand un événement arrive, quand la langue change, et chaque minute pour les « il y a ».
#[allow(clippy::too_many_arguments)]
fn activity_history(mut commands: Commands, lau: Res<Launcher>, language: Res<Language>, time: Res<Time<Real>>, mut shown: Local<Option<(u64, u64, usize, Lang)>>, boxes: Query<Entity, With<ActivityHistory>>, added: Query<(), Added<ActivityHistory>>, children: Query<&Children>) {
    let state = Some((activity::revision(), (time.elapsed_secs_f64() / 60.0) as u64, lau.roms.len(), language.0));
    if *shown == state && added.is_empty() {
        return;
    }
    *shown = state;
    let events = activity::latest(ACTIVITY_ROWS);
    let now = activity::now_secs();
    let font = lau.font.clone();
    let title = |code: &str| game_title(&lau, code);
    let rows: Vec<(Color, String, String)> = events
        .iter()
        .map(|event| {
            let color = match event.what.level() {
                activity::Level::Good => GOOD,
                activity::Level::Error => BAD,
                activity::Level::Update => GOLD,
                activity::Level::Info => INFO,
            };
            (color, event.what.text(&title), activity::ago(event.time, now))
        })
        .collect();
    for parent in &boxes {
        clear_children(&mut commands, parent, &children);
        commands.entity(parent).with_children(|c| {
            if rows.is_empty() {
                c.spawn(text(&font, tr("Nothing yet.", "Rien pour l'instant."), 13.0, DIM));
            }
            for (color, line, when) in &rows {
                c.spawn(Node { flex_shrink: 0.0, flex_direction: FlexDirection::Row, column_gap: px(8), ..default() }).with_children(|row| {
                    row.spawn((Node { width: px(8), height: px(8), flex_shrink: 0.0, margin: UiRect::top(px(5)), border_radius: BorderRadius::all(px(4)), ..default() }, BackgroundColor(*color)));
                    row.spawn(Node { flex_grow: 1.0, flex_basis: px(0), min_width: px(0), flex_direction: FlexDirection::Column, ..default() }).with_children(|lines| {
                        lines.spawn(text(&font, line.clone(), 13.0, TEXT));
                        lines.spawn(text(&font, when.clone(), 11.0, DIM));
                    });
                });
            }
        });
    }
}

/// Une nouvelle version trouvée a sa ligne dans l'historique.
fn note_update(offer: Res<UpdateOffer>, mut noted: Local<String>) {
    let Some(info) = offer.0.lock().unwrap().clone() else { return };
    if *noted != info.version {
        activity::update_found(&info.version);
        *noted = info.version;
    }
}

// ─────────────────────────────────────────────────────────────────────────
//  La page d'un jeu
// ─────────────────────────────────────────────────────────────────────────

fn spawn_game(main: &mut ChildSpawnerCommands, lau: &Launcher, rom: &Rom, registry: &GameRegistry) {
    let font = &lau.font;
    let file = rom.info.file.clone();
    // Nom, boutons des onglets, favori
    main.spawn(Node { height: px(HEADER_H), flex_shrink: 0.0, flex_direction: FlexDirection::Column, row_gap: px(14), padding: UiRect::new(px(MAIN_PAD + COVER + 20.0), px(112), px(MAIN_PAD), px(0)), ..default() }).with_children(|header| {
        header.spawn((Node { padding: UiRect::axes(px(16), px(8)), border: UiRect::all(px(2)), border_radius: BorderRadius::all(px(6)), overflow: Overflow::clip(), ..default() }, BorderColor::all(BORDER))).with_child(text(font, rom.info.title.clone(), 20.0, TEXT));
        header.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(10), ..default() }).with_children(|tabs| {
            for tab in Tab::ALL {
                let (normal, hover) = if lau.tab == tab { (ITEM_ON, ITEM_ON) } else { (ITEM, ITEM_HOVER) };
                button(tabs, Node { flex_grow: 1.0, flex_basis: px(0), ..button_node() }, normal, hover)
                    .observe(move |_: On<Activate>, mut lau: ResMut<Launcher>| {
                        lau.tab = tab;
                        lau.confirm_remove = None;
                        lau.dirty = true;
                    })
                    .with_child(text(font, tab.label(), 15.0, TEXT));
            }
        });
    });
    button(main, Node { position_type: PositionType::Absolute, top: px(16), right: px(24), width: px(72), height: px(72), justify_content: JustifyContent::Center, align_items: AlignItems::Center, border_radius: BorderRadius::all(px(8)), border: UiRect::all(px(2)), ..default() }, PANEL, ITEM_HOVER)
        .insert(BorderColor::all(BORDER))
        .observe({
            let file = file.clone();
            move |_: On<Activate>, mut lau: ResMut<Launcher>| toggle_favorite(&mut lau, &file)
        })
        .with_children(|b| star(b, lau, &file, 44.0));

    // Zone du bas : un haut à côté de l'image (qui déborde dessus), le reste défile sous elle
    main.spawn((
        Node { flex_grow: 1.0, flex_basis: px(0), min_height: px(0), margin: UiRect::new(px(MAIN_PAD), px(MAIN_PAD), px(0), px(MAIN_PAD)), border: UiRect::all(px(2)), border_radius: BorderRadius::all(px(8)), flex_direction: FlexDirection::Column, overflow: Overflow::clip(), ..default() },
        BorderColor::all(BORDER),
        BackgroundColor(PANEL),
    ))
    .with_children(|zone| {
        let beside = COVER + 20.0;
        let (top, rest): (Top, Rest) = match lau.tab {
            Tab::Play => play_tab(rom, registry),
            Tab::Modding => modding_tab(rom, registry, lau.mod_link.clone(), lau.typing_host),
            Tab::Description => description_tab(lau, rom),
            Tab::Settings => settings_tab(lau, rom, registry),
        };
        zone.spawn(Node { flex_shrink: 0.0, min_height: px(MAIN_PAD + COVER - HEADER_H + 14.0), flex_direction: FlexDirection::Column, row_gap: px(10), padding: UiRect::new(px(beside - 2.0), px(20), px(16), px(10)), ..default() }).with_children(|t| top(t, font));
        zone.spawn((
            Node { flex_grow: 1.0, flex_basis: px(0), min_height: px(0), flex_direction: FlexDirection::Column, row_gap: px(8), padding: UiRect::new(px(20), px(20), px(4), px(70)), overflow: Overflow::scroll_y(), ..default() },
            ScrollPosition::default(),
            Hovered::default(),
            Scrollable,
        ))
        .with_children(|r| rest(r, font));
    });
    // L'image, par-dessus le coin de la zone du bas
    main.spawn((Node { position_type: PositionType::Absolute, top: px(MAIN_PAD), left: px(MAIN_PAD), ..default() }, GlobalZIndex(1))).with_children(|b| cover(b, rom, COVER));
}

type Top = Box<dyn FnOnce(&mut ChildSpawnerCommands, &Fonts)>;
type Rest = Box<dyn FnOnce(&mut ChildSpawnerCommands, &Fonts)>;

/// Module natif du jeu : le jeu enregistré dont le game.toml donne ce code.
fn native_game(registry: &GameRegistry, code: &str) -> Option<String> {
    registry.specs().find(|s| !s.game_code.is_empty() && s.game_code.eq_ignore_ascii_case(code)).map(|s| s.id.clone())
}

/// Lance le jeu avec cette rom, sans changer la version choisie dans l'onglet Moding.
fn play_version_button(parent: &mut ChildSpawnerCommands, font: &Fonts, label: &str, id: String, rom: PathBuf) {
    button(parent, Node { align_self: AlignSelf::FlexStart, ..button_node() }, ITEM, ITEM_HOVER)
        .observe(move |_: On<Activate>, mut commands: Commands| {
            commands.insert_resource(ActiveRom(rom.clone()));
            commands.insert_resource(ActiveGame(Some(id.clone())));
            commands.set_state(StudioState::Playing);
        })
        .with_child(text(font, label.to_string(), 15.0, TEXT));
}

fn play_tab(rom: &Rom, registry: &GameRegistry) -> (Top, Rest) {
    let native = native_game(registry, &rom.info.code);
    let experimental = native.is_some() && crate::native_game::is_experimental(&rom.info.code);
    let code = rom.info.code.clone();
    let code_for_state = code.clone();
    let state = crate::native_game::preparation(&code);
    let path = rom.info.path.clone();
    let original = path.clone();
    let summary = format!("{}  -  {}  -  {}", rom.info.code, rom.info.region(), format_size(rom.info.size));
    (
        Box::new(move |t, font| {
            use crate::native_game::Preparation;
            let running = matches!(state, Preparation::Running(_));
            let usable = native.is_some() && !running;
            let (normal, hover) = if usable { (PLAY, PLAY_HOVER) } else { (ITEM, ITEM) };
            let mut play = button(t, Node { align_self: AlignSelf::FlexStart, padding: UiRect::axes(px(48), px(14)), ..button_node() }, normal, hover);
            if let (Some(id), false) = (native.clone(), running) {
                let ready = matches!(state, Preparation::Ready);
                play.observe(move |_: On<Activate>, mut commands: Commands, mut lau: ResMut<Launcher>| {
                    if ready {
                        // La rom de l'utilisateur, ou sa copie modifiée si les mods de ce jeu sont actifs
                        commands.insert_resource(ActiveRom(crate::mods::rom_to_play(&code, &path)));
                        commands.insert_resource(ActiveGame(Some(id.clone())));
                        commands.set_state(StudioState::Playing);
                    } else {
                        // Pas d'attente devant un écran : la page reste là et le bouton s'active quand c'est prêt
                        crate::native_game::prepare(&code, &path);
                        lau.dirty = true;
                    }
                });
            }
            let label = match (&state, native.is_some()) {
                (Preparation::Running(_), _) => tr("Preparing...", "Preparation..."),
                (Preparation::NotStarted | Preparation::Failed(_), true) => tr("Prepare", "Preparer"),
                _ => tr("Play", "Jouer"),
            };
            play.with_child(text(font, label, 22.0, if usable { TEXT } else { DIM }));
            if let (Some(id), true) = (native.clone(), matches!(state, Preparation::Ready)) {
                if crate::mods::is_on(&code_for_state) {
                    play_version_button(t, font, tr("Play the original version", "Jouer a la version originale"), id, original.clone());
                } else if crate::mods::has_modded_rom(&code_for_state) {
                    play_version_button(t, font, tr("Play the modded version", "Jouer a la version modee"), id, crate::mods::modded_rom(&code_for_state));
                }
            }
            match &state {
                Preparation::Running(message) if !message.is_empty() => {
                    t.spawn(text(font, t!("{message} You can keep using the launcher.", "{message} Vous pouvez continuer a utiliser le launcher."), 15.0, DIM));
                }
                Preparation::Failed(e) => {
                    t.spawn(text(font, t!("This game could not be prepared: {e}", "Ce jeu n'a pas pu etre prepare : {e}"), 15.0, DIM));
                }
                _ => {}
            }
            if native.is_none() {
                let reason = if crate::native_game::windows_only(&code_for_state) {
                    tr("Not playable on this system yet: native games only run on Windows and macOS for now.", "Pas encore jouable sur ce systeme : les jeux natifs ne tournent que sous Windows et macOS pour l'instant.")
                } else {
                    tr("Not playable yet: this game has no native module.", "Pas encore jouable : ce jeu n'a pas de module natif.")
                };
                t.spawn(text(font, reason, 15.0, DIM));
            }
            if let Some(game) = crate::native_game::game_state(&code_for_state) {
                let percent = |v: Option<f32>| v.map_or_else(|| "?".to_string(), |v| format!("{v:.1} %"));
                let play = match game.play.as_str() {
                    "playable" => tr("playable (tested until in-game)", "jouable (essaye jusqu'en jeu)"),
                    "starts" => tr("starts", "demarre"),
                    "crash" => tr("crashes at start", "plante au demarrage"),
                    "stuck" => tr("stuck at start", "bloque au demarrage"),
                    _ => tr("not tested", "pas essaye"),
                };
                t.spawn(text(font, t!("Code matched: {}   C: {}   Data: {}", "Code decompile : {}   C : {}   Donnees : {}", percent(game.code), percent(game.c), percent(game.data)), 15.0, TEXT));
                t.spawn(text(font, t!("Playability: {play}", "Jouabilite : {play}"), 15.0, TEXT));
            }
            if experimental {
                t.spawn(text(font, tr("Experimental: this game may crash or show nothing yet.", "Experimental : ce jeu peut planter ou ne rien afficher pour l'instant."), 15.0, DIM));
            }
            if crate::mods::is_on(&code_for_state) {
                t.spawn(text(font, tr("Mods are on: the game starts with your changes (Modding tab to go back to the originals).", "Mods actifs : le jeu demarre avec vos changements (onglet Moding pour revenir aux originaux)."), 15.0, GOLD));
            }
            t.spawn(text(font, summary, 14.0, DIM));
        }),
        Box::new(|_, _| {}),
    )
}

/// L'option « outils de mod » : à télécharger, puis exporter, modifier, appliquer, et revenir aux originaux quand on veut.
fn spawn_mods(r: &mut ChildSpawnerCommands, font: &Fonts, code: &str, rom: &Path, native: Option<&str>) {
    use crate::mods;
    let wide = |r: &mut ChildSpawnerCommands| {
        r.spawn(Node { height: px(2), margin: UiRect::vertical(px(10)), flex_shrink: 0.0, ..default() }).insert(BackgroundColor(BORDER));
    };
    wide(r);
    r.spawn(text(font, tr("Mod tools (optional)", "Outils de mod (option)"), 18.0, TEXT));
    let running = mods::running();
    if !mods::installed() {
        r.spawn(text(font, tr("Export the game's files as PNG, OBJ, WAV, MP3, MP4 and TXT for GIMP, Blender, Godot and Audacity, change them, and play with your changes. Your rom is never modified.", "Exporte les fichiers du jeu en PNG, OBJ, WAV, MP3, MP4 et TXT pour GIMP, Blender, Godot et Audacity, modifiez-les, et jouez avec vos changements. Votre rom n'est jamais modifiee."), 15.0, DIM));
        let label = if running { tr("Downloading...", "Telechargement...") } else { tr("Download the mod tools", "Telecharger les outils de mod") };
        let (normal, hover) = if running { (ITEM, ITEM) } else { (PLAY, PLAY_HOVER) };
        button(r, Node { align_self: AlignSelf::FlexStart, ..button_node() }, normal, hover)
            .observe(|_: On<Activate>, mut lau: ResMut<Launcher>| {
                crate::mods::install();
                lau.dirty = true;
            })
            .with_child(text(font, label, 15.0, TEXT));
    } else {
        r.spawn(Node { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, column_gap: px(8), row_gap: px(8), ..default() }).with_children(|row| {
            let (shade, shade_hover) = if running { (ITEM, ITEM) } else { (ITEM, ITEM_HOVER) };
            let (export_code, export_rom) = (code.to_string(), rom.to_path_buf());
            button(row, button_node(), shade, shade_hover)
                .observe(move |_: On<Activate>, mut lau: ResMut<Launcher>| {
                    crate::mods::export(&export_code, &export_rom);
                    lau.dirty = true;
                })
                .with_child(text(font, tr("1. Export the files", "1. Exporter les fichiers"), 15.0, TEXT));
            let open_code = code.to_string();
            button(row, button_node(), ITEM, ITEM_HOVER)
                .observe(move |_: On<Activate>| {
                    let dir = crate::mods::mod_dir(&open_code);
                    let _ = std::fs::create_dir_all(&dir);
                    open_folder_in_system(&dir);
                })
                .with_child(text(font, tr("2. Open the mod folder", "2. Ouvrir le dossier du mod"), 15.0, TEXT));
            let (apply_code, apply_rom) = (code.to_string(), rom.to_path_buf());
            button(row, button_node(), shade, shade_hover)
                .observe(move |_: On<Activate>, mut lau: ResMut<Launcher>| {
                    crate::mods::apply(&apply_code, &apply_rom);
                    lau.dirty = true;
                })
                .with_child(text(font, tr("3. Apply my changes", "3. Appliquer mes changements"), 15.0, TEXT));
        });
        if mods::has_modded_rom(code) {
            let on = mods::is_on(code);
            r.spawn(text(font, tr("The game starts with:", "Le jeu demarre avec :"), 15.0, DIM));
            r.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(8), ..default() }).with_children(|row| {
                for (value, label) in [(false, tr("The originals", "Les originaux")), (true, tr("My mods", "Mes mods"))] {
                    let (normal, hover) = if on == value { (ITEM_ON, ITEM_ON) } else { (ITEM, ITEM_HOVER) };
                    let choice_code = code.to_string();
                    button(row, button_node(), normal, hover)
                        .observe(move |_: On<Activate>, mut lau: ResMut<Launcher>| {
                            crate::mods::set_on(&choice_code, value);
                            lau.dirty = true;
                        })
                        .with_child(text(font, label, 15.0, TEXT));
                }
            });
        }
    }
    // La copie reste jouable sans l'outil
    if mods::has_modded_rom(code) && !running {
        if let (Some(id), crate::native_game::Preparation::Ready) = (native, crate::native_game::preparation(code)) {
            play_version_button(r, font, tr("Play the modded version", "Jouer a la version modee"), id.to_string(), mods::modded_rom(code));
        }
    }
    let (lines, error) = mods::lines_for(code);
    if let Some(e) = error {
        r.spawn(text(font, e, 15.0, BAD));
    }
    for line in lines {
        r.spawn(text(font, line, 14.0, DIM));
    }
    if !running {
        for line in mods::report_lines(code) {
            r.spawn(text(font, line, 14.0, DIM));
        }
        if mods::installed() {
            button(r, Node { align_self: AlignSelf::FlexStart, margin: UiRect::top(px(10)), ..button_node() }, ITEM, ITEM_HOVER)
                .observe(|_: On<Activate>, mut lau: ResMut<Launcher>| {
                    crate::mods::remove_tool();
                    lau.dirty = true;
                })
                .with_child(text(font, tr("Remove the mod tools (your mods stay)", "Retirer les outils de mod (vos mods restent)"), 14.0, TEXT));
        }
    }
}

/// Les mods venus d'ailleurs : un fichier, un dossier ou un lien, et la liste de ceux qui sont installés.
fn spawn_mod_packs(r: &mut ChildSpawnerCommands, font: &Fonts, code: &str, rom: &Path, link: Option<String>, typing_host: bool) {
    use crate::mod_packs;
    mod_packs::offer_base(code);
    let host_typed = if typing_host { link.clone() } else { None };
    let link = if typing_host { None } else { link };
    r.spawn((Node { height: px(2), margin: UiRect::vertical(px(10)), flex_shrink: 0.0, ..default() }, BackgroundColor(BORDER)));
    r.spawn(text(font, tr("Mods from elsewhere", "Mods venus d'ailleurs"), 18.0, TEXT));
    r.spawn(text(font, tr("A .zip, .7z, .rar or .tar.gz file, a folder, or a link: drop it here, or use the buttons. A mod only holds game files: nothing is run on your computer.", "Un fichier .zip, .7z, .rar ou .tar.gz, un dossier ou un lien : deposez-le ici, ou utilisez les boutons. Un mod ne contient que des fichiers de jeu : rien n'est execute sur votre ordinateur."), 15.0, DIM));
    r.spawn(Node { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, column_gap: px(8), row_gap: px(8), ..default() }).with_children(|row| {
        for (folder, label) in [(false, tr("Add a file", "Ajouter un fichier")), (true, tr("Add a folder", "Ajouter un dossier"))] {
            let (code, rom) = (code.to_string(), rom.to_path_buf());
            button(row, button_node(), ITEM, ITEM_HOVER)
                .observe(move |_: On<Activate>| {
                    let (code, rom) = (code.clone(), rom.clone());
                    // Le dialogue bloque jusqu'à sa fermeture ; sur macOS, rfd l'ouvre de lui-même sur le fil principal
                    std::thread::spawn(move || {
                        let dialog = rfd::FileDialog::new().set_title(tr("Add a mod", "Ajouter un mod"));
                        let picked = if folder { dialog.pick_folder() } else { dialog.add_filter(tr("Mods", "Mods"), &crate::mod_packs::ARCHIVES).pick_file() };
                        if let Some(path) = picked {
                            crate::mod_packs::add_path(&code, &rom, &path);
                        }
                    });
                })
                .with_child(text(font, label, 15.0, TEXT));
        }
    });
    r.spawn(Node { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, column_gap: px(8), row_gap: px(8), align_items: AlignItems::Center, ..default() }).with_children(|row| {
        let shown = match &link {
            Some(link) => format!("{link}_"),
            None => tr("Click here, then type or paste a link (https://github.com/author/mods)", "Cliquez ici, puis tapez ou collez un lien (https://github.com/auteur/mods)").to_string(),
        };
        let (normal, hover) = if link.is_some() { (ITEM_ON, ITEM_ON) } else { (ITEM, ITEM_HOVER) };
        button(row, Node { flex_grow: 1.0, min_width: px(320), ..button_node() }, normal, hover)
            .observe(|_: On<Activate>, mut lau: ResMut<Launcher>| {
                if lau.mod_link.is_none() {
                    lau.mod_link = Some(String::new());
                    lau.dirty = true;
                }
            })
            .with_child(text(font, shown, 15.0, if link.is_some() { TEXT } else { DIM }));
        button(row, button_node(), PLAY, PLAY_HOVER)
            .observe(|_: On<Activate>, mut lau: ResMut<Launcher>| send_mod_link(&mut lau))
            .with_child(text(font, tr("Find mods", "Trouver les mods"), 15.0, TEXT));
    });
    let saved = mod_packs::saved_links(code);
    if !saved.is_empty() {
        r.spawn(Node { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, column_gap: px(8), row_gap: px(6), align_items: AlignItems::Center, ..default() }).with_children(|row| {
            row.spawn(text(font, tr("Added from before:", "Deja ajoutes :"), 14.0, DIM));
            for link in saved {
                let (code, target) = (code.to_string(), link.clone());
                button(row, button_node(), ITEM, ITEM_HOVER)
                    .observe(move |_: On<Activate>, mut lau: ResMut<Launcher>| {
                        crate::mod_packs::find(&code, &target);
                        lau.dirty = true;
                    })
                    .with_child(text(font, link, 14.0, TEXT));
            }
            let code = code.to_string();
            button(row, button_node(), ITEM, ITEM_HOVER)
                .observe(move |_: On<Activate>, mut lau: ResMut<Launcher>| {
                    crate::mod_packs::check_updates(&code);
                    lau.dirty = true;
                })
                .with_child(text(font, tr("Check for updates", "Chercher les mises a jour"), 14.0, TEXT));
        });
    }
    if let Some(preview) = mod_packs::preview(code) {
        r.spawn((Node { flex_direction: FlexDirection::Column, row_gap: px(8), padding: UiRect::all(px(12)), flex_shrink: 0.0, border: UiRect::all(px(2)), border_radius: BorderRadius::all(px(6)), ..default() }, BorderColor::all(ITEM_ON), BackgroundColor(SIDEBAR))).with_children(|panel| {
            panel.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(12), align_items: AlignItems::Center, justify_content: JustifyContent::SpaceBetween, ..default() }).with_children(|head| {
                let from = if preview.tag.is_empty() { t!("From {}", "Depuis {}", preview.source) } else { t!("From {} (release {})", "Depuis {} (release {})", preview.source, preview.tag) };
                head.spawn(text(font, from, 15.0, TEXT));
                let code = code.to_string();
                button(head, button_node(), ITEM, ITEM_HOVER)
                    .observe(move |_: On<Activate>, mut lau: ResMut<Launcher>| {
                        crate::mod_packs::close_preview(&code);
                        lau.dirty = true;
                    })
                    .with_child(text(font, tr("Close", "Fermer"), 14.0, TEXT));
            });
            for found in preview.mods {
                let fits = found.refused.is_none();
                panel.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(12), align_items: AlignItems::Center, justify_content: JustifyContent::SpaceBetween, ..default() }).with_children(|row| {
                    row.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(2), flex_shrink: 1.0, ..default() }).with_children(|about| {
                        let by = if found.author.is_empty() { String::new() } else { t!(" by {}", " par {}", found.author) };
                        about.spawn(text(font, format!("{} {}{by}", found.name, found.version), 15.0, if fits { TEXT } else { DIM }));
                        if !found.description.is_empty() {
                            about.spawn(text(font, found.description.clone(), 14.0, DIM));
                        }
                        if let Some(reason) = &found.refused {
                            about.spawn(text(font, reason.clone(), 14.0, GOLD));
                        }
                    });
                    if fits {
                        let label = match &found.installed {
                            None => tr("Install", "Installer").to_string(),
                            Some(old) if *old == found.version => tr("Reinstall", "Reinstaller").to_string(),
                            Some(_) => t!("Update to {}", "Mettre a jour : {}", found.version),
                        };
                        let (code, rom, id) = (code.to_string(), rom.to_path_buf(), found.id.clone());
                        button(row, Node { flex_shrink: 0.0, ..button_node() }, PLAY, PLAY_HOVER)
                            .observe(move |_: On<Activate>, mut lau: ResMut<Launcher>| {
                                crate::mod_packs::install_found(&code, &rom, &id);
                                lau.dirty = true;
                            })
                            .with_child(text(font, label, 14.0, TEXT));
                    }
                });
            }
        });
    }
    for pack in mod_packs::list(code) {
        r.spawn((Node { flex_direction: FlexDirection::Column, row_gap: px(4), padding: UiRect::all(px(10)), flex_shrink: 0.0, border_radius: BorderRadius::all(px(6)), ..default() }, BackgroundColor(SIDEBAR))).with_children(|row| {
            let by = if pack.author.is_empty() { String::new() } else { t!(" by {}", " par {}", pack.author) };
            row.spawn(text(font, format!("{} {}{by}", pack.name, pack.version), 15.0, TEXT));
            if !pack.description.is_empty() {
                row.spawn(text(font, pack.description.clone(), 14.0, DIM));
            }
            if pack.network {
                let saved = mod_packs::host_address(code);
                row.spawn(text(font, t!("To host, leave the address empty and give the other player your IP address (port {}). To join, type the host's address.", "Pour heberger, laissez l'adresse vide et donnez votre adresse IP a l'autre joueur (port {}). Pour rejoindre, tapez l'adresse de l'hote.", mod_packs::NET_PORT), 14.0, DIM));
                let shown = match &host_typed {
                    Some(typed) => format!("{typed}_"),
                    None if saved.is_empty() => tr("Host's address: none (click to type it)", "Adresse de l'hote : aucune (cliquez pour la taper)").to_string(),
                    None => t!("Host's address: {saved}", "Adresse de l'hote : {saved}"),
                };
                let (normal, hover) = if host_typed.is_some() { (ITEM_ON, ITEM_ON) } else { (ITEM, ITEM_HOVER) };
                button(row, Node { align_self: AlignSelf::FlexStart, ..button_node() }, normal, hover)
                    .observe(move |_: On<Activate>, mut lau: ResMut<Launcher>| {
                        if lau.mod_link.is_none() {
                            lau.mod_link = Some(saved.clone());
                            lau.typing_host = true;
                            lau.dirty = true;
                        }
                    })
                    .with_child(text(font, shown, 14.0, TEXT));
            }
            row.spawn(Node { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, column_gap: px(8), row_gap: px(6), ..default() }).with_children(|actions| {
                let (code_on, rom_on, id_on, enabled) = (code.to_string(), rom.to_path_buf(), pack.id.clone(), pack.enabled);
                let (normal, hover) = if enabled { (ITEM_ON, ITEM_ON) } else { (ITEM, ITEM_HOVER) };
                button(actions, button_node(), normal, hover)
                    .observe(move |_: On<Activate>, mut lau: ResMut<Launcher>| {
                        if !crate::mods::running() {
                            crate::mod_packs::set_enabled(&code_on, &rom_on, &id_on, !enabled);
                            lau.dirty = true;
                        }
                    })
                    .with_child(text(font, if enabled { tr("On", "Actif") } else { tr("Off", "Inactif") }, 14.0, TEXT));
                // Une mise à jour trouvée s'installe depuis la liste du lien
                if let Some(version) = mod_packs::update_for(code, &pack.id).filter(|_| !pack.source.is_empty()) {
                    let (code_up, source) = (code.to_string(), pack.source.clone());
                    button(actions, button_node(), PLAY, PLAY_HOVER)
                        .observe(move |_: On<Activate>, mut lau: ResMut<Launcher>| {
                            crate::mod_packs::find(&code_up, &source);
                            lau.dirty = true;
                        })
                        .with_child(text(font, t!("Update to {version}", "Mettre a jour : {version}"), 14.0, TEXT));
                }
                let (code_out, rom_out, id_out) = (code.to_string(), rom.to_path_buf(), pack.id.clone());
                button(actions, button_node(), ITEM, ITEM_HOVER)
                    .observe(move |_: On<Activate>, mut lau: ResMut<Launcher>| {
                        if !crate::mods::running() {
                            crate::mod_packs::remove(&code_out, &rom_out, &id_out);
                            lau.dirty = true;
                        }
                    })
                    .with_child(text(font, tr("Remove", "Retirer"), 14.0, TEXT));
            });
        });
    }
}

/// Refait la page d'un jeu quand le travail de l'outil de mod avance.
fn poll_mods(mut lau: ResMut<Launcher>, mut seen: Local<u64>) {
    let now = crate::mods::revision();
    if now != *seen {
        *seen = now;
        if lau.open.is_some() && lau.tab == Tab::Modding {
            lau.dirty = true;
        }
    }
}

fn modding_tab(rom: &Rom, registry: &GameRegistry, link: Option<String>, typing_host: bool) -> (Top, Rest) {
    let file = rom.info.file.clone();
    let (code, path) = (rom.info.code.clone(), rom.info.path.clone());
    let native = native_game(registry, &code);
    (
        Box::new(move |t, font| {
            button(t, Node { align_self: AlignSelf::FlexStart, ..button_node() }, ITEM, ITEM_HOVER)
                .observe(move |_: On<Activate>, mut browser: ResMut<AssetBrowser>| browser.show_game(&file))
                .with_child(text(font, tr("Open this game's assets (F6)", "Ouvrir les assets de ce jeu (F6)"), 15.0, TEXT));
            button(t, Node { align_self: AlignSelf::FlexStart, ..button_node() }, ITEM, ITEM_HOVER)
                .observe(|_: On<Activate>, mut toggle: MessageWriter<crate::devtools::ToggleCodeWindow>| {
                    toggle.write(crate::devtools::ToggleCodeWindow);
                })
                .with_child(text(font, tr("Open the Code window (F5)", "Ouvrir la fenetre Code (F5)"), 15.0, TEXT));
        }),
        Box::new(move |r, font| {
            r.spawn(text(font, tr("Assets: the game's 3D models and images, with their path in the rom.", "Assets : modeles 3D et images du jeu, avec leur chemin dans la rom."), 15.0, DIM));
            r.spawn(text(font, tr("Code: the game's sources, rebuilt and restarted every time you save.", "Code : les sources du jeu, recompilees et relancees a chaque enregistrement."), 15.0, DIM));
            spawn_mods(r, font, &code, &path, native.as_deref());
            spawn_mod_packs(r, font, &code, &path, link, typing_host);
        }),
    )
}

fn description_tab(lau: &Launcher, rom: &Rom) -> (Top, Rest) {
    let info = rom.info.clone();
    let tags = lau.prefs.tags_of(&info.code).join(", ");
    let (genres, developer, year) = (genres_of(lau, rom), rom.meta.developer.clone(), rom.meta.year.map_or_else(String::new, |year| year.to_string()));
    let publisher_known = publisher_of(rom).to_string();
    (
        Box::new({
            let (title, publisher) = (info.title.clone(), publisher_known);
            move |t, font| {
                t.spawn(text(font, title, 22.0, TEXT));
                if !publisher.is_empty() {
                    t.spawn(text(font, publisher, 16.0, DIM));
                }
            }
        }),
        Box::new(move |r, font| {
            let rows = [
                (tr("Developer", "Developpeur"), developer),
                (tr("Year", "Annee"), year),
                ("Genre", genres),
                ("Tags", tags),
                (tr("Game code", "Code du jeu"), info.code.clone()),
                ("Region", info.region().to_string()),
                (tr("Internal name", "Nom interne"), info.internal_name.clone()),
                (tr("Size", "Taille"), format_size(info.size)),
                (tr("File", "Fichier"), info.file.clone()),
            ];
            for (name, value) in rows.into_iter().filter(|(_, value)| !value.is_empty()) {
                r.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(12), ..default() }).with_children(|row| {
                    row.spawn((text(font, name, 15.0, DIM), Node { width: px(140), ..default() }));
                    row.spawn(text(font, value, 15.0, TEXT));
                });
            }
        }),
    )
}

fn settings_tab(lau: &Launcher, rom: &Rom, registry: &GameRegistry) -> (Top, Rest) {
    let confirming = lau.confirm_remove.is_some();
    let (file, path) = (rom.info.file.clone(), rom.info.path.clone());
    let shown = path.display().to_string();
    let (folders, moved) = (lau.folders.clone(), file.clone());
    let current = library::parent_of(&file).to_string();
    let native_code = native_game(registry, &rom.info.code).map(|_| rom.info.code.clone());
    let path_for_native = rom.info.path.clone();
    let code = rom.info.code.clone();
    let game_tags = lau.prefs.tags_of(&code).to_vec();
    let known_tags = lau.prefs.all_tags();
    let tag_input = lau.tag_input.clone();
    (
        Box::new(move |t, font| {
            if let Some(code) = native_code.clone() {
                use crate::native_game::Preparation;
                t.spawn(text(font, tr("Native game", "Jeu natif"), 15.0, DIM));
                let path = path_for_native.clone();
                let label = match crate::native_game::preparation(&code) {
                    Preparation::Ready => tr("Prepare again", "Preparer a nouveau"),
                    Preparation::Running(_) => tr("Preparing...", "Preparation..."),
                    _ => tr("Prepare", "Preparer"),
                };
                button(t, Node { align_self: AlignSelf::FlexStart, ..button_node() }, ITEM, ITEM_HOVER)
                    .observe(move |_: On<Activate>, mut lau: ResMut<Launcher>| {
                        crate::native_game::forget(&code);
                        crate::native_game::prepare(&code, &path);
                        lau.dirty = true;
                    })
                    .with_child(text(font, label, 15.0, TEXT));
            }
            t.spawn(text(font, "Rom", 15.0, DIM));
            t.spawn(text(font, shown, 14.0, TEXT));
            button(t, Node { align_self: AlignSelf::FlexStart, ..button_node() }, ITEM, ITEM_HOVER).observe(|_: On<Activate>| open_rom_folder()).with_child(text(font, tr("Open the roms folder", "Ouvrir le dossier des roms"), 15.0, TEXT));
            let (normal, hover, label) = if confirming { (DANGER, DANGER_HOVER, tr("Confirm? (deletes the file)", "Confirmer ? (efface le fichier)")) } else { (ITEM, ITEM_HOVER, tr("Remove this game", "Retirer ce jeu")) };
            button(t, Node { align_self: AlignSelf::FlexStart, ..button_node() }, normal, hover)
                .observe(move |_: On<Activate>, mut lau: ResMut<Launcher>, time: Res<Time<Real>>| {
                    if lau.confirm_remove.is_none() {
                        lau.confirm_remove = Some(time.elapsed_secs_f64());
                        lau.dirty = true;
                        return;
                    }
                    lau.confirm_remove = None;
                    lau.dirty = true;
                    remove_rom(&mut lau, &file, &path, &time);
                })
                .with_child(text(font, label, 15.0, TEXT));
        }),
        Box::new(move |r, font| {
            spawn_tags(r, font, &code, &game_tags, &known_tags, tag_input);
            if folders.is_empty() {
                return;
            }
            r.spawn(text(font, tr("Folder of this game", "Dossier de ce jeu"), 15.0, DIM));
            r.spawn(Node { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, column_gap: px(6), row_gap: px(6), ..default() }).with_children(|row| {
                for folder in std::iter::once(String::new()).chain(folders) {
                    let (normal, hover) = if folder == current { (ITEM_ON, ITEM_ON) } else { (ITEM, ITEM_HOVER) };
                    let name = if folder.is_empty() { tr("(no folder)", "(aucun dossier)").to_string() } else { folder.clone() };
                    let file = moved.clone();
                    button(row, button_node(), normal, hover)
                        .observe(move |_: On<Activate>, mut lau: ResMut<Launcher>, mut images: ResMut<Assets<Image>>, time: Res<Time<Real>>| move_game(&mut lau, &mut images, &time, &file, &folder))
                        .with_child(text(font, name, 14.0, TEXT));
                }
            });
        }),
    )
}

/// Les tags du jeu : un clic met ou retire un tag déjà connu, « Nouveau tag » en crée un.
fn spawn_tags(r: &mut ChildSpawnerCommands, font: &Fonts, code: &str, game_tags: &[String], known_tags: &[String], input: Option<String>) {
    r.spawn(text(font, tr("Tags (click to add or remove)", "Tags (clic pour mettre ou retirer)"), 15.0, DIM));
    r.spawn(Node { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, align_items: AlignItems::Center, column_gap: px(6), row_gap: px(6), ..default() }).with_children(|row| {
        for tag in known_tags {
            let on = game_tags.contains(tag);
            let (normal, hover) = if on { (ITEM_ON, ITEM_ON) } else { (ITEM, ITEM_HOVER) };
            let (code, name) = (code.to_string(), tag.clone());
            button(row, button_node(), normal, hover)
                .observe(move |_: On<Activate>, mut lau: ResMut<Launcher>| {
                    if on {
                        lau.prefs.remove_tag(&code, &name);
                    } else if lau.prefs.add_tag(&code, &name).is_err() {
                        return;
                    }
                    lau.prefs.store();
                    lau.dirty = true;
                })
                .with_child(text(font, tag.clone(), 14.0, TEXT));
        }
        match input {
            Some(tag) => {
                row.spawn((Node { padding: UiRect::axes(px(12), px(8)), min_width: px(200), border: UiRect::all(px(2)), border_radius: BorderRadius::all(px(6)), ..default() }, BorderColor::all(ITEM_ON), BackgroundColor(PANEL))).with_child(text(font, format!("{tag}_"), 14.0, TEXT));
                button(row, button_node(), ITEM_ON, ITEM_HOVER)
                    .observe(|_: On<Activate>, mut lau: ResMut<Launcher>, time: Res<Time<Real>>| add_typed_tag(&mut lau, &time))
                    .with_child(text(font, tr("Add", "Ajouter"), 14.0, TEXT));
                row.spawn(text(font, tr("Enter = add, Esc = cancel", "Entree = ajouter, Echap = annuler"), 13.0, DIM));
            }
            None => {
                button(row, button_node(), ITEM, ITEM_HOVER)
                    .observe(|_: On<Activate>, mut lau: ResMut<Launcher>| {
                        lau.tag_input = Some(String::new());
                        lau.dirty = true;
                    })
                    .with_child(text(font, tr("New tag", "Nouveau tag"), 14.0, TEXT));
            }
        }
    });
}

/// Déplace la rom ; les favoris suivent. Rend son nouveau chemin relatif, ou dit pourquoi elle reste.
fn relocate_rom(lau: &mut Launcher, time: &Time<Real>, file: &str, folder: &str) -> Option<String> {
    match library::move_rom(&library::rom_dir(), file, folder) {
        Ok(moved) => {
            for favorite in lau.prefs.favorites.iter_mut().filter(|f| *f == file) {
                favorite.clone_from(&moved);
            }
            lau.prefs.store();
            Some(moved)
        }
        Err(e) => {
            say(lau, time, e);
            None
        }
    }
}

/// Range un jeu : il reste ouvert et favori, et la vue des jeux le retrouve dans son nouveau dossier.
fn move_game(lau: &mut Launcher, images: &mut Assets<Image>, time: &Time<Real>, file: &str, folder: &str) {
    if let Some(moved) = relocate_rom(lau, time, file, folder) {
        lau.open = Some(moved);
        lau.folder = folder.to_string();
        lau.scroll_reset = true;
        reload_roms(lau, images);
    }
}

/// Un jeu glissé sur un dossier : on reste où l'on est, pour en ranger d'autres.
fn drop_game(lau: &mut Launcher, images: &mut Assets<Image>, time: &Time<Real>, file: &str, folder: &str) {
    if library::parent_of(file) == folder {
        return;
    }
    if relocate_rom(lau, time, file, folder).is_some() {
        let shown = if folder.is_empty() { tr("the main folder", "la racine") } else { folder };
        say(lau, time, t!("Moved to {shown}", "Range dans {shown}"));
        reload_roms(lau, images);
    }
}

fn remove_rom(lau: &mut Launcher, file: &str, path: &Path, time: &Time<Real>) {
    match std::fs::remove_file(path) {
        Ok(()) => {
            lau.prefs.favorites.retain(|f| f != file);
            lau.prefs.store();
            lau.open = None;
            lau.roms.retain(|r| r.info.file != file);
            lau.stamp.retain(|(name, _)| name != file);
            activity::record(What::RomRemoved { file: file.to_string() });
            say(lau, time, t!("{file} removed", "{file} retire"));
        }
        Err(e) => say(lau, time, format!("{file} : {e}")),
    }
}

fn open_rom_folder() {
    let dir = library::rom_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        warn!("dossier des roms impossible a creer : {}: {e}", dir.display());
        return;
    }
    open_folder_in_system(&dir);
}

/// Montre un dossier dans l'explorateur du système.
fn open_folder_in_system(dir: &Path) {
    let program = if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if let Err(e) = std::process::Command::new(program).arg(dir).spawn() {
        warn!("impossible d'ouvrir {} : {e}", dir.display());
    }
}

// ─────────────────────────────────────────────────────────────────────────
//  Page Options > Launcher
// ─────────────────────────────────────────────────────────────────────────

fn page_label(value: impl Into<String>, size: f32, color: Color) -> (Text, TextFont, TextColor) {
    text(&Fonts::default(), value, size, color)
}

/// Contenu de la page (appelé par `options`).
pub fn spawn_options_page(c: &mut ChildSpawnerCommands) {
    c.spawn(page_label("Launcher", 24.0, TEXT));
    c.spawn(page_label(tr("Language", "Langue"), 15.0, DIM));
    c.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(6), ..default() }).with_children(|row| {
        for choice in Lang::ALL {
            row.spawn((Button, Hovered::default(), button_node(), BackgroundColor(ITEM), LangChoice(choice)))
                .observe(move |_: On<Activate>, mut language: ResMut<Language>| i18n::set_language(&mut language, choice))
                .with_child(page_label(choice.name(), 14.0, TEXT));
        }
    });
    c.spawn(page_label(tr("Game view", "Vue des jeux"), 15.0, DIM));
    c.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(6), ..default() }).with_children(|row| {
        for (view, name) in view_names() {
            view_button(row, &Fonts::default(), view, name);
        }
    });
    c.spawn(page_label(tr("Game covers", "Jaquettes des jeux"), 15.0, DIM));
    c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(6), ..default() }).with_children(|row| {
        for (on, name) in [(false, tr("No", "Non")), (true, tr("Yes", "Oui"))] {
            row.spawn((Button, Hovered::default(), button_node(), BackgroundColor(ITEM), CoverChoice(on)))
                .observe(set_covers(on))
                .with_child(page_label(name, 14.0, TEXT));
        }
        row.spawn((page_label("", 13.0, DIM), CoverStatus));
    });
    c.spawn(page_label(tr("Downloaded from GameTDB for the games you installed, with their developer, year and genre.", "Telechargees depuis GameTDB pour les jeux installes, avec leur developpeur, leur annee et leur genre."), 13.0, DIM));
    c.spawn(page_label(tr("Roms folder", "Dossier des roms"), 15.0, DIM));
    c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(12), ..default() }).with_children(|row| {
        button(row, Node { flex_shrink: 0.0, ..button_node() }, ITEM, ITEM_HOVER).observe(|_: On<Activate>| open_rom_folder()).with_child(page_label(tr("Open the folder", "Ouvrir le dossier"), 15.0, TEXT));
        row.spawn(page_label(library::rom_dir().display().to_string(), 13.0, DIM));
    });
    c.spawn(page_label(tr("Native games", "Jeux natifs"), 15.0, DIM));
    button(c, Node { align_self: AlignSelf::FlexStart, ..button_node() }, ITEM, ITEM_HOVER)
        .observe(|_: On<Activate>, mut lau: ResMut<Launcher>| lau.prepare_all = true)
        .with_child(page_label(tr("Prepare all games, experimental ones too", "Preparer tous les jeux, meme experimentaux"), 14.0, TEXT));
    c.spawn(page_label(tr("One game at a time, about a minute each. Games that work are always prepared.", "Un jeu a la fois, environ une minute chacun. Les jeux qui marchent sont toujours prepares."), 13.0, DIM));
    c.spawn(page_label(tr("The app's mods", "Les mods de l'app"), 15.0, DIM));
    c.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(6), ..default() }).with_children(|row| {
        for (on, name) in [(false, tr("No", "Non")), (true, tr("Yes", "Oui"))] {
            row.spawn((Button, Hovered::default(), button_node(), BackgroundColor(ITEM), BaseModsChoice(on)))
                .observe(move |_: On<Activate>, mut lau: ResMut<Launcher>| {
                    crate::mod_packs::set_base_enabled(on);
                    lau.dirty = true;
                })
                .with_child(page_label(name, 14.0, TEXT));
        }
    });
    c.spawn(page_label(t!("Show the mods of {} in each game's Modding tab.", "Montrer les mods de {} dans l'onglet Moding de chaque jeu.", crate::mod_packs::BASE_REPOSITORY), 13.0, DIM));
    c.spawn(page_label(tr("Updates", "Mises a jour"), 15.0, DIM));
    button(c, Node { align_self: AlignSelf::FlexStart, ..button_node() }, ITEM, ITEM_HOVER).observe(start_update).with_child(page_label(tr("Check and install updates", "Chercher et installer les mises a jour"), 14.0, TEXT));
    c.spawn(page_label("Versions", 15.0, DIM));
    c.spawn((page_label("", 14.0, TEXT), VersionText));
    button(c, Node { align_self: AlignSelf::FlexStart, ..button_node() }, ITEM, ITEM_HOVER).observe(open_versions).with_child(page_label(tr("Choose a version and update (closes the studio)", "Choisir une version et mettre a jour (ferme le studio)"), 15.0, TEXT));
}

/// Le launcher de mise à jour remplace l'exécutable du studio : il faut donc le fermer.
fn open_versions(_: On<Activate>, mut exit: MessageWriter<AppExit>, mut toasts: MessageWriter<crate::link::ShowToast>) {
    let exe = std::env::current_exe().unwrap_or_default();
    let updater = exe.with_file_name(format!("nds-game-studio-launcher{}", std::env::consts::EXE_SUFFIX));
    match std::process::Command::new(&updater).arg("--versions").spawn() {
        Ok(_) => {
            exit.write(AppExit::Success);
        }
        Err(e) => {
            toasts.write(crate::link::ShowToast(format!("{} : {e}", updater.display())));
        }
    }
}

fn version_text(footer: Option<Res<StudioFooter>>, mut texts: Query<&mut Text, With<VersionText>>) {
    let wanted = footer.map(|f| f.0.clone()).unwrap_or_default();
    for mut text in &mut texts {
        if text.0 != wanted {
            text.0 = wanted.clone();
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
//  Ce qui tourne toujours
// ─────────────────────────────────────────────────────────────────────────

fn wheel(mut events: MessageReader<MouseWheel>, mut areas: Query<(&Hovered, &ComputedNode, &mut ScrollPosition), With<Scrollable>>) {
    for event in events.read() {
        let delta = match event.unit {
            MouseScrollUnit::Line => event.y * WHEEL_LINE,
            MouseScrollUnit::Pixel => event.y,
        };
        for (hovered, node, mut position) in &mut areas {
            if hovered.get() {
                let max = ((node.content_size().y - node.size().y) * node.inverse_scale_factor()).max(0.0);
                position.0.y = (position.0.y - delta).clamp(0.0, max);
            }
        }
    }
}

fn looks(mut buttons: Query<(&Look, &Hovered, Has<Pressed>, &mut BackgroundColor)>) {
    for (look, hovered, pressed, mut color) in &mut buttons {
        let wanted = if hovered.get() || pressed { look.hover } else { look.normal };
        if color.0 != wanted {
            color.0 = wanted;
        }
    }
}

#[derive(Component)]
struct BaseModsChoice(bool);

fn base_mods_looks(mut buttons: Query<(&BaseModsChoice, &Hovered, &mut BackgroundColor)>) {
    let on = crate::mod_packs::base_enabled();
    for (choice, hovered, mut color) in &mut buttons {
        let wanted = if choice.0 == on { ITEM_ON } else if hovered.get() { ITEM_HOVER } else { ITEM };
        if color.0 != wanted {
            color.0 = wanted;
        }
    }
}

fn lang_choice_looks(language: Res<Language>, mut buttons: Query<(&LangChoice, &Hovered, &mut BackgroundColor)>) {
    for (choice, hovered, mut color) in &mut buttons {
        let wanted = if language.0 == choice.0 { ITEM_ON } else if hovered.get() { ITEM_HOVER } else { ITEM };
        if color.0 != wanted {
            color.0 = wanted;
        }
    }
}

/// La zone du launcher est refaite dans la nouvelle langue.
fn redo_on_language(language: Res<Language>, mut lau: ResMut<Launcher>) {
    if language.is_changed() && !language.is_added() {
        lau.dirty = true;
    }
}

fn view_choice_looks(lau: Res<Launcher>, mut buttons: Query<(&ViewChoice, &Hovered, &mut BackgroundColor)>) {
    for (choice, hovered, mut color) in &mut buttons {
        let wanted = if lau.prefs.view == choice.0 { ITEM_ON } else if hovered.get() { ITEM_HOVER } else { ITEM };
        if color.0 != wanted {
            color.0 = wanted;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    fn imported(already: bool) -> Result<Imported, String> {
        Ok(Imported { title: "Jeu".into(), already })
    }

    #[test]
    fn tally_is_silent_for_one_rom() {
        let mut tally = ImportTally::default();
        assert_eq!(tally.summary(), None);
        tally.record(&Err("photo.nds: not a DS rom".into()));
        assert_eq!(tally.summary(), None);
    }

    #[test]
    fn tally_keeps_the_first_error() {
        let mut tally = ImportTally::default();
        for result in [imported(false), Err("a.nds: not a DS rom".to_string()), imported(false), imported(true), Err("b.nds: not a DS rom".to_string())] {
            tally.record(&result);
        }
        assert_eq!(tally.summary().unwrap(), "2 added, 1 already in the launcher, 2 failed: a.nds: not a DS rom");
    }

    #[test]
    fn cancelling_the_picker_does_nothing() {
        let mut world = World::new();
        world.init_resource::<Launcher>();
        world.init_resource::<Time<Real>>();
        {
            let mut lau = world.resource_mut::<Launcher>();
            lau.picking = true;
            *lau.picked.lock().unwrap() = Some(Vec::new());
        }
        world.run_system_once(poll_picker).unwrap();
        let lau = world.resource::<Launcher>();
        assert!(!lau.picking);
        assert_eq!(lau.copying, 0);
        assert_eq!(lau.status, "");
    }
}
