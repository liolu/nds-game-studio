//! Les deux écrans DS.
//!
//! Comme sur la console, il y a deux moteurs graphiques : `Engine::Main` et `Engine::Sub`. Chacun
//! dessine dans sa propre image (le « canvas ») avec sa caméra 2D, et une caméra 3D en plus si la
//! spécification du jeu le demande. Échanger les écrans (`DsScreens::main_on`) ne change que
//! l'écran physique qui montre chaque canvas, comme le bit POWCNT de la DS.
//!
//! Fenêtres : la fenêtre principale montre les deux écrans l'un au-dessus de l'autre quand ils
//! sont fusionnés (`ScreenLayout::merged`), sinon seulement l'écran du haut ; l'écran du bas a
//! alors sa propre fenêtre (créée et fermée par `link`). Une seule fenêtre quand c'est fusionné :
//! l'aimantation de Windows, le redimensionnement par n'importe quel bord et la réduction marchent
//! comme pour n'importe quelle fenêtre.
//!
//! Rendu natif en qualité maximale : chaque canvas a la taille en pixels de la zone qu'il occupe
//! dans sa fenêtre, avec MSAA x4. Les jeux comptent toujours en pixels DS : 256x192 au centre, plus
//! large en 16:9 / 16:10 / remplir (les jeux voient plus de choses sur les côtés).
//! `RenderStyle::Pixel` (game.toml : `render = "pixel"`) garde un rendu rétro en vraie résolution.
//!
//! Une entité de jeu choisit son moteur avec `Engine::layers()` (sans ce composant, elle n'est
//! dessinée sur aucun écran). Les positions sont en pixels DS : `ds_to_world(x, y)` place un point
//! (0,0 en haut à gauche de la zone 256x192) dans le repère de la caméra 2D (origine au centre, y
//! vers le haut).

use bevy::camera::visibility::RenderLayers;
use bevy::camera::{ImageRenderTarget, RenderTarget};
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureFormat};
use bevy::window::{PrimaryWindow, WindowRef};

pub const SCREEN_W: u32 = 256;
pub const SCREEN_H: u32 = 192;
/// Largeur maximale en mode « remplir » (21:9).
const MAX_WIDE: f32 = SCREEN_H as f32 * 21.0 / 9.0;

/// Moteur graphique DS (ce qui est dessiné).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Engine {
    Main,
    Sub,
}

/// Écran physique (où c'est affiché). Le stylet est toujours sur l'écran du bas.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Deserialize, serde::Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Screen {
    #[default]
    Top,
    Bottom,
}

impl Engine {
    pub const ALL: [Engine; 2] = [Engine::Main, Engine::Sub];

    fn index(self) -> usize {
        self as usize
    }

    /// Couche de rendu du moteur : à mettre sur toute entité dessinée par ce moteur.
    pub fn layers(self) -> RenderLayers {
        RenderLayers::layer(1 + self.index())
    }
}

impl Screen {
    pub const ALL: [Screen; 2] = [Screen::Top, Screen::Bottom];

    pub fn other(self) -> Screen {
        match self {
            Screen::Top => Screen::Bottom,
            Screen::Bottom => Screen::Top,
        }
    }

    /// Couche de la fenêtre (le canvas affiché), distincte des couches des moteurs.
    pub(crate) fn display_layer(self) -> usize {
        10 + self as usize
    }
}

/// Point en pixels DS (0,0 en haut à gauche) -> position dans le repère de la caméra 2D.
pub fn ds_to_world(x: f32, y: f32) -> Vec2 {
    Vec2::new(x - SCREEN_W as f32 / 2.0, SCREEN_H as f32 / 2.0 - y)
}

/// Inverse de `ds_to_world`.
pub fn world_to_ds(p: Vec2) -> Vec2 {
    Vec2::new(p.x + SCREEN_W as f32 / 2.0, SCREEN_H as f32 / 2.0 - p.y)
}

/// Format d'image de chaque écran.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Deserialize)]
pub enum Aspect {
    /// 4:3 comme la console, bandes noires si la fenêtre est plus large.
    #[serde(rename = "4:3")]
    Ds,
    #[serde(rename = "16:9")]
    Wide16x9,
    #[serde(rename = "16:10")]
    Wide16x10,
    /// Remplit la fenêtre (de 4:3 à 21:9) : pas de bandes noires.
    #[default]
    #[serde(rename = "fill")]
    Fill,
}

impl Aspect {
    pub const ALL: [Aspect; 4] = [Aspect::Fill, Aspect::Ds, Aspect::Wide16x9, Aspect::Wide16x10];

    fn ratio(self) -> Option<f32> {
        match self {
            Aspect::Ds => Some(4.0 / 3.0),
            Aspect::Wide16x9 => Some(16.0 / 9.0),
            Aspect::Wide16x10 => Some(16.0 / 10.0),
            Aspect::Fill => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Aspect::Ds => "4:3 (DS)",
            Aspect::Wide16x9 => "16:9",
            Aspect::Wide16x10 => "16:10",
            Aspect::Fill => "remplir la fenetre",
        }
    }

    pub fn next(self) -> Aspect {
        let i = Aspect::ALL.iter().position(|a| *a == self).unwrap_or(0);
        Aspect::ALL[(i + 1) % Aspect::ALL.len()]
    }

    /// Largeur d'un écran en pixels DS pour une fenêtre de proportions `window_ratio`.
    fn ds_width(self, window_ratio: f32) -> f32 {
        match self.ratio() {
            Some(r) => SCREEN_H as f32 * r,
            None => (SCREEN_H as f32 * window_ratio).clamp(SCREEN_W as f32, MAX_WIDE),
        }
    }
}

/// Disposition des écrans, réglée par la spécification du jeu, F9 (fusion) et F8 (format).
#[derive(Resource, Clone, Copy, Debug)]
pub struct ScreenLayout {
    /// Les deux écrans dans une seule fenêtre.
    pub merged: bool,
    /// Écart entre les deux écrans fusionnés, en pixels DS.
    pub gap: u32,
    pub aspect: Aspect,
}

impl Default for ScreenLayout {
    fn default() -> Self {
        Self { merged: true, gap: 0, aspect: Aspect::default() }
    }
}

/// Place d'un écran dans sa fenêtre (calculée à chaque image).
#[derive(Clone, Copy, Debug)]
pub struct ScreenPlace {
    pub window: Entity,
    /// Zone de l'écran dans la fenêtre, en pixels logiques (origine en haut à gauche).
    pub rect: Rect,
    /// Pixels logiques de fenêtre par pixel DS.
    pub scale: f32,
    /// Largeur de l'écran en pixels DS (256 en 4:3, plus en format large).
    pub ds_width: f32,
}

impl ScreenPlace {
    /// Point de la fenêtre (pixels logiques) -> pixels DS (0..256 au centre), s'il est sur l'écran.
    pub fn to_ds(&self, cursor: Vec2) -> Option<Vec2> {
        if !self.rect.contains(cursor) {
            return None;
        }
        let p = (cursor - self.rect.min) / self.scale;
        Some(Vec2::new(p.x - (self.ds_width - SCREEN_W as f32) / 2.0, p.y))
    }
}

/// Places des deux écrans (haut, bas), mises à jour en `PostUpdate`.
#[derive(Resource, Default)]
pub struct ScreenPlaces(pub [Option<ScreenPlace>; 2]);

impl ScreenPlaces {
    pub fn get(&self, screen: Screen) -> Option<ScreenPlace> {
        self.0[screen as usize]
    }
}

/// Zones des écrans dans une fenêtre de taille `size` qui en montre `rows` (1 ou 2), avec `gap`
/// pixels DS entre eux : (largeur DS, échelle, zone de l'écran du haut ou unique).
pub fn layout(size: Vec2, rows: u32, gap: u32, aspect: Aspect) -> (f32, f32, Rect) {
    let rows_h = if rows == 2 { 2.0 * SCREEN_H as f32 + gap as f32 } else { SCREEN_H as f32 };
    let row_ratio = size.x / (size.y / rows_h * SCREEN_H as f32).max(1.0);
    let ds_width = aspect.ds_width(row_ratio);
    let scale = (size.x / ds_width).min(size.y / rows_h).max(0.01);
    let block = Vec2::new(ds_width, rows_h) * scale;
    let origin = ((size - block) / 2.0).max(Vec2::ZERO);
    (ds_width, scale, Rect::from_corners(origin, origin + Vec2::new(ds_width, SCREEN_H as f32) * scale))
}

#[derive(Resource)]
pub struct DsScreens {
    canvas: [Handle<Image>; 2],
    pub(crate) main_window: Entity,
    /// Fenêtre de l'écran du bas quand les écrans sont séparés.
    pub(crate) bottom_window: Option<Entity>,
    /// Écran physique qui affiche le moteur principal (`Top` par défaut, comme la DS).
    pub main_on: Screen,
}

impl DsScreens {
    pub fn canvas(&self, engine: Engine) -> Handle<Image> {
        self.canvas[engine.index()].clone()
    }

    /// Fenêtre qui montre cet écran.
    pub fn window(&self, screen: Screen) -> Entity {
        match screen {
            Screen::Top => self.main_window,
            Screen::Bottom => self.bottom_window.unwrap_or(self.main_window),
        }
    }

    pub fn main_window(&self) -> Entity {
        self.main_window
    }

    pub fn screen_of(&self, engine: Engine) -> Screen {
        match engine {
            Engine::Main => self.main_on,
            Engine::Sub => self.main_on.other(),
        }
    }

    pub fn engine_on(&self, screen: Screen) -> Engine {
        if screen == self.main_on { Engine::Main } else { Engine::Sub }
    }

    pub fn swap(&mut self) {
        self.main_on = self.main_on.other();
    }
}

/// Caméra 2D d'un moteur (toujours présente, dessine dans le canvas).
#[derive(Component)]
pub struct EngineCamera2d(pub Engine);

/// Caméra 3D d'un moteur, créée quand la spécification du jeu met cet écran en 3D. Le jeu la
/// place avec son `Transform` ; elle dessine avant la caméra 2D, qui se superpose (comme le BG0 3D
/// de la DS sous les sprites).
#[derive(Component)]
pub struct EngineCamera3d(pub Engine);

/// Caméra qui affiche les canvas dans une fenêtre (1 unité = 1 pixel logique de la fenêtre).
#[derive(Component)]
pub(crate) struct DisplayCamera(pub Entity);

/// Résolution de dessin des écrans.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RenderStyle {
    /// À la résolution de la fenêtre, MSAA x4 (qualité maximale).
    #[default]
    Hd,
    /// Vraie résolution DS agrandie sans lissage, comme sur la console.
    Pixel,
}

/// Sprite qui montre un canvas dans une fenêtre.
#[derive(Component)]
struct Display(Screen);

pub struct ScreensPlugin;

impl Plugin for ScreensPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RenderStyle>()
            .init_resource::<ScreenLayout>()
            .init_resource::<ScreenPlaces>()
            .add_systems(PreStartup, setup_screens)
            .add_systems(PostUpdate, (place_screens, fit_canvases).chain().before(bevy::camera::CameraUpdateSystems));
    }
}

/// Caméra d'affichage d'une fenêtre (la fenêtre principale ou celle de l'écran du bas).
pub(crate) fn display_camera(window: Entity, order: isize) -> impl Bundle {
    (
        Camera2d,
        Camera { order, clear_color: ClearColorConfig::Custom(Color::BLACK), ..default() },
        RenderTarget::Window(WindowRef::Entity(window)),
        Msaa::Off,
        DisplayCamera(window),
        RenderLayers::none(),
    )
}

fn setup_screens(mut commands: Commands, mut images: ResMut<Assets<Image>>, primary: Single<Entity, With<PrimaryWindow>>) {
    let main_window = *primary;
    let canvas = Engine::ALL.map(|engine| {
        let handle = images.add(Image::new_target_texture(SCREEN_W, SCREEN_H, TextureFormat::Bgra8UnormSrgb, None));
        commands.spawn((
            Camera2d,
            Camera { order: -10 + 2 * engine.index() as isize + 1, clear_color: ClearColorConfig::Custom(Color::BLACK), ..default() },
            RenderTarget::Image(handle.clone().into()),
            Msaa::Off,
            EngineCamera2d(engine),
            engine.layers(),
        ));
        handle
    });
    for screen in Screen::ALL {
        commands.spawn((Sprite { image: canvas[screen as usize].clone(), ..default() }, Display(screen), RenderLayers::layer(screen.display_layer())));
    }
    commands.spawn(display_camera(main_window, 0));
    commands.insert_resource(DsScreens { canvas, main_window, bottom_window: None, main_on: Screen::Top });
}

/// Calcule la place de chaque écran et y met son sprite ; chaque caméra d'affichage montre les
/// écrans de sa fenêtre.
fn place_screens(
    screens: Res<DsScreens>,
    layout_cfg: Res<ScreenLayout>,
    windows: Query<&Window>,
    mut places: ResMut<ScreenPlaces>,
    mut displays: Query<(&Display, &mut Sprite, &mut Transform, &mut Visibility)>,
    mut cameras: Query<(&DisplayCamera, &mut RenderLayers)>,
) {
    // `link` crée ou ferme la fenêtre du bas selon `ScreenLayout::merged`
    let merged = screens.bottom_window.is_none();
    let mut new_places = [None; 2];
    for screen in Screen::ALL {
        let window_entity = screens.window(screen);
        let Ok(window) = windows.get(window_entity) else { continue };
        let size = window.size();
        let rows = if merged { 2 } else { 1 };
        let (ds_width, scale, first) = layout(size, rows, layout_cfg.gap, layout_cfg.aspect);
        let rect = if merged && screen == Screen::Bottom {
            let dy = (SCREEN_H + layout_cfg.gap) as f32 * scale;
            Rect::from_corners(first.min + Vec2::Y * dy, first.max + Vec2::Y * dy)
        } else {
            first
        };
        new_places[screen as usize] = Some(ScreenPlace { window: window_entity, rect, scale, ds_width });
    }
    places.0 = new_places;

    for (display, mut sprite, mut transform, mut visibility) in &mut displays {
        let Some(place) = places.get(display.0) else {
            *visibility = Visibility::Hidden;
            continue;
        };
        let Ok(window) = windows.get(place.window) else { continue };
        let image = screens.canvas(screens.engine_on(display.0));
        if sprite.image != image {
            sprite.image = image;
        }
        let size = place.rect.size();
        if sprite.custom_size != Some(size) {
            sprite.custom_size = Some(size);
        }
        // Repère de la caméra d'affichage : origine au centre de la fenêtre, y vers le haut
        let center = place.rect.center() - window.size() / 2.0;
        let translation = Vec3::new(center.x, -center.y, 0.0);
        if transform.translation != translation {
            transform.translation = translation;
        }
        if *visibility != Visibility::Visible {
            *visibility = Visibility::Visible;
        }
    }

    for (camera, mut layers) in &mut cameras {
        let mut shown: Vec<usize> = Screen::ALL.iter().filter(|s| places.get(**s).is_some_and(|p| p.window == camera.0)).map(|s| s.display_layer()).collect();
        if camera.0 == screens.main_window {
            shown.push(crate::link::TOAST_LAYER);
        }
        let wanted = RenderLayers::from_layers(&shown);
        if *layers != wanted {
            *layers = wanted;
        }
    }
}

/// Taille de chaque canvas : celle (en pixels physiques) de la zone de son écran, ou la vraie
/// résolution DS en mode pixel. Le facteur d'échelle de la cible garde les caméras en pixels DS
/// et fait dessiner le texte à la bonne taille.
fn fit_canvases(
    style: Res<RenderStyle>,
    screens: Res<DsScreens>,
    places: Res<ScreenPlaces>,
    windows: Query<&Window>,
    mut images: ResMut<Assets<Image>>,
    mut cameras: Query<(&mut RenderTarget, &mut Msaa, Option<&EngineCamera2d>, Option<&EngineCamera3d>)>,
    mut applied_style: Local<[Option<RenderStyle>; 2]>,
) {
    for engine in Engine::ALL {
        let Some(place) = places.get(screens.screen_of(engine)) else { continue };
        let Ok(window) = windows.get(place.window) else { continue };
        let scale = match *style {
            RenderStyle::Hd => (place.scale * window.scale_factor()).max(0.25),
            RenderStyle::Pixel => 1.0,
        };
        let size = UVec2::new((place.ds_width * scale).round().max(1.0) as u32, (SCREEN_H as f32 * scale).round().max(1.0) as u32);
        let handle = screens.canvas(engine);
        let Some(current) = images.get(&handle).map(|i| i.size()) else { continue };
        let restyle = applied_style[engine as usize] != Some(*style);
        if current != size || restyle {
            let Some(mut image) = images.get_mut(&handle) else { continue };
            image.resize(Extent3d { width: size.x, height: size.y, depth_or_array_layers: 1 });
            image.sampler = if *style == RenderStyle::Pixel { ImageSampler::nearest() } else { ImageSampler::linear() };
            applied_style[engine as usize] = Some(*style);
        }
        // Facteur exact du canvas arrondi : la caméra voit exactement 192 pixels DS de haut
        let scale = size.y as f32 / SCREEN_H as f32;
        let msaa = if *style == RenderStyle::Hd { Msaa::Sample4 } else { Msaa::Off };
        for (mut target, mut camera_msaa, cam2d, cam3d) in &mut cameras {
            if cam2d.map(|c| c.0) != Some(engine) && cam3d.map(|c| c.0) != Some(engine) {
                continue;
            }
            if *camera_msaa != msaa {
                *camera_msaa = msaa;
            }
            let up_to_date = matches!(&*target, RenderTarget::Image(t) if t.handle == handle && t.scale_factor == scale);
            if !up_to_date {
                *target = RenderTarget::Image(ImageRenderTarget { handle: handle.clone(), scale_factor: scale });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merged_ds_layout_stacks_two_4_3_screens() {
        // 512x(384*2) : deux écrans x2 l'un sous l'autre, sans bandes
        let (w, s, top) = layout(Vec2::new(512.0, 768.0), 2, 0, Aspect::Ds);
        assert_eq!((w, s), (256.0, 2.0));
        assert_eq!(top, Rect::new(0.0, 0.0, 512.0, 384.0));
        // Fenêtre trop large en 4:3 : bandes noires sur les côtés
        let (_, s, top) = layout(Vec2::new(1000.0, 768.0), 2, 0, Aspect::Ds);
        assert_eq!(s, 2.0);
        assert_eq!(top.min.x, (1000.0 - 512.0) / 2.0);
    }

    #[test]
    fn fill_widens_the_screens_and_ds_points_stay_centered() {
        let (w, s, top) = layout(Vec2::new(800.0, 768.0), 2, 0, Aspect::Fill);
        assert_eq!((w, s), (400.0, 2.0));
        assert_eq!(top.width(), 800.0);
        let place = ScreenPlace { window: Entity::PLACEHOLDER, rect: top, scale: s, ds_width: w };
        // Le centre de la fenêtre = le centre de la zone DS (128, 96)
        assert_eq!(place.to_ds(Vec2::new(400.0, 192.0)), Some(Vec2::new(128.0, 96.0)));
        // Jamais plus large que 21:9 (448) : bandes sur les côtés au-delà
        let (w, ..) = layout(Vec2::new(1400.0, 768.0), 2, 0, Aspect::Fill);
        assert_eq!(w, 448.0);
        // Fenêtre plus étroite que 4:3 : jamais moins de 256 de large (bandes en haut et en bas)
        let (w, ..) = layout(Vec2::new(300.0, 768.0), 2, 0, Aspect::Fill);
        assert_eq!(w, 256.0);
        // 16:9 avec un écart
        let (w, ..) = layout(Vec2::new(683.0, 800.0), 2, 16, Aspect::Wide16x9);
        assert!((w - 341.33).abs() < 0.01);
    }
}
