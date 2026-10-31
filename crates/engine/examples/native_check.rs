//! Essai sans prendre le clavier : lance un jeu natif, sans focus et muet, simule des commandes, enregistre une
//! capture et ferme.
//! `cargo run --release -p nds-engine --example native_check -- <code> <rom> <capture.png> <secondes> [enter@s] [touch@s]`
//! (avec `NDSGS_NATIVE_MUTE=1` et `NDSGS_NATIVE_DIR`)

use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use nds_engine::prelude::*;

#[derive(Resource)]
struct Check {
    capture: String,
    at: f32,
    done: bool,
    enter_at: Option<f32>,
    touch_at: Option<f32>,
}

fn no_focus(mut windows: Query<&mut Window, Added<Window>>) {
    for mut window in &mut windows {
        window.focused = false;
    }
}

fn simulate(
    time: Res<Time>,
    check: Res<Check>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut windows: Query<&mut Window>,
) {
    let now = time.elapsed_secs();
    let during = |at: Option<f32>| at.is_some_and(|t| now > t && now < t + 0.4);
    if during(check.enter_at) {
        keys.press(KeyCode::Enter);
    } else {
        keys.release(KeyCode::Enter);
    }
    if during(check.touch_at) {
        // Milieu de l'écran du bas, dans la fenêtre où les deux écrans sont fusionnés
        for mut window in &mut windows {
            let (w, h) = (window.physical_width() as f64, window.physical_height() as f64);
            window.set_physical_cursor_position(Some(bevy::math::DVec2::new(w / 2.0, h * 0.75)));
        }
        mouse.press(MouseButton::Left);
    } else {
        mouse.release(MouseButton::Left);
    }
}

fn capture(mut commands: Commands, time: Res<Time>, mut check: ResMut<Check>, mut exit: MessageWriter<AppExit>) {
    if !check.done && time.elapsed_secs() > check.at {
        check.done = true;
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(check.capture.clone()));
    }
    if check.done && time.elapsed_secs() > check.at + 2.0 {
        exit.write(AppExit::Success);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (code, rom, shot) = (&args[1], &args[2], &args[3]);
    let secs = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(20.0);
    let time_of = |prefix: &str| args.iter().find_map(|a| a.strip_prefix(prefix)?.parse::<f32>().ok());
    let mut app = App::new();
    app.add_plugins(NdsEnginePlugin::default());
    // Le moteur lit la ligne de commande : ces arguments ne sont pas pour lui.
    // Le code "launcher" montre le launcher au lieu de lancer un jeu.
    app.world_mut().remove_resource::<AutoStart>();
    if code != "launcher" {
        app.insert_resource(AutoStart(format!("native-{code}")));
    }
    app
        .insert_resource(ActiveRom(rom.into()))
        .insert_resource(Check {
            capture: shot.clone(),
            at: secs,
            done: false,
            enter_at: time_of("enter@"),
            touch_at: time_of("touch@"),
        })
        .add_systems(PreUpdate, simulate.after(bevy::input::InputSystems))
        .add_systems(Update, (no_focus, capture))
        .run();
}
