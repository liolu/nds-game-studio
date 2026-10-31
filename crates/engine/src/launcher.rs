//! Le launcher : ce qu'on voit quand aucun jeu ne tourne, à la place des deux écrans DS.
//!
//! - à gauche : le logo, le bouton de la vue des jeux (4 carrés) et les jeux favoris ;
//! - la vue des jeux, en grille ou en liste, avec les dossiers de jeux (les sous-dossiers de `rom`) : clic pour
//!   entrer, Retour ou Echap pour remonter, « Nouveau dossier » ; un jeu se range depuis ses Parametres ;
//! - la page d'un jeu : son image, son nom, le bouton favori et quatre boutons qui changent la zone
//!   du bas (Jouer, Moding, Description, Parametres) ;
//! - les textes suivent la langue (`i18n`) ; la zone est refaite quand elle change ;
//! - le bouton Options, toujours en bas à droite, même quand on fait défiler.
//!
//! Sans rom, aucun jeu n'est montré : seulement le message qui dit de déposer une rom dans la
//! fenêtre. Elle est copiée dans le dossier `rom` à côté du launcher (voir `library`).
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
use bevy::picking::hover::{HoverMap, Hovered};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::ui::{Pressed, ScrollPosition};
use bevy::ui_widgets::{Activate, Button};
use bevy::window::FileDragAndDrop;

use crate::asset_browser::AssetBrowser;
use crate::i18n::{self, t, tr, Lang, Language, Tr};
use crate::library::{self, Imported, LibraryPrefs, LibraryView, RomInfo};
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

const SIDEBAR_W: f32 = 170.0;
const COVER: f32 = 300.0;
const MAIN_PAD: f32 = 24.0;
/// Hauteur au-dessus de la zone du bas ; le reste de l'image déborde dans la zone.
const HEADER_H: f32 = 126.0;
const STATUS_SECONDS: f64 = 5.0;
const POLL_EVERY: f64 = 2.0;
const CONFIRM_SECONDS: f64 = 3.0;
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
}

#[derive(Resource, Default)]
struct Launcher {
    roms: Vec<Rom>,
    cover_job: Option<Arc<Mutex<crate::covers::Progress>>>,
    /// Codes déjà demandés depuis le démarrage : un jeu absent de GameTDB n'est pas redemandé à chaque image.
    cover_asked: std::collections::HashSet<String>,
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
struct VersionText;

#[derive(Component)]
struct CoverChoice(bool);

#[derive(Component)]
struct CoverStatus;

#[derive(Component)]
struct LangChoice(Lang);

pub struct LauncherPlugin;

impl Plugin for LauncherPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Launcher>()
            .add_systems(Startup, setup)
            .add_systems(OnEnter(StudioState::Menu), spawn_launcher.before(crate::studio::auto_start))
            .add_systems(Update, (take_drops, poll_copies, poll_disk, back_key, type_folder_name, expire_confirm, log_clicks, rebuild, show_status).chain().run_if(in_state(StudioState::Menu)))
            .add_systems(Update, (wheel, looks, view_choice_looks, lang_choice_looks, version_text, redo_on_language))
            .add_systems(Update, (poll_covers, cover_choice_looks));
    }
}

fn setup(mut lau: ResMut<Launcher>, mut images: ResMut<Assets<Image>>, mut fonts: ResMut<Assets<Font>>) {
    lau.prefs = LibraryPrefs::load();
    lau.star = images.add(star_image());
    lau.font = Fonts { latin: system_font(&mut fonts, &["segoeui.ttf"]), japanese: system_font(&mut fonts, &["YuGothR.ttc", "meiryo.ttc", "msgothic.ttc"]) };
}

/// Polices de Windows (accents et japonais des titres de jeux) ; ailleurs, celle de Bevy.
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

fn system_font(fonts: &mut Assets<Font>, names: &[&str]) -> Option<Handle<Font>> {
    let windir = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let bytes = names.iter().find_map(|name| std::fs::read(windir.join("Fonts").join(name)).ok())?;
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
    let mut content = None;
    commands
        .spawn((
            Node { position_type: PositionType::Absolute, top: px(TITLE_H), left: px(0), right: px(0), bottom: px(0), flex_direction: FlexDirection::Row, ..default() },
            BackgroundColor(BG),
            UiTargetCamera(camera),
            DespawnOnExit(StudioState::Menu),
        ))
        .with_children(|root| {
            content = Some(root.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Row, ..default() }).id());
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
            Rom { info, icon, art, tint: Color::srgb(r * 0.45, g * 0.45, b * 0.45) }
        })
        .collect();
    lau.dirty = true;
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
        lau.cover_job = None;
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
//  Ajouter une rom (glisser-déposer), suivre le dossier
// ─────────────────────────────────────────────────────────────────────────

fn take_drops(mut drops: MessageReader<FileDragAndDrop>, mut lau: ResMut<Launcher>, time: Res<Time<Real>>, mut overlays: Query<&mut Node, With<DropOverlay>>) {
    for drop in drops.read() {
        let show = match drop {
            FileDragAndDrop::HoveredFile { .. } => true,
            FileDragAndDrop::HoveredFileCanceled { .. } => false,
            FileDragAndDrop::DroppedFile { path_buf, .. } => {
                start_copy(&mut lau, path_buf.clone());
                say(&mut lau, &time, tr("Copying the rom...", "Copie de la rom..."));
                false
            }
        };
        for mut node in &mut overlays {
            node.display = if show { Display::Flex } else { Display::None };
        }
    }
}

fn start_copy(lau: &mut Launcher, source: PathBuf) {
    lau.copying += 1;
    let results = lau.imported.clone();
    let target = library::folder_path(&library::rom_dir(), &lau.folder);
    std::thread::spawn(move || {
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
        let message = match result {
            Ok(Imported { title, already: true }) => t!("{title} is already in the launcher", "{title} est deja dans le launcher"),
            Ok(Imported { title, already: false }) => t!("{title} added", "{title} ajoute"),
            Err(e) => e,
        };
        say(&mut lau, &time, message);
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
    if !focused || !keys.just_pressed(KeyCode::Escape) || lau.naming.is_some() {
        return;
    }
    if lau.open.is_some() {
        lau.open = None;
        lau.dirty = true;
    } else if !lau.folder.is_empty() {
        let parent = library::parent_of(&lau.folder).to_string();
        enter_folder(&mut lau, parent);
    }
}

fn enter_folder(lau: &mut Launcher, folder: String) {
    lau.folder = folder;
    lau.naming = None;
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
    lau.dirty = true;
}

fn open_game(file: String) -> impl FnMut(On<Activate>, ResMut<Launcher>) {
    move |_, mut lau| {
        info!("jeu ouvert : {file}");
        lau.open = Some(file.clone());
        lau.tab = Tab::Play;
        lau.confirm_remove = None;
        lau.dirty = true;
    }
}

fn spawn_empty(main: &mut ChildSpawnerCommands, lau: &Launcher) {
    let font = &lau.font;
    main.spawn(Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Column, justify_content: JustifyContent::Center, align_items: AlignItems::Center, row_gap: px(14), padding: UiRect::all(px(40)), ..default() }).with_children(|empty| {
        empty.spawn(text(font, tr("No games", "Aucun jeu"), 32.0, TEXT));
        empty.spawn(text(font, tr("Drag a DS rom (.nds) into this window to add it to the launcher.", "Glissez une rom DS (.nds) dans cette fenetre pour l'ajouter au launcher."), 18.0, TEXT));
        empty.spawn(text(font, t!("It will be copied to {}", "Elle sera copiee dans {}", library::rom_dir().display()), 14.0, DIM));
    });
}

fn spawn_library(main: &mut ChildSpawnerCommands, lau: &Launcher) {
    let font = &lau.font;
    let count = roms_here(lau).count();
    let games = if count == 1 { tr("1 game", "1 jeu").to_string() } else { t!("{count} games", "{count} jeux") };
    let empty_folder = count == 0 && !lau.folder.is_empty() && folders_here(lau).next().is_none();
    main.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(8), padding: UiRect::new(px(MAIN_PAD), px(MAIN_PAD), px(16), px(8)), ..default() }).with_children(|bar| {
        if !lau.folder.is_empty() {
            button(bar, button_node(), ITEM, ITEM_HOVER)
                .observe(|_: On<Activate>, mut lau: ResMut<Launcher>| {
                    let parent = library::parent_of(&lau.folder).to_string();
                    enter_folder(&mut lau, parent);
                })
                .with_child(text(font, tr("<- Back", "<- Retour"), 14.0, TEXT));
        }
        let title = if lau.folder.is_empty() { games } else { format!("{}   -   {games}", lau.folder) };
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
    let mut scroll = main.spawn((
        Node { flex_grow: 1.0, flex_basis: px(0), min_height: px(0), width: percent(100), overflow: Overflow::scroll_y(), ..default() },
        ScrollPosition(Vec2::new(0.0, lau.grid_scroll)),
        Hovered::default(),
        Scrollable,
        GridScroll,
    ));
    scroll.with_children(|area| match lau.prefs.view {
        LibraryView::Grid => spawn_grid(area, lau),
        LibraryView::List => spawn_list(area, lau),
    });
}

/// Les jeux du dossier montré.
fn roms_here(lau: &Launcher) -> impl Iterator<Item = &Rom> {
    lau.roms.iter().filter(|r| library::parent_of(&r.info.file) == lau.folder)
}

/// Les dossiers du dossier montré.
fn folders_here(lau: &Launcher) -> impl Iterator<Item = &String> {
    lau.folders.iter().filter(|f| library::parent_of(f) == lau.folder)
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

fn spawn_grid(area: &mut ChildSpawnerCommands, lau: &Launcher) {
    let font = &lau.font;
    area.spawn(Node { width: percent(100), height: auto(), flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, align_content: AlignContent::FlexStart, column_gap: px(20), row_gap: px(20), padding: UiRect::new(px(MAIN_PAD), px(MAIN_PAD), px(6), px(84)), ..default() }).with_children(|grid| {
        for folder in folders_here(lau) {
            button(grid, Node { width: px(190), height: px(268), flex_shrink: 0.0, flex_direction: FlexDirection::Column, align_items: AlignItems::Center, padding: UiRect::all(px(10)), row_gap: px(8), border_radius: BorderRadius::all(px(10)), overflow: Overflow::clip(), ..default() }, PANEL, ITEM_HOVER)
                .observe(open_folder(folder.clone()))
                .with_children(|card| {
                    folder_icon(card, 170.0);
                    card.spawn((text(font, library::last_name(folder), 14.0, TEXT), Node { width: percent(100), ..default() }));
                    card.spawn((text(font, games_in_folder(lau, folder), 12.0, DIM), Node { width: percent(100), ..default() }));
                });
        }
        for rom in roms_here(lau) {
            button(grid, Node { width: px(190), height: px(268), flex_shrink: 0.0, flex_direction: FlexDirection::Column, align_items: AlignItems::Center, padding: UiRect::all(px(10)), row_gap: px(8), border_radius: BorderRadius::all(px(10)), overflow: Overflow::clip(), ..default() }, PANEL, ITEM_HOVER)
                .observe(open_game(rom.info.file.clone()))
                .with_children(|card| {
                    cover(card, rom, 170.0);
                    if lau.prefs.is_favorite(&rom.info.file) {
                        card.spawn(Node { position_type: PositionType::Absolute, top: px(16), right: px(16), ..default() }).with_children(|b| star(b, lau, &rom.info.file, 22.0));
                    }
                    card.spawn((text(font, rom.info.title.clone(), 14.0, TEXT), Node { width: percent(100), ..default() }));
                    card.spawn((text(font, rom.info.publisher.clone(), 12.0, DIM), Node { width: percent(100), ..default() }));
                });
        }
    });
}

fn spawn_list(area: &mut ChildSpawnerCommands, lau: &Launcher) {
    let font = &lau.font;
    let columns: [(&str, f32); 4] = [(tr("Publisher", "Editeur"), 210.0), ("Code", 70.0), ("Region", 90.0), (tr("Size", "Taille"), 80.0)];
    area.spawn(Node { width: percent(100), height: auto(), flex_direction: FlexDirection::Column, row_gap: px(4), padding: UiRect::new(px(MAIN_PAD), px(MAIN_PAD), px(6), px(84)), ..default() }).with_children(|list| {
        list.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(14), padding: UiRect::axes(px(8), px(4)), ..default() }).with_children(|head| {
            head.spawn(Node { width: px(44), ..default() });
            head.spawn((text(font, tr("Game", "Jeu"), 13.0, DIM), Node { flex_grow: 1.0, flex_basis: px(0), ..default() }));
            for (name, width) in columns {
                head.spawn((text(font, name, 13.0, DIM), Node { width: px(width), ..default() }));
            }
            head.spawn(Node { width: px(22), ..default() });
        });
        for folder in folders_here(lau) {
            button(list, Node { flex_direction: FlexDirection::Row, flex_shrink: 0.0, align_items: AlignItems::Center, column_gap: px(14), padding: UiRect::axes(px(8), px(6)), border_radius: BorderRadius::all(px(6)), ..default() }, PANEL, ITEM_HOVER)
                .observe(open_folder(folder.clone()))
                .with_children(|row| {
                    folder_icon(row, 44.0);
                    row.spawn((text(font, library::last_name(folder), 15.0, TEXT), Node { flex_grow: 1.0, flex_basis: px(0), min_width: px(0), ..default() }));
                    row.spawn(text(font, games_in_folder(lau, folder), 14.0, DIM));
                });
        }
        for rom in roms_here(lau) {
            let info = &rom.info;
            button(list, Node { flex_direction: FlexDirection::Row, flex_shrink: 0.0, align_items: AlignItems::Center, column_gap: px(14), padding: UiRect::axes(px(8), px(6)), border_radius: BorderRadius::all(px(6)), ..default() }, PANEL, ITEM_HOVER)
                .observe(open_game(info.file.clone()))
                .with_children(|row| {
                    cover(row, rom, 44.0);
                    row.spawn((text(font, info.title.clone(), 15.0, TEXT), Node { flex_grow: 1.0, flex_basis: px(0), min_width: px(0), ..default() }));
                    let values = [info.publisher.clone(), info.code.clone(), info.region().to_string(), format_size(info.size)];
                    for ((_, width), value) in columns.iter().zip(values) {
                        row.spawn((text(font, value, 14.0, DIM), Node { width: px(*width), ..default() }));
                    }
                    star(row, lau, &info.file, 22.0);
                });
        }
    });
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
            Tab::Modding => modding_tab(rom),
            Tab::Description => description_tab(rom),
            Tab::Settings => settings_tab(lau, rom),
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

fn play_tab(rom: &Rom, registry: &GameRegistry) -> (Top, Rest) {
    let native = native_game(registry, &rom.info.code);
    let experimental = native.is_some() && crate::native_game::is_experimental(&rom.info.code);
    let path = rom.info.path.clone();
    let summary = format!("{}  -  {}  -  {}", rom.info.code, rom.info.region(), format_size(rom.info.size));
    (
        Box::new(move |t, font| {
            let ready = native.is_some();
            let (normal, hover) = if ready { (PLAY, PLAY_HOVER) } else { (ITEM, ITEM) };
            let mut play = button(t, Node { align_self: AlignSelf::FlexStart, padding: UiRect::axes(px(48), px(14)), ..button_node() }, normal, hover);
            if let Some(id) = native {
                play.observe(move |_: On<Activate>, mut commands: Commands| {
                    commands.insert_resource(ActiveRom(path.clone()));
                    commands.insert_resource(ActiveGame(Some(id.clone())));
                    commands.set_state(StudioState::Playing);
                });
            }
            play.with_child(text(font, tr("Play", "Jouer"), 22.0, if ready { TEXT } else { DIM }));
            if !ready {
                t.spawn(text(font, tr("Not playable yet: this game has no native module (decompilation + runtime).", "Pas encore jouable : ce jeu n'a pas de module natif (decompilation + runtime)."), 15.0, DIM));
            }
            if experimental {
                t.spawn(text(font, tr("Experimental: this game may crash or show nothing yet.", "Experimental : ce jeu peut planter ou ne rien afficher pour l'instant."), 15.0, DIM));
            }
            t.spawn(text(font, summary, 14.0, DIM));
        }),
        Box::new(|_, _| {}),
    )
}

fn modding_tab(rom: &Rom) -> (Top, Rest) {
    let file = rom.info.file.clone();
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
        Box::new(|r, font| {
            r.spawn(text(font, tr("Assets: the game's 3D models and images, with their path in the rom.", "Assets : modeles 3D et images du jeu, avec leur chemin dans la rom."), 15.0, DIM));
            r.spawn(text(font, tr("Code: the game's sources, rebuilt and restarted every time you save.", "Code : les sources du jeu, recompilees et relancees a chaque enregistrement."), 15.0, DIM));
        }),
    )
}

fn description_tab(rom: &Rom) -> (Top, Rest) {
    let info = rom.info.clone();
    (
        Box::new({
            let (title, publisher) = (info.title.clone(), info.publisher.clone());
            move |t, font| {
                t.spawn(text(font, title, 22.0, TEXT));
                if !publisher.is_empty() {
                    t.spawn(text(font, publisher, 16.0, DIM));
                }
            }
        }),
        Box::new(move |r, font| {
            let rows = [
                (tr("Game code", "Code du jeu"), info.code.clone()),
                ("Region", info.region().to_string()),
                (tr("Internal name", "Nom interne"), info.internal_name.clone()),
                (tr("Size", "Taille"), format_size(info.size)),
                (tr("File", "Fichier"), info.file.clone()),
            ];
            for (name, value) in rows {
                r.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(12), ..default() }).with_children(|row| {
                    row.spawn((text(font, name, 15.0, DIM), Node { width: px(140), ..default() }));
                    row.spawn(text(font, value, 15.0, TEXT));
                });
            }
        }),
    )
}

fn settings_tab(lau: &Launcher, rom: &Rom) -> (Top, Rest) {
    let confirming = lau.confirm_remove.is_some();
    let (file, path) = (rom.info.file.clone(), rom.info.path.clone());
    let shown = path.display().to_string();
    let (folders, moved) = (lau.folders.clone(), file.clone());
    let current = library::parent_of(&file).to_string();
    (
        Box::new(move |t, font| {
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

/// Range un jeu : il reste ouvert et favori, et la vue des jeux le retrouve dans son nouveau dossier.
fn move_game(lau: &mut Launcher, images: &mut Assets<Image>, time: &Time<Real>, file: &str, folder: &str) {
    match library::move_rom(&library::rom_dir(), file, folder) {
        Ok(moved) => {
            for favorite in lau.prefs.favorites.iter_mut().filter(|f| *f == file) {
                *favorite = moved.clone();
            }
            lau.prefs.store();
            lau.open = Some(moved);
            lau.folder = folder.to_string();
            lau.scroll_reset = true;
            reload_roms(lau, images);
        }
        Err(e) => say(lau, time, e),
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
    let program = if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if let Err(e) = std::process::Command::new(program).arg(&dir).spawn() {
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
    c.spawn(page_label(tr("Downloaded from GameTDB for the games you installed.", "Telechargees depuis GameTDB pour les jeux installes."), 13.0, DIM));
    c.spawn(page_label(tr("Roms folder", "Dossier des roms"), 15.0, DIM));
    c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(12), ..default() }).with_children(|row| {
        button(row, Node { flex_shrink: 0.0, ..button_node() }, ITEM, ITEM_HOVER).observe(|_: On<Activate>| open_rom_folder()).with_child(page_label(tr("Open the folder", "Ouvrir le dossier"), 15.0, TEXT));
        row.spawn(page_label(library::rom_dir().display().to_string(), 13.0, DIM));
    });
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
