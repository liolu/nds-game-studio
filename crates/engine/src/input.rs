//! Entrées DS : les 12 boutons et le stylet.
//!
//! L'état est relu à chaque pas de `FixedUpdate` (la cadence du jeu, 60 Hz par défaut) : la logique
//! des jeux tourne dans `FixedUpdate` et voit chaque appui une seule fois, comme sur la console.
//! Clavier (positions physiques, donc pareil en AZERTY et QWERTY) et manettes (disposition
//! Nintendo : bouton est = A, sud = B) sont lus ensemble. Le stylet = clic gauche sur l'écran du bas.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::screens::{Screen, ScreenPlaces};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DsButton {
    A,
    B,
    X,
    Y,
    L,
    R,
    Start,
    Select,
    Up,
    Down,
    Left,
    Right,
}

impl DsButton {
    pub const ALL: [DsButton; 12] = [
        DsButton::A,
        DsButton::B,
        DsButton::X,
        DsButton::Y,
        DsButton::L,
        DsButton::R,
        DsButton::Start,
        DsButton::Select,
        DsButton::Up,
        DsButton::Down,
        DsButton::Left,
        DsButton::Right,
    ];

    fn bit(self) -> u16 {
        1 << self as u16
    }

    fn default_keys(self) -> &'static [KeyCode] {
        use KeyCode::*;
        match self {
            DsButton::A => &[KeyX],
            DsButton::B => &[KeyZ],
            DsButton::X => &[KeyS],
            DsButton::Y => &[KeyA],
            DsButton::L => &[KeyQ],
            DsButton::R => &[KeyW],
            DsButton::Start => &[Enter],
            DsButton::Select => &[ShiftRight, Backspace],
            DsButton::Up => &[ArrowUp],
            DsButton::Down => &[ArrowDown],
            DsButton::Left => &[ArrowLeft],
            DsButton::Right => &[ArrowRight],
        }
    }

    pub(crate) fn pad_buttons(self) -> &'static [GamepadButton] {
        use GamepadButton::*;
        match self {
            DsButton::A => &[East],
            DsButton::B => &[South],
            DsButton::X => &[North],
            DsButton::Y => &[West],
            DsButton::L => &[LeftTrigger],
            DsButton::R => &[RightTrigger],
            DsButton::Start => &[Start],
            DsButton::Select => &[Select],
            DsButton::Up => &[DPadUp],
            DsButton::Down => &[DPadDown],
            DsButton::Left => &[DPadLeft],
            DsButton::Right => &[DPadRight],
        }
    }
}

/// Touches de manette qui commandent ce bouton DS.
pub(crate) fn pad_buttons_of(button: DsButton) -> &'static [GamepadButton] {
    button.pad_buttons()
}

/// Touches du clavier pour chaque bouton (la spécification du jeu peut les changer).
#[derive(Resource, Clone)]
pub struct KeyMap(pub HashMap<DsButton, Vec<KeyCode>>);

impl Default for KeyMap {
    fn default() -> Self {
        Self(DsButton::ALL.iter().map(|b| (*b, b.default_keys().to_vec())).collect())
    }
}

/// État des boutons, mis à jour à chaque pas de jeu.
#[derive(Resource, Default)]
pub struct DsInput {
    held: u16,
    previous: u16,
}

impl DsInput {
    pub fn pressed(&self, button: DsButton) -> bool {
        self.held & button.bit() != 0
    }

    pub fn just_pressed(&self, button: DsButton) -> bool {
        self.held & !self.previous & button.bit() != 0
    }

    pub fn just_released(&self, button: DsButton) -> bool {
        !self.held & self.previous & button.bit() != 0
    }

    /// Croix directionnelle : x = droite - gauche, y = haut - bas.
    pub fn dpad(&self) -> Vec2 {
        let axis = |plus, minus| self.pressed(plus) as i32 as f32 - self.pressed(minus) as i32 as f32;
        Vec2::new(axis(DsButton::Right, DsButton::Left), axis(DsButton::Up, DsButton::Down))
    }
}

/// Stylet sur l'écran du bas, en pixels DS (0,0 en haut à gauche de la zone 256x192 ; en format
/// large, x peut sortir de 0..256 sur les côtés).
#[derive(Resource, Default)]
pub struct Stylus {
    /// Dernière position du stylet sur l'écran (même relevé).
    pub position: Vec2,
    down: bool,
    was_down: bool,
}

impl Stylus {
    pub fn down(&self) -> bool {
        self.down
    }

    pub fn just_touched(&self) -> bool {
        self.down && !self.was_down
    }

    pub fn just_released(&self) -> bool {
        !self.down && self.was_down
    }

    /// Position si le stylet touche l'écran.
    pub fn touch(&self) -> Option<Vec2> {
        self.down.then_some(self.position)
    }
}

pub struct InputPlugin;

impl Plugin for InputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<KeyMap>()
            .init_resource::<DsInput>()
            .init_resource::<Stylus>()
            .add_systems(FixedPreUpdate, (read_buttons, read_stylus));
    }
}

fn read_buttons(
    map: Res<KeyMap>,
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    focus: Option<Res<bevy::input_focus::InputFocus>>,
    editors: Query<(), With<crate::devtools::CodeEditor>>,
    mut input: ResMut<DsInput>,
) {
    // On tape dans l'éditeur de code (F5) : le clavier n'est pas au jeu
    let typing = crate::devtools::typing_in_editor(focus.as_deref(), &editors);
    let mut held = 0;
    for button in DsButton::ALL {
        let key = !typing && map.0.get(&button).is_some_and(|k| keys.any_pressed(k.iter().copied()));
        let pad = pads.iter().any(|p| p.any_pressed(button.pad_buttons().iter().copied()));
        // Le stick gauche sert aussi de croix
        let stick = pads.iter().any(|p| {
            let s = p.left_stick();
            match button {
                DsButton::Up => s.y > 0.5,
                DsButton::Down => s.y < -0.5,
                DsButton::Left => s.x < -0.5,
                DsButton::Right => s.x > 0.5,
                _ => false,
            }
        });
        if key || pad || stick {
            held |= button.bit();
        }
    }
    input.previous = input.held;
    input.held = held;
}

fn read_stylus(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    places: Res<ScreenPlaces>,
    windows: Query<&Window>,
    mut stylus: ResMut<Stylus>,
) {
    stylus.was_down = stylus.down;
    let Some(place) = places.get(Screen::Bottom) else { return };
    let Ok(window) = windows.get(place.window) else { return };
    let alt = keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]); // Alt + clic = déplacer la fenêtre
    let inside = window.cursor_position().and_then(|cursor| place.to_ds(cursor));
    match inside {
        Some(p) => {
            stylus.position = p;
            stylus.down = buttons.pressed(MouseButton::Left) && !alt;
        }
        None => stylus.down = false,
    }
}
