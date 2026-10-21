//! Les deux écrans DS.
//!
//! Comme sur la console, il y a deux moteurs graphiques : `Engine::Main` et `Engine::Sub`. Chacun
//! dessine dans sa propre image de 256x192 (le « canvas ») avec sa caméra 2D, et une caméra 3D en
//! plus si la spécification du jeu le demande. Chaque fenêtre (écran du haut, écran du bas) affiche
//! ensuite le canvas d'un des moteurs, agrandi sans flou. Échanger les écrans (`DsScreens::main_on`)
//! ne change que l'image affichée par chaque fenêtre, comme le bit POWCNT de la DS.
//!
//! Le canvas suit la taille de la fenêtre qui l'affiche (rendu HD) : les jeux comptent toujours en
//! pixels DS (256x192), mais formes, texte et 3D sont dessinés à la résolution de l'écran du PC.
//! Par défaut c'est la qualité maximale (résolution de la fenêtre + MSAA x4). `RenderStyle::Pixel`
//! (game.toml : `render = "pixel"`) garde la vraie résolution DS, agrandie sans lissage, pour un
//! rendu rétro.
//!
//! Une entité de jeu choisit son moteur avec `Engine::layers()` (sans ce composant, elle n'est
//! dessinée sur aucun écran). Les positions sont en pixels DS : `ds_to_world(x, y)` place un point
//! (0,0 en haut à gauche) dans le repère de la caméra 2D (origine au centre, y vers le haut).

use bevy::camera::visibility::RenderLayers;
use bevy::camera::{ImageRenderTarget, RenderTarget, ScalingMode};
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureFormat};
use bevy::window::{PrimaryWindow, WindowRef, WindowResolution};

pub const SCREEN_W: u32 = 256;
pub const SCREEN_H: u32 = 192;

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

    /// Couche de la fenêtre (le canvas agrandi), distincte des couches des moteurs.
    fn display_layers(self) -> RenderLayers {
        RenderLayers::layer(10 + self as usize)
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

#[derive(Resource)]
pub struct DsScreens {
    canvas: [Handle<Image>; 2],
    windows: [Entity; 2],
    /// Écran physique qui affiche le moteur principal (`Top` par défaut, comme la DS).
    pub main_on: Screen,
}

impl DsScreens {
    pub fn canvas(&self, engine: Engine) -> Handle<Image> {
        self.canvas[engine.index()].clone()
    }

    pub fn window(&self, screen: Screen) -> Entity {
        self.windows[screen as usize]
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

/// Résolution de dessin des écrans.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RenderStyle {
    /// À la résolution de la fenêtre (net à toutes les tailles).
    #[default]
    Hd,
    /// 256x192 agrandi sans lissage, comme sur la console.
    Pixel,
}

/// Sprite qui montre un canvas dans une fenêtre.
#[derive(Component)]
struct Display(Screen);

pub struct ScreensPlugin;

impl Plugin for ScreensPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RenderStyle>()
            .add_systems(PreStartup, setup_screens)
            .add_systems(PostUpdate, (show_canvases, fit_canvases).before(bevy::camera::CameraUpdateSystems));
    }
}

fn setup_screens(mut commands: Commands, mut images: ResMut<Assets<Image>>, primary: Single<(Entity, &Window), With<PrimaryWindow>>) {
    let (top, top_window) = *primary;
    // L'écran du bas : liée au départ (pas de barre de titre, voir `link`)
    let bottom = commands
        .spawn(Window {
            title: format!("{} - ecran tactile", top_window.title),
            resolution: WindowResolution::new(top_window.physical_width(), top_window.physical_height()),
            decorations: false,
            skip_taskbar: true,
            ..default()
        })
        .id();

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

    for (screen, window) in [(Screen::Top, top), (Screen::Bottom, bottom)] {
        commands.spawn((
            Sprite { image: canvas[screen as usize].clone(), custom_size: Some(Vec2::new(SCREEN_W as f32, SCREEN_H as f32)), ..default() },
            Display(screen),
            screen.display_layers(),
        ));
        commands.spawn((
            Camera2d,
            Camera { order: screen as isize, clear_color: ClearColorConfig::Custom(Color::BLACK), ..default() },
            RenderTarget::Window(WindowRef::Entity(window)),
            // Garde les proportions 4:3, bandes noires si la fenêtre n'est pas au bon format
            Projection::Orthographic(OrthographicProjection {
                scaling_mode: ScalingMode::AutoMin { min_width: SCREEN_W as f32, min_height: SCREEN_H as f32 },
                ..OrthographicProjection::default_2d()
            }),
            Msaa::Off,
            screen.display_layers(),
        ));
    }

    commands.insert_resource(DsScreens { canvas, windows: [top, bottom], main_on: Screen::Top });
}

/// Chaque fenêtre affiche le canvas du moteur qui lui est attribué (suit les échanges d'écrans).
fn show_canvases(screens: Res<DsScreens>, mut displays: Query<(&Display, &mut Sprite)>) {
    if !screens.is_changed() {
        return;
    }
    for (display, mut sprite) in &mut displays {
        let wanted = screens.canvas(screens.engine_on(display.0));
        if sprite.image != wanted {
            sprite.image = wanted;
        }
    }
}

/// Taille du canvas d'un moteur : celle de la fenêtre qui l'affiche (proportions 4:3 gardées) en
/// HD, 256x192 en mode pixel. Le facteur d'échelle de la cible garde les caméras en pixels DS et
/// fait dessiner le texte à la bonne taille.
fn fit_canvases(
    style: Res<RenderStyle>,
    screens: Res<DsScreens>,
    windows: Query<&Window>,
    mut images: ResMut<Assets<Image>>,
    mut cameras: Query<(&mut RenderTarget, &mut Msaa, Option<&EngineCamera2d>, Option<&EngineCamera3d>)>,
    mut applied_style: Local<[Option<RenderStyle>; 2]>,
) {
    for engine in Engine::ALL {
        let Ok(window) = windows.get(screens.window(screens.screen_of(engine))) else { continue };
        let scale = match *style {
            RenderStyle::Hd => (window.physical_width() as f32 / SCREEN_W as f32).min(window.physical_height() as f32 / SCREEN_H as f32).max(1.0),
            RenderStyle::Pixel => 1.0,
        };
        let size = UVec2::new((SCREEN_W as f32 * scale).round() as u32, (SCREEN_H as f32 * scale).round() as u32);
        let handle = screens.canvas(engine);
        let Some(current) = images.get(&handle).map(|i| i.size()) else { continue };
        let restyle = applied_style[engine as usize] != Some(*style);
        if current != size || restyle {
            let Some(mut image) = images.get_mut(&handle) else { continue };
            image.resize(Extent3d { width: size.x, height: size.y, depth_or_array_layers: 1 });
            image.sampler = if *style == RenderStyle::Pixel { ImageSampler::nearest() } else { ImageSampler::linear() };
            applied_style[engine as usize] = Some(*style);
        }
        // Qualité maximale en HD : bords lissés (formes, texte, 3D)
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

/// Zone occupée par l'écran dans une fenêtre de cette taille (bandes noires exclues) :
/// (décalage, échelle), en pixels logiques de la fenêtre.
pub fn letterbox(window_size: Vec2) -> (Vec2, f32) {
    let scale = (window_size.x / SCREEN_W as f32).min(window_size.y / SCREEN_H as f32);
    let used = Vec2::new(SCREEN_W as f32, SCREEN_H as f32) * scale;
    ((window_size - used) / 2.0, scale)
}
