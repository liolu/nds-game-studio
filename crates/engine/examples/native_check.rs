//! Essai sans prendre le clavier : lance un jeu natif, sans focus et muet, simule des commandes, enregistre une
//! capture et ferme.
//! `cargo run --release -p nds-engine --example native_check -- <code> <rom> <capture.png> <secondes> [enter@s] [touch@s] [f8@s] [c@s] [v@s]`
//! `click=texte@s` active le bouton qui porte ce texte (le dernier s'il y en a plusieurs) ; une capture par fenetre.
//! `drop=chemin@s` depose un fichier ou un dossier sur la fenetre ; `type=texte@s` tape ce texte au clavier.
//! `press=x,y@s`, `moveto=x,y@s`, `release=x,y@s` : la souris dans la fenetre, sans toucher au vrai curseur.
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
    touch_at: Vec<f32>,
    f8_at: Option<f32>,
    c_at: Vec<f32>,
    v_at: Vec<f32>,
    clicks: Vec<(String, f32, bool)>,
    drops: Vec<(String, f32, bool)>,
    typed: Vec<(String, f32, bool)>,
    mouse: Vec<(MouseStep, Vec2, f32, bool)>,
}

#[derive(Clone, Copy, PartialEq)]
enum MouseStep {
    Press,
    Move,
    Release,
}

/// Les memes messages que winit : le picking lit `WindowEvent`, `ButtonInput` lit `MouseButtonInput`.
fn mouse_steps(time: Res<Time>, mut check: ResMut<Check>, mut windows: Query<(Entity, &mut Window)>, mut events: MessageWriter<bevy::window::WindowEvent>, mut buttons: MessageWriter<bevy::input::mouse::MouseButtonInput>, mut moves: MessageWriter<bevy::window::CursorMoved>) {
    use bevy::input::ButtonState;
    use bevy::input::mouse::MouseButtonInput;
    use bevy::window::{CursorMoved, WindowEvent};
    let now = time.elapsed_secs();
    let Some((window, mut state)) = windows.iter_mut().next() else { return };
    for (step, at, when, done) in &mut check.mouse {
        if *done || now < *when {
            continue;
        }
        *done = true;
        state.bypass_change_detection().set_cursor_position(Some(*at));
        let moved = CursorMoved { window, position: *at, delta: None };
        moves.write(moved.clone());
        events.write(WindowEvent::CursorMoved(moved));
        if *step != MouseStep::Move {
            let input = MouseButtonInput { button: MouseButton::Left, state: if *step == MouseStep::Press { ButtonState::Pressed } else { ButtonState::Released }, window };
            buttons.write(input);
            events.write(WindowEvent::MouseButtonInput(input));
        }
    }
}

/// Les fichiers deposes et les touches passent par les memes messages que les vrais.
fn drops_and_keys(time: Res<Time>, mut check: ResMut<Check>, windows: Query<Entity, With<Window>>, mut drops: MessageWriter<bevy::window::FileDragAndDrop>, mut keys: MessageWriter<bevy::input::keyboard::KeyboardInput>) {
    let now = time.elapsed_secs();
    let Some(window) = windows.iter().next() else { return };
    for (path, at, done) in &mut check.drops {
        if !*done && now >= *at {
            *done = true;
            drops.write(bevy::window::FileDragAndDrop::DroppedFile { window, path_buf: path.as_str().into() });
        }
    }
    for (text, at, done) in &mut check.typed {
        if !*done && now >= *at {
            *done = true;
            for c in text.chars() {
                keys.write(bevy::input::keyboard::KeyboardInput {
                    key_code: KeyCode::KeyA,
                    logical_key: bevy::input::keyboard::Key::Character(c.to_string().into()),
                    state: bevy::input::ButtonState::Pressed,
                    text: Some(c.to_string().into()),
                    repeat: false,
                    window,
                });
            }
        }
    }
}

fn clicks(
    mut commands: Commands,
    time: Res<Time>,
    mut check: ResMut<Check>,
    texts: Query<(Entity, &Text)>,
    parents: Query<&ChildOf>,
    buttons: Query<(), With<bevy::ui_widgets::Button>>,
) {
    let now = time.elapsed_secs();
    for (wanted, at, done) in &mut check.clicks {
        if *done || now < *at {
            continue;
        }
        *done = true;
        let found = texts.iter().filter(|(_, t)| t.0.contains(wanted.as_str())).filter_map(|(e, _)| {
            std::iter::once(e).chain(parents.iter_ancestors(e)).find(|a| buttons.contains(*a))
        });
        match found.last() {
            Some(entity) => commands.trigger(bevy::ui_widgets::Activate { entity }),
            None => println!("pas de bouton : {wanted}"),
        }
    }
}

fn no_focus(mut windows: Query<&mut Window, Added<Window>>) {
    for mut window in &mut windows {
        window.focused = false;
        // Hors de l'ecran : l'essai ne gene pas le joueur
        window.position = WindowPosition::At(IVec2::new(-4000, 100));
        let height = std::env::var("NDSGS_CHECK_HEIGHT").ok().and_then(|h| h.parse().ok()).unwrap_or(800.0);
        window.resolution.set(1280.0, height);
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
    if during(check.f8_at) {
        keys.press(KeyCode::F8);
    } else {
        keys.release(KeyCode::F8);
    }
    for (at, key) in [(&check.c_at, KeyCode::KeyC), (&check.v_at, KeyCode::KeyV)] {
        if at.iter().any(|t| during(Some(*t))) {
            keys.press(key);
        } else {
            keys.release(key);
        }
    }
    if check.touch_at.iter().any(|t| during(Some(*t))) {
        // Milieu de l'écran du bas, dans la fenêtre où les deux écrans sont fusionnés
        for mut window in &mut windows {
            let (w, h) = (window.physical_width() as f64, window.physical_height() as f64);
            window.set_physical_cursor_position(Some(bevy::math::DVec2::new(w / 2.0, h * 0.75)));
        }
        mouse.press(MouseButton::Left);
    } else if !check.touch_at.is_empty() {
        mouse.release(MouseButton::Left);
    }
}

fn capture(mut commands: Commands, time: Res<Time>, mut check: ResMut<Check>, mut exit: MessageWriter<AppExit>, windows: Query<Entity, With<Window>>) {
    if !check.done && time.elapsed_secs() > check.at {
        check.done = true;
        for (i, window) in windows.iter().enumerate() {
            let path = if i == 0 { check.capture.clone() } else { check.capture.replace(".png", &format!("-{i}.png")) };
            commands.spawn(Screenshot::window(window)).observe(save_to_disk(path));
        }
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
            touch_at: args.iter().filter_map(|a| a.strip_prefix("touch@")?.parse().ok()).collect(),
            f8_at: time_of("f8@"),
            c_at: args.iter().filter_map(|a| a.strip_prefix("c@")?.parse().ok()).collect(),
            v_at: args.iter().filter_map(|a| a.strip_prefix("v@")?.parse().ok()).collect(),
            clicks: args.iter().filter_map(|a| a.strip_prefix("click=")).filter_map(|a| a.rsplit_once('@')).filter_map(|(t, s)| Some((t.to_string(), s.parse().ok()?, false))).collect(),
            drops: args.iter().filter_map(|a| a.strip_prefix("drop=")).filter_map(|a| a.rsplit_once('@')).filter_map(|(t, s)| Some((t.to_string(), s.parse().ok()?, false))).collect(),
            typed: args.iter().filter_map(|a| a.strip_prefix("type=")).filter_map(|a| a.rsplit_once('@')).filter_map(|(t, s)| Some((t.to_string(), s.parse().ok()?, false))).collect(),
            mouse: args.iter().filter_map(|a| {
                let (step, rest) = a.split_once('=')?;
                let step = match step {
                    "press" => MouseStep::Press,
                    "moveto" => MouseStep::Move,
                    "release" => MouseStep::Release,
                    _ => return None,
                };
                let (at, when) = rest.rsplit_once('@')?;
                let (x, y) = at.split_once(',')?;
                Some((step, Vec2::new(x.parse().ok()?, y.parse().ok()?), when.parse().ok()?, false))
            }).collect(),
        })
        .add_systems(PreUpdate, simulate.after(bevy::input::InputSystems))
        .add_systems(Update, (no_focus, capture, clicks, drops_and_keys, mouse_steps))
        .run();
}
