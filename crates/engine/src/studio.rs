//! Le studio : liste des jeux enregistrés, menu, lancement d'un jeu avec sa spécification.
//!
//! Un jeu est un plugin Bevy qui s'enregistre avec sa spécification et une fonction de départ :
//!
//! ```ignore
//! app.add_ds_game(include_str!("game.toml"), setup)
//!    .add_systems(FixedUpdate, (bouger, sauter).run_if(playing("platformer")));
//! ```
//!
//! `setup` crée les entités du jeu, chacune avec `in_game()` pour qu'elles disparaissent au
//! retour au menu, et `Engine::Main.layers()` ou `Engine::Sub.layers()` pour choisir son écran.
//! Échap, ou Start + Select, revient au menu.

use bevy::camera::RenderTarget;
use bevy::ecs::system::SystemId;
use bevy::prelude::*;
use bevy::sprite::Anchor;
use bevy::text::TextBounds;

use crate::input::{DsButton, DsInput, KeyMap, Stylus};
use crate::screens::{ds_to_world, DsScreens, Engine, EngineCamera2d, EngineCamera3d, Screen, ScreenLayout, SCREEN_H, SCREEN_W};
use crate::spec::{GameSpec, ScreenMode};

#[derive(States, Default, Clone, PartialEq, Eq, Hash, Debug)]
pub enum StudioState {
    /// Avant la création des écrans (`PreStartup`) : Bevy entre dans l'état initial plus tôt.
    #[default]
    Boot,
    Menu,
    Playing,
}

/// Identifiant (`id` de game.toml) du jeu en cours.
#[derive(Resource, Default)]
pub struct ActiveGame(pub Option<String>);

/// Jeu à lancer tout de suite au lieu d'afficher le menu (ligne de commande).
#[derive(Resource)]
pub struct AutoStart(pub String);

struct RegisteredGame {
    spec: GameSpec,
    setup: SystemId,
}

#[derive(Resource, Default)]
pub struct GameRegistry(Vec<RegisteredGame>);

impl GameRegistry {
    pub fn specs(&self) -> impl Iterator<Item = &GameSpec> {
        self.0.iter().map(|g| &g.spec)
    }

    fn find(&self, id: &str) -> Option<&RegisteredGame> {
        self.0.iter().find(|g| g.spec.id == id)
    }
}

pub trait AppGameExt {
    /// Enregistre un jeu : sa spécification (texte de game.toml) et sa fonction de départ.
    fn add_ds_game<M>(&mut self, spec_toml: &str, setup: impl IntoSystem<(), (), M> + 'static) -> &mut Self;
}

impl AppGameExt for App {
    fn add_ds_game<M>(&mut self, spec_toml: &str, setup: impl IntoSystem<(), (), M> + 'static) -> &mut Self {
        let spec = GameSpec::from_toml(spec_toml).unwrap_or_else(|e| panic!("game.toml invalide : {e}"));
        let setup = self.register_system(setup);
        let mut registry = self.world_mut().get_resource_or_init::<GameRegistry>();
        assert!(registry.find(&spec.id).is_none(), "deux jeux ont l'id {}", spec.id);
        registry.0.push(RegisteredGame { spec, setup });
        self
    }
}

/// Condition : ce jeu est en cours.
pub fn playing(id: &'static str) -> impl FnMut(Res<State<StudioState>>, Res<ActiveGame>) -> bool {
    move |state, active| *state.get() == StudioState::Playing && active.0.as_deref() == Some(id)
}

/// Composant à mettre sur chaque entité d'un jeu : supprimée au retour au menu.
pub fn in_game() -> DespawnOnExit<StudioState> {
    DespawnOnExit(StudioState::Playing)
}

pub struct StudioPlugin;

impl Plugin for StudioPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GameRegistry>()
            .init_resource::<ActiveGame>()
            .init_state::<StudioState>()
            .add_systems(Startup, |mut commands: Commands| commands.set_state(StudioState::Menu))
            .add_systems(OnEnter(StudioState::Menu), (reset_to_studio, spawn_menu).chain())
            .add_systems(FixedUpdate, menu_input.run_if(in_state(StudioState::Menu)))
            .add_systems(Update, refresh_menu.run_if(in_state(StudioState::Menu)))
            .add_systems(OnEnter(StudioState::Playing), start_game)
            .add_systems(Update, back_to_menu.run_if(in_state(StudioState::Playing)));
    }
}

// ─────────────────────────────────────────────────────────────────────────
//  Application d'une spécification
// ─────────────────────────────────────────────────────────────────────────

fn apply_spec(world: &mut World, spec: &GameSpec, title: &str) {
    world.resource_mut::<DsScreens>().main_on = spec.screens.main;
    world.insert_resource(spec.screens.render);
    {
        let mut layout = world.resource_mut::<ScreenLayout>();
        layout.gap = spec.screens.gap;
        // Sans format imposé par le jeu, on garde celui choisi par le joueur (F8)
        if let Some(aspect) = spec.screens.aspect {
            layout.aspect = aspect;
        }
    }
    world.insert_resource(spec.key_map());
    world.resource_mut::<Time<Fixed>>().set_timestep_hz(spec.timing.fps.clamp(1.0, 240.0));
    // Un jeu précédent a pu déplacer les caméras 2D (défilement)
    let mut cameras = world.query_filtered::<&mut Transform, With<EngineCamera2d>>();
    for mut transform in cameras.iter_mut(world) {
        *transform = Transform::default();
    }

    // Taille des fenêtres : seulement si le jeu en impose une (sinon on garde celle du joueur,
    // fenêtre aimantée à une moitié d'écran par exemple)
    let (main, bottom) = {
        let screens = world.resource::<DsScreens>();
        (screens.window(Screen::Top), screens.bottom_window)
    };
    let gap = spec.screens.gap as f32;
    for (entity, rows) in [(Some(main), if bottom.is_some() { 1.0 } else { 2.0 }), (bottom, 1.0)] {
        let Some(mut window) = entity.and_then(|e| world.get_mut::<Window>(e)) else { continue };
        window.title = if Some(main) == entity { title.to_string() } else { format!("{title} - ecran tactile") };
        if let Some(scale) = spec.screens.scale {
            let scale = scale.clamp(1, 8) as f32;
            let height = if rows == 2.0 { 2.0 * SCREEN_H as f32 + gap } else { SCREEN_H as f32 };
            window.resolution.set(SCREEN_W as f32 * scale, height * scale);
        }
    }

    for engine in Engine::ALL {
        let main = engine == Engine::Main;
        let three_d = spec.mode(main) == ScreenMode::ThreeD;
        let clear = spec.clear_color(main);
        let mut cameras = world.query::<(&EngineCamera2d, &mut Camera)>();
        for (cam, mut camera) in cameras.iter_mut(world) {
            if cam.0 == engine {
                // En 3D, la 2D se dessine par-dessus l'image 3D
                camera.clear_color = if three_d { ClearColorConfig::None } else { ClearColorConfig::Custom(clear) };
            }
        }
        if three_d {
            let canvas = world.resource::<DsScreens>().canvas(engine);
            world.spawn((
                Camera3d::default(),
                Camera { order: -20 + engine as isize, clear_color: ClearColorConfig::Custom(clear), ..default() },
                RenderTarget::Image(canvas.into()),
                Msaa::Off,
                Transform::from_xyz(0.0, 0.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y),
                EngineCamera3d(engine),
                engine.layers(),
                in_game(),
            ));
        }
    }
}

fn studio_spec() -> GameSpec {
    let mut spec = GameSpec::from_toml("id = \"studio\"\nname = \"Nds Game Studio\"").expect("spec du studio");
    spec.screens.main_clear = "#14182b".into();
    spec.screens.sub_clear = "#1f2540".into();
    spec
}

fn reset_to_studio(world: &mut World) {
    world.resource_mut::<ActiveGame>().0 = None;
    let spec = studio_spec();
    apply_spec(world, &spec, "Nds Game Studio");
    world.insert_resource(KeyMap::default());
}

fn start_game(world: &mut World) {
    let Some(id) = world.resource::<ActiveGame>().0.clone() else { return };
    let Some((spec, setup)) = world.resource::<GameRegistry>().find(&id).map(|g| (g.spec.clone(), g.setup)) else {
        error!("jeu inconnu : {id}");
        world.resource_mut::<NextState<StudioState>>().set(StudioState::Menu);
        return;
    };
    info!("lancement de {} ({})", spec.name, spec.summary());
    apply_spec(world, &spec, &format!("Nds Game Studio - {}", spec.name));
    if let Err(e) = world.run_system(setup) {
        error!("{id} : setup a echoue : {e}");
    }
}

fn back_to_menu(keys: Res<ButtonInput<KeyCode>>, input: Res<DsInput>, mut next: ResMut<NextState<StudioState>>) {
    if keys.just_pressed(KeyCode::Escape) || (input.pressed(DsButton::Start) && input.pressed(DsButton::Select)) {
        next.set(StudioState::Menu);
    }
}

// ─────────────────────────────────────────────────────────────────────────
//  Menu : infos du jeu choisi en haut, liste tactile en bas
// ─────────────────────────────────────────────────────────────────────────

const ITEM_H: f32 = 26.0;
const LIST_TOP: f32 = 30.0;

#[derive(Resource, Default)]
struct MenuSelection(usize);

#[derive(Component)]
struct MenuItem(usize);

#[derive(Component)]
struct MenuInfo;

/// Texte affiché en bas de l'écran du haut (numéro de version, fourni par l'application).
#[derive(Resource, Default)]
pub struct StudioFooter(pub String);

fn text(value: impl Into<String>, size: f32, color: Color) -> (Text2d, TextFont, TextColor) {
    (Text2d::new(value), TextFont::from_font_size(size), TextColor(color))
}

fn spawn_menu(mut commands: Commands, registry: Res<GameRegistry>, footer: Option<Res<StudioFooter>>, auto: Option<Res<AutoStart>>) {
    if let Some(auto) = auto {
        commands.remove_resource::<AutoStart>();
        commands.insert_resource(ActiveGame(Some(auto.0.clone())));
        commands.set_state(StudioState::Playing);
        return;
    }
    commands.init_resource::<MenuSelection>();
    let scoped = DespawnOnExit(StudioState::Menu);
    let main = Engine::Main.layers();
    let sub = Engine::Sub.layers();

    commands.spawn((text("Nds Game Studio", 20.0, Color::srgb(0.95, 0.85, 0.4)), Transform::from_translation(ds_to_world(128.0, 18.0).extend(1.0)), main.clone(), scoped.clone()));
    commands.spawn((
        text("", 10.0, Color::WHITE),
        Anchor::TOP_LEFT,
        TextBounds::new_horizontal(236.0),
        Transform::from_translation(ds_to_world(10.0, 40.0).extend(1.0)),
        MenuInfo,
        main.clone(),
        scoped.clone(),
    ));
    let footer = footer.map(|f| f.0.clone()).unwrap_or_default();
    commands.spawn((text(footer, 8.0, Color::srgb(0.6, 0.6, 0.7)), Transform::from_translation(ds_to_world(128.0, 184.0).extend(1.0)), main, scoped.clone()));

    commands.spawn((text("Touchez un jeu (A pour jouer)", 10.0, Color::srgb(0.8, 0.8, 0.9)), Transform::from_translation(ds_to_world(128.0, 14.0).extend(1.0)), sub.clone(), scoped.clone()));
    for (i, spec) in registry.specs().enumerate() {
        let center = ds_to_world(128.0, LIST_TOP + ITEM_H * i as f32 + ITEM_H / 2.0);
        commands
            .spawn((Sprite::from_color(Color::srgb(0.2, 0.25, 0.4), Vec2::new(220.0, ITEM_H - 4.0)), Transform::from_translation(center.extend(0.0)), MenuItem(i), sub.clone(), scoped.clone()))
            .with_child((text(spec.name.clone(), 11.0, Color::WHITE), Transform::from_xyz(0.0, 0.0, 1.0), sub.clone()));
    }
    if registry.0.is_empty() {
        commands.spawn((text("Aucun jeu enregistre", 11.0, Color::WHITE), Transform::from_translation(ds_to_world(128.0, 96.0).extend(1.0)), sub, scoped));
    }
}

fn menu_input(
    input: Res<DsInput>,
    stylus: Res<Stylus>,
    registry: Res<GameRegistry>,
    selection: Option<ResMut<MenuSelection>>,
    mut commands: Commands,
) {
    let (Some(mut selection), false) = (selection, registry.0.is_empty()) else { return };
    let count = registry.0.len();
    let mut launch = input.just_pressed(DsButton::A) || input.just_pressed(DsButton::Start);
    if input.just_pressed(DsButton::Down) {
        selection.0 = (selection.0 + 1) % count;
    }
    if input.just_pressed(DsButton::Up) {
        selection.0 = (selection.0 + count - 1) % count;
    }
    if stylus.just_touched() {
        let row = ((stylus.position.y - LIST_TOP) / ITEM_H).floor();
        if row >= 0.0 && (row as usize) < count && (18.0..238.0).contains(&stylus.position.x) {
            // Premier appui : choisir ; appui sur le jeu déjà choisi : jouer
            launch |= selection.0 == row as usize;
            selection.0 = row as usize;
        }
    }
    if launch {
        commands.insert_resource(ActiveGame(Some(registry.0[selection.0].spec.id.clone())));
        commands.set_state(StudioState::Playing);
    }
}

fn refresh_menu(
    selection: Option<Res<MenuSelection>>,
    registry: Res<GameRegistry>,
    mut items: Query<(&MenuItem, &mut Sprite)>,
    mut info: Query<&mut Text2d, With<MenuInfo>>,
) {
    let Some(selection) = selection else { return };
    if !selection.is_changed() {
        return;
    }
    for (item, mut sprite) in &mut items {
        sprite.color = if item.0 == selection.0 { Color::srgb(0.85, 0.55, 0.2) } else { Color::srgb(0.2, 0.25, 0.4) };
    }
    if let (Some(spec), Ok(mut text)) = (registry.0.get(selection.0).map(|g| &g.spec), info.single_mut()) {
        text.0 = format!("{}\n{}\n\n{}\n\n{}", spec.name, spec.genre, spec.description, spec.summary());
    }
}
