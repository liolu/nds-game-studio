//! Spécification d'un jeu (game.toml) : ce qui change d'un jeu DS à l'autre.
//!
//! ```toml
//! id = "platformer"
//! name = "Mini Platformer"
//! genre = "plateforme"
//!
//! [screens]
//! main = "top"     # écran qui affiche le moteur principal (top | bottom), échangeable en jeu
//! render = "hd"    # hd (résolution de la fenêtre) | pixel (256x192 agrandi, rendu rétro)
//! scale = 3        # taille de départ des fenêtres (x 256x192)
//! gap = 0          # écart entre les écrans liés, en pixels DS
//! main_mode = "2d" # 2d | 3d (3d = caméra 3D sous la 2D)
//! sub_mode = "2d"
//! main_clear = "#1a1c2c"
//! sub_clear = "#333c57"
//!
//! [input]
//! touch = true     # le jeu utilise le stylet
//! [input.keys]     # touches clavier remplacées (noms KeyCode de Bevy)
//! a = ["KeyX", "Space"]
//!
//! [timing]
//! fps = 60         # cadence de FixedUpdate
//! ```

use std::collections::HashMap;

use bevy::prelude::*;
use serde::Deserialize;

use crate::input::{DsButton, KeyMap};
use crate::screens::{RenderStyle, Screen};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ScreenMode {
    #[default]
    #[serde(rename = "2d")]
    TwoD,
    #[serde(rename = "3d")]
    ThreeD,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct ScreensSpec {
    pub main: Screen,
    pub render: RenderStyle,
    pub scale: u32,
    pub gap: u32,
    pub main_mode: ScreenMode,
    pub sub_mode: ScreenMode,
    pub main_clear: String,
    pub sub_clear: String,
}

impl Default for ScreensSpec {
    fn default() -> Self {
        Self {
            main: Screen::Top,
            render: RenderStyle::Hd,
            scale: crate::DEFAULT_SCALE,
            gap: 0,
            main_mode: ScreenMode::TwoD,
            sub_mode: ScreenMode::TwoD,
            main_clear: "#000000".into(),
            sub_clear: "#000000".into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Default)]
#[serde(default)]
pub struct InputSpec {
    pub touch: bool,
    pub keys: HashMap<DsButton, Vec<String>>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct TimingSpec {
    pub fps: f64,
}

impl Default for TimingSpec {
    fn default() -> Self {
        Self { fps: 60.0 }
    }
}

#[derive(Clone, Debug, Deserialize, Default)]
pub struct GameSpec {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub genre: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub screens: ScreensSpec,
    #[serde(default)]
    pub input: InputSpec,
    #[serde(default)]
    pub timing: TimingSpec,
}

impl GameSpec {
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let spec: GameSpec = toml::from_str(text).map_err(|e| e.to_string())?;
        if spec.id.is_empty() {
            return Err("game.toml : `id` manquant".into());
        }
        Ok(spec)
    }

    /// Couleur de fond d'un moteur (`#rrggbb`), noir si illisible.
    pub fn clear_color(&self, main: bool) -> Color {
        let text = if main { &self.screens.main_clear } else { &self.screens.sub_clear };
        Srgba::hex(text.trim_start_matches('#')).map(Color::from).unwrap_or(Color::BLACK)
    }

    pub fn mode(&self, main: bool) -> ScreenMode {
        if main { self.screens.main_mode } else { self.screens.sub_mode }
    }

    /// Touches de ce jeu : celles par défaut, remplacées par `[input.keys]`.
    pub fn key_map(&self) -> KeyMap {
        let mut map = KeyMap::default();
        for (button, names) in &self.input.keys {
            let keys: Vec<KeyCode> = names.iter().filter_map(|n| key_code(n)).collect();
            if !keys.is_empty() {
                map.0.insert(*button, keys);
            }
        }
        map
    }

    /// Résumé d'une ligne pour le menu.
    pub fn summary(&self) -> String {
        let mode = |m: ScreenMode| if m == ScreenMode::ThreeD { "3D" } else { "2D" };
        format!(
            "principal {} | {}/{} | {} | {} fps",
            if self.screens.main == Screen::Top { "haut" } else { "bas" },
            mode(self.screens.main_mode),
            mode(self.screens.sub_mode),
            if self.input.touch { "stylet" } else { "boutons" },
            self.timing.fps
        )
    }
}

/// Nom de touche Bevy (`KeyX`, `Space`, `ArrowUp`, `Digit1`...) -> KeyCode.
fn key_code(name: &str) -> Option<KeyCode> {
    use KeyCode::*;
    if let Some(letter) = name.strip_prefix("Key").filter(|l| l.len() == 1) {
        const LETTERS: [KeyCode; 26] = [
            KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN, KeyO, KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU,
            KeyV, KeyW, KeyX, KeyY, KeyZ,
        ];
        let c = letter.chars().next()?.to_ascii_uppercase();
        return c.is_ascii_uppercase().then(|| LETTERS[(c as u8 - b'A') as usize]);
    }
    if let Some(digit) = name.strip_prefix("Digit").and_then(|d| d.parse::<usize>().ok()).filter(|d| *d < 10) {
        return Some([Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9][digit]);
    }
    Some(match name {
        "Space" => Space,
        "Enter" => Enter,
        "Backspace" => Backspace,
        "Tab" => Tab,
        "Escape" => Escape,
        "ShiftLeft" => ShiftLeft,
        "ShiftRight" => ShiftRight,
        "ControlLeft" => ControlLeft,
        "ControlRight" => ControlRight,
        "ArrowUp" => ArrowUp,
        "ArrowDown" => ArrowDown,
        "ArrowLeft" => ArrowLeft,
        "ArrowRight" => ArrowRight,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_spec() {
        let spec = GameSpec::from_toml(
            r##"
            id = "demo"
            name = "Demo"
            [screens]
            main = "bottom"
            render = "pixel"
            main_mode = "3d"
            gap = 12
            main_clear = "#ff0000"
            [input]
            touch = true
            [input.keys]
            a = ["Space", "KeyK"]
            [timing]
            fps = 30
            "##,
        )
        .unwrap();
        assert_eq!(spec.screens.main, Screen::Bottom);
        assert_eq!(spec.screens.render, RenderStyle::Pixel);
        assert_eq!(spec.mode(true), ScreenMode::ThreeD);
        assert_eq!(spec.mode(false), ScreenMode::TwoD);
        assert_eq!(spec.screens.gap, 12);
        assert_eq!(spec.screens.scale, crate::DEFAULT_SCALE);
        assert_eq!(spec.timing.fps, 30.0);
        assert_eq!(spec.key_map().0[&DsButton::A], vec![KeyCode::Space, KeyCode::KeyK]);
        assert_eq!(spec.key_map().0[&DsButton::B], vec![KeyCode::KeyZ]);
        assert_eq!(spec.clear_color(true), Color::from(Srgba::rgb(1.0, 0.0, 0.0)));
    }

    #[test]
    fn minimal_spec_uses_ds_defaults() {
        let spec = GameSpec::from_toml("id = \"x\"\nname = \"X\"").unwrap();
        assert_eq!(spec.screens.main, Screen::Top);
        assert_eq!(spec.screens.render, RenderStyle::Hd);
        assert_eq!(spec.timing.fps, 60.0);
        assert!(GameSpec::from_toml("name = \"sans id\"").is_err());
    }
}
