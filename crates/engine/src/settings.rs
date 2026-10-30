//! Options gardées d'un lancement à l'autre (fichier `options.toml` dans le dossier des données).
//!
//! - **Langue** : anglais par défaut, français au choix (`i18n`) ;
//! - **Affichage** : format, écrans fusionnés ou séparés, plein écran, rendu HD ou pixel ;
//! - **Développement** : recompiler et relancer à l'enregistrement, accès développeur ;
//! - **Manettes** : modèle choisi à la main dans Options > Manettes ;
//! - **Fenêtre principale** : place (pixels physiques), taille (pixels logiques), agrandie.
//!
//! Lu au démarrage et appliqué, sauf ce qui est passé en ligne de commande (`LaunchOptions` :
//! le studio se relance avec sa place après une recompilation). Les jeux peuvent imposer leur format
//! ou leur rendu (game.toml) : seuls les changements du joueur sont gardés (F8, F9, Options,
//! fenêtre déplacée...), écrits 1 s après le dernier changement et en quittant.

use bevy::ecs::system::NonSendMarker;
use bevy::prelude::*;
use bevy::state::state::StateTransitionSystems;
use bevy::window::{PrimaryWindow, WindowMode, WindowResolution};
use bevy::winit::WINIT_WINDOWS;
use serde::{Deserialize, Serialize};

use crate::devtools::{DevSettings, LaunchOptions};
use crate::i18n::{self, Lang, Language};
use crate::options::OptionsView;
use crate::screens::{Aspect, DsScreens, RenderStyle, ScreenLayout};
use crate::studio::StudioState;

/// Écrire le fichier 1 s après le dernier changement (pas à chaque image d'un glisser de fenêtre).
const SAVE_DELAY: f64 = 1.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplaySettings {
    /// fill | 4:3 | 16:9 | 16:10
    pub aspect: String,
    pub merged: bool,
    pub fullscreen: bool,
    /// hd | pixel
    pub render: String,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        let layout = ScreenLayout::default();
        Self { aspect: aspect_name(layout.aspect).into(), merged: layout.merged, fullscreen: false, render: render_name(RenderStyle::default()).into() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DevOptions {
    pub auto_reload: bool,
    pub developer_access: bool,
}

impl Default for DevOptions {
    fn default() -> Self {
        let dev = DevSettings::default();
        Self { auto_reload: dev.auto_reload, developer_access: dev.developer_access }
    }
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ControllerOptions {
    /// Modèle choisi à la main (id de controllers/<id>.toml) ; sans, celui de la manette branchée.
    pub model: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowOptions {
    /// Coin de la fenêtre principale (pixels physiques).
    pub position: Option<[i32; 2]>,
    /// Taille de la fenêtre principale (pixels logiques).
    pub size: Option<[f32; 2]>,
    pub maximized: bool,
}

/// Position que Windows donne à une fenêtre réduite (-32000) : jamais une vraie place.
const MINIMIZED_POS: i32 = -30000;
/// Largeur en dessous de laquelle la page d'un jeu du launcher ne tient plus.
const MIN_LAUNCHER_W: f32 = 960.0;

/// Tout ce qui est gardé dans options.toml.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// en | fr (écrit en premier : le launcher de mise à jour lit cette ligne)
    pub language: String,
    pub display: DisplaySettings,
    pub dev: DevOptions,
    pub controllers: ControllerOptions,
    pub window: WindowOptions,
}

impl Settings {
    /// Fichier vide, cassé ou d'une autre version : les valeurs par défaut (jamais d'erreur).
    pub fn parse(text: &str) -> Self {
        toml::from_str(text).unwrap_or_else(|e| {
            warn!("options.toml illisible, options par defaut : {e}");
            Self::default()
        })
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    pub fn aspect(&self) -> Aspect {
        Aspect::ALL.into_iter().find(|a| aspect_name(*a) == self.display.aspect).unwrap_or_default()
    }

    pub fn render(&self) -> RenderStyle {
        if self.display.render == render_name(RenderStyle::Pixel) { RenderStyle::Pixel } else { RenderStyle::Hd }
    }
}

fn aspect_name(aspect: Aspect) -> &'static str {
    match aspect {
        Aspect::Ds => "4:3",
        Aspect::Wide16x9 => "16:9",
        Aspect::Wide16x10 => "16:10",
        Aspect::Fill => "fill",
    }
}

fn render_name(style: RenderStyle) -> &'static str {
    match style {
        RenderStyle::Hd => "hd",
        RenderStyle::Pixel => "pixel",
    }
}

pub fn settings_path() -> std::path::PathBuf {
    crate::save::data_dir().join("options.toml")
}

fn load() -> Settings {
    let path = settings_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            info!("options lues : {}", path.display());
            Settings::parse(&text)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            info!("pas encore d'options.toml : options par defaut");
            Settings::default()
        }
        Err(e) => {
            warn!("options.toml illisible ({e}) : options par defaut");
            Settings::default()
        }
    }
}

fn store(settings: &Settings) {
    let path = settings_path();
    match crate::save::write_atomic(&path, &settings.to_toml()) {
        Ok(()) => info!("options enregistrees : {}", path.display()),
        Err(e) => warn!("options non enregistrees ({}) : {e}", path.display()),
    }
}

/// Remettre toutes les options par défaut et effacer options.toml (bouton d'Options > Affichage).
#[derive(Message)]
pub struct ResetSettings;

/// Les choix du joueur, et ce qui était à l'écran à la dernière image.
#[derive(Resource, Default)]
struct PlayerSettings {
    player: Settings,
    seen: Option<Settings>,
    /// Moment du dernier changement pas encore écrit.
    dirty_at: Option<f64>,
}

/// Agrandir la fenêtre principale dès qu'elle existe (winit ne le fait pas à la création).
#[derive(Resource)]
struct MaximizeAtStart;

pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PlayerSettings>()
            .add_message::<ResetSettings>()
            .add_systems(StateTransition, restore_on_menu.after(StateTransitionSystems::EnterSchedules))
            .add_systems(Update, (maximize_at_start.run_if(resource_exists::<MaximizeAtStart>), reset))
            .add_systems(Last, watch);
    }

    // `finish` : après `NdsEnginePlugin::build`, donc après `LaunchOptions::apply_to_app` (qui
    // remet un `ScreenLayout`) ; on applique le fichier sans toucher à ce que la ligne de commande donne.
    fn finish(&self, app: &mut App) {
        let mut player = load();
        let cli = LaunchOptions::from_args();
        // Relance après recompilation (place passée) : les écrans restent comme ils étaient
        let cli_screens = cli.split || cli.at.is_some() || cli.size.is_some();
        let world = app.world_mut();

        let mut layout = world.get_resource::<ScreenLayout>().copied().unwrap_or_default();
        if cli.aspect.is_none() {
            layout.aspect = player.aspect();
        }
        if !cli_screens {
            layout.merged = player.display.merged;
        }
        world.insert_resource(layout);
        world.insert_resource(player.render());
        world.insert_resource(DevSettings { auto_reload: player.dev.auto_reload, developer_access: player.dev.developer_access });
        let mut language = Language::default();
        i18n::set_language(&mut language, Lang::from_code(&player.language));
        world.insert_resource(language);
        if let Some(model) = player.controllers.model.clone() {
            if let Some(mut view) = world.get_resource_mut::<OptionsView>() {
                view.model = Some(model);
                view.model_chosen = true;
            }
        }

        let mut primary = world.query_filtered::<&mut Window, With<PrimaryWindow>>();
        if let Ok(mut window) = primary.single_mut(world) {
            // Place gardée pendant que la fenêtre était réduite (anciennes versions) : ignorée
            let minimized = player.window.position.is_some_and(|[x, y]| x <= MINIMIZED_POS || y <= MINIMIZED_POS);
            if let (None, Some([x, y]), false) = (cli.at, player.window.position, minimized) {
                window.position = WindowPosition::At(IVec2::new(x, y));
            }
            // Une fenêtre étroite (taille d'un jeu, anciennes versions) : la taille du launcher
            if let (None, Some([w, h]), false) = (cli.size, player.window.size, minimized)
                && w >= MIN_LAUNCHER_W
            {
                window.resolution = WindowResolution::new(w as u32, h.max(128.0) as u32);
            }
            if player.display.fullscreen {
                window.mode = WindowMode::BorderlessFullscreen(MonitorSelection::Current);
            }
        }
        if player.window.maximized && !player.display.fullscreen && cli.at.is_none() && cli.size.is_none() {
            world.insert_resource(MaximizeAtStart);
        }

        // Ce que la ligne de commande impose est aussi le choix actuel du joueur (relance)
        if let Some(aspect) = cli.aspect {
            player.display.aspect = aspect_name(aspect).into();
        }
        if cli_screens {
            player.display.merged = !cli.split;
        }
        world.insert_resource(PlayerSettings { player, seen: None, dirty_at: None });
    }
}

fn maximize_at_start(mut commands: Commands, screens: Option<Res<DsScreens>>, mut windows: Query<&mut Window>, _main_thread: NonSendMarker) {
    let Some(screens) = screens else { return };
    let main = screens.main_window();
    if !WINIT_WINDOWS.with_borrow(|w| w.get_window(main).is_some()) {
        return;
    }
    if let Ok(mut window) = windows.get_mut(main) {
        window.set_maximized(true);
    }
    commands.remove_resource::<MaximizeAtStart>();
}

/// Retour au launcher : la spécification du studio a remis le rendu HD (et le jeu a pu changer le
/// format), on remet les choix du joueur.
fn restore_on_menu(
    state: Option<Res<State<StudioState>>>,
    mut last: Local<Option<StudioState>>,
    mut settings: ResMut<PlayerSettings>,
    mut layout: ResMut<ScreenLayout>,
    mut style: ResMut<RenderStyle>,
) {
    let Some(state) = state else { return };
    let now = state.get().clone();
    if last.as_ref() == Some(&now) {
        return;
    }
    *last = Some(now.clone());
    if now == StudioState::Menu {
        let aspect = settings.player.aspect();
        let render = settings.player.render();
        if layout.aspect != aspect {
            layout.aspect = aspect;
        }
        if *style != render {
            *style = render;
        }
    }
    // Ce que la spécification vient de changer n'est pas un choix du joueur
    settings.seen = None;
}

#[allow(clippy::too_many_arguments)]
fn reset(
    mut requests: MessageReader<ResetSettings>,
    mut settings: ResMut<PlayerSettings>,
    mut layout: ResMut<ScreenLayout>,
    mut style: ResMut<RenderStyle>,
    mut dev: ResMut<DevSettings>,
    mut view: ResMut<OptionsView>,
    mut language: ResMut<Language>,
    screens: Option<Res<DsScreens>>,
    mut windows: Query<&mut Window>,
) {
    if requests.read().count() == 0 {
        return;
    }
    let path = settings_path();
    match std::fs::remove_file(&path) {
        Ok(()) => info!("options reinitialisees : {} efface", path.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => info!("options reinitialisees"),
        Err(e) => warn!("options reinitialisees, mais {} n'a pas pu etre efface : {e}", path.display()),
    }
    let defaults = Settings::default();
    layout.aspect = defaults.aspect();
    layout.merged = defaults.display.merged;
    *style = defaults.render();
    *dev = DevSettings::default();
    i18n::set_language(&mut language, Lang::default());
    view.model = None;
    view.model_chosen = false;
    if let Some(mut window) = screens.and_then(|s| windows.get_mut(s.main_window()).ok()) {
        window.mode = WindowMode::Windowed;
    }
    *settings = PlayerSettings { player: defaults, seen: None, dirty_at: None };
}

/// Ce qui est à l'écran ; place et taille gardées d'avant tant que la fenêtre est agrandie ou en plein écran.
fn snapshot(player: &Settings, layout: &ScreenLayout, style: RenderStyle, dev: &DevSettings, view: &OptionsView, main: Entity, window: Option<&Window>) -> Settings {
    let mut now = player.clone();
    now.language = i18n::lang().code().into();
    now.display.aspect = aspect_name(layout.aspect).into();
    now.display.merged = layout.merged;
    now.display.render = render_name(style).into();
    now.dev.auto_reload = dev.auto_reload;
    now.dev.developer_access = dev.developer_access;
    now.controllers.model = if view.model_chosen { view.model.clone() } else { None };
    let Some(window) = window else { return now };
    let fullscreen = window.mode != WindowMode::Windowed;
    now.display.fullscreen = fullscreen;
    if fullscreen {
        return now;
    }
    let (maximized, minimized, at) = WINIT_WINDOWS
        .with_borrow(|w| w.get_window(main).map(|w| (w.is_maximized(), w.is_minimized().unwrap_or(false), w.outer_position().ok().map(|p| [p.x, p.y]))))
        .unwrap_or((false, false, None));
    // Réduite dans la barre des tâches, Windows la met en (-32000, -32000) en 160x28 : on garde
    // la place d'avant (sinon le studio se rouvrirait réduit)
    if minimized || at.is_some_and(|[x, y]| x <= MINIMIZED_POS || y <= MINIMIZED_POS) {
        return now;
    }
    now.window.maximized = maximized;
    if !maximized {
        if at.is_some() {
            now.window.position = at;
        }
        now.window.size = Some([window.width(), window.height()]);
    }
    now
}

/// Garde les changements du joueur et écrit le fichier (1 s après le dernier, ou en quittant).
#[allow(clippy::too_many_arguments)]
fn watch(
    mut settings: ResMut<PlayerSettings>,
    layout: Res<ScreenLayout>,
    style: Res<RenderStyle>,
    dev: Res<DevSettings>,
    view: Res<OptionsView>,
    screens: Option<Res<DsScreens>>,
    windows: Query<&Window>,
    time: Res<Time<Real>>,
    mut exit: MessageReader<AppExit>,
    _main_thread: NonSendMarker,
) {
    let Some(screens) = screens else { return };
    let main = screens.main_window();
    let now = snapshot(&settings.player, &layout, *style, &dev, &view, main, windows.get(main).ok());
    let seconds = time.elapsed_secs_f64();
    if let Some(seen) = settings.seen.take().filter(|seen| *seen != now) {
        // Seuls les champs qui viennent de changer sont des choix du joueur
        let player = &mut settings.player;
        macro_rules! take {
            ($($field:ident).+) => {
                if seen.$($field).+ != now.$($field).+ {
                    player.$($field).+ = now.$($field).+.clone();
                }
            };
        }
        take!(language);
        take!(display.aspect);
        take!(display.merged);
        take!(display.fullscreen);
        take!(display.render);
        take!(dev.auto_reload);
        take!(dev.developer_access);
        take!(controllers.model);
        take!(window.position);
        take!(window.size);
        take!(window.maximized);
        settings.dirty_at = Some(seconds);
    }
    settings.seen = Some(now);
    let quitting = exit.read().count() > 0;
    if settings.dirty_at.is_some_and(|t| quitting || seconds - t >= SAVE_DELAY) {
        settings.dirty_at = None;
        store(&settings.player);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let settings = Settings {
            language: "fr".into(),
            display: DisplaySettings { aspect: "16:9".into(), merged: false, fullscreen: true, render: "pixel".into() },
            dev: DevOptions { auto_reload: false, developer_access: true },
            controllers: ControllerOptions { model: Some("xbox-series".into()) },
            window: WindowOptions { position: Some([-10, 40]), size: Some([512.0, 800.0]), maximized: true },
        };
        let parsed = Settings::parse(&settings.to_toml());
        assert_eq!(parsed, settings);
        assert_eq!(parsed.aspect(), Aspect::Wide16x9);
        assert_eq!(parsed.render(), RenderStyle::Pixel);
        // Le launcher de mise à jour cherche cette ligne
        assert!(settings.to_toml().lines().any(|l| l == "language = \"fr\""));
    }

    #[test]
    fn defaults() {
        assert_eq!(Settings::parse(""), Settings::default());
        assert_eq!(Settings::parse("n'importe quoi = = ["), Settings::default());
        assert_eq!(Settings::parse("display = 3"), Settings::default());
        // Champ inconnu ou manquant : le reste est gardé
        let partial = Settings::parse("[display]\nrender = \"pixel\"\nautre = 1\n");
        assert_eq!(partial.render(), RenderStyle::Pixel);
        assert_eq!(partial.aspect(), Aspect::default());
        assert!(partial.display.merged);
        // Format inconnu : celui par défaut
        assert_eq!(Settings::parse("[display]\naspect = \"21:9\"").aspect(), Aspect::default());
        let defaults = Settings::default();
        assert_eq!(defaults.aspect(), ScreenLayout::default().aspect);
        assert_eq!(defaults.render(), RenderStyle::default());
        assert!(defaults.dev.auto_reload);
        assert!(!defaults.dev.developer_access);
        assert_eq!(Lang::from_code(&defaults.language), Lang::En);
    }
}
