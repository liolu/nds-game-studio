//! Le studio : liste des jeux enregistrés, lancement d'un jeu avec sa spécification (l'écran
//! d'accueil est le launcher, voir `launcher`).
//!
//! Un jeu est un plugin Bevy qui s'enregistre avec sa spécification et une fonction de départ :
//!
//! ```ignore
//! app.add_ds_game(include_str!("game.toml"), setup)
//!    .add_systems(FixedUpdate, (bouger, sauter).run_if(playing("platformer")));
//! ```
//!
//! `setup` crée les entités du jeu, chacune avec `in_game()` pour qu'elles disparaissent au
//! retour au launcher, et `Engine::Main.layers()` ou `Engine::Sub.layers()` pour choisir son écran.
//! Échap, ou Start + Select, revient au launcher.

use bevy::camera::RenderTarget;
use bevy::ecs::system::SystemId;
use bevy::prelude::*;

use crate::input::{DsButton, DsInput, KeyMap};
use crate::screens::{DsScreens, Engine, EngineCamera2d, EngineCamera3d, Screen, ScreenLayout, SCREEN_H, SCREEN_W};
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

/// La rom de l'utilisateur pour le jeu en cours : le runtime y lit les fichiers du jeu.
#[derive(Resource, Clone)]
pub struct ActiveRom(pub std::path::PathBuf);

/// Jeu à lancer tout de suite au lieu d'afficher le launcher (ligne de commande).
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
            .add_systems(OnEnter(StudioState::Menu), (reset_to_studio, auto_start).chain())
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
        window.title = if Some(main) == entity { title.to_string() } else { crate::i18n::t!("{title} - touch screen", "{title} - ecran tactile") };
        if let Some(scale) = spec.screens.scale {
            let scale = scale.clamp(1, 8) as f32;
            let height = if rows == 2.0 { 2.0 * SCREEN_H as f32 + gap } else { SCREEN_H as f32 };
            window.resolution.set(SCREEN_W as f32 * scale, height * scale + crate::titlebar::TITLE_H);
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
    world.insert_resource(crate::save::GameSaves::default());
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
    // Les sauvegardes du jeu (voir `save`) vont dans son dossier
    world.insert_resource(crate::save::GameSaves::for_game(&id));
    apply_spec(world, &spec, &format!("Nds Game Studio - {}", spec.name));
    if let Err(e) = world.run_system(setup) {
        error!("{id} : setup a echoue : {e}");
    }
}

fn back_to_menu(
    keys: Res<ButtonInput<KeyCode>>,
    input: Res<DsInput>,
    focus: Option<Res<bevy::input_focus::InputFocus>>,
    editors: Query<(), With<crate::devtools::CodeEditor>>,
    mut next: ResMut<NextState<StudioState>>,
) {
    if crate::devtools::typing_in_editor(focus.as_deref(), &editors) {
        return;
    }
    if keys.just_pressed(KeyCode::Escape) || (input.pressed(DsButton::Start) && input.pressed(DsButton::Select)) {
        next.set(StudioState::Menu);
    }
}

/// Texte affiché en bas à gauche du launcher (numéro de version, fourni par l'application).
#[derive(Resource, Default)]
pub struct StudioFooter(pub String);

/// Ligne de commande `nds-game-studio <id>` : le jeu démarre sans passer par le launcher.
pub(crate) fn auto_start(mut commands: Commands, auto: Option<Res<AutoStart>>) {
    if let Some(auto) = auto {
        commands.remove_resource::<AutoStart>();
        commands.insert_resource(ActiveGame(Some(auto.0.clone())));
        commands.set_state(StudioState::Playing);
    }
}
