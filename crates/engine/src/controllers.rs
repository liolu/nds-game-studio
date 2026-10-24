//! Manettes : la liste des modèles connus (`crates/engine/controllers/<id>.toml`, format dans
//! `FORMAT.md`), la reconnaissance d'une manette branchée et son dessin dans le menu Options.
//! Sur le dessin, la touche pressée sur la vraie manette s'allume et les sticks bougent.

use std::sync::OnceLock;

use bevy::prelude::*;
use serde::Deserialize;

use crate::input::DsButton;

/// Taille de la toile des dessins (unités du fichier).
pub const CANVAS: Vec2 = Vec2::new(480.0, 300.0);

const FILES: [&str; 9] = [
    include_str!("../controllers/xbox-series.toml"),
    include_str!("../controllers/xbox-360.toml"),
    include_str!("../controllers/dualsense.toml"),
    include_str!("../controllers/dualshock-4.toml"),
    include_str!("../controllers/dualshock-3.toml"),
    include_str!("../controllers/switch-pro.toml"),
    include_str!("../controllers/joycon.toml"),
    include_str!("../controllers/8bitdo-sn30-pro.toml"),
    include_str!("../controllers/generic.toml"),
];

#[derive(Deserialize, Debug, Clone)]
pub struct Shape {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    #[serde(default)]
    pub radius: f32,
    pub color: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ButtonDef {
    pub input: String,
    #[serde(default)]
    pub label: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    #[serde(default)]
    pub radius: f32,
    pub color: String,
    #[serde(default = "white")]
    pub label_color: String,
}

fn white() -> String {
    "#ffffff".into()
}

#[derive(Deserialize, Debug, Clone)]
pub struct StickDef {
    pub side: String,
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub color: String,
    pub cap_color: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ControllerDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(default)]
    pub vendor_ids: Vec<u16>,
    #[serde(default)]
    pub product_ids: Vec<u16>,
    #[serde(default, rename = "part")]
    pub parts: Vec<Shape>,
    #[serde(default, rename = "button")]
    pub buttons: Vec<ButtonDef>,
    #[serde(default, rename = "stick")]
    pub sticks: Vec<StickDef>,
}

/// Tous les modèles connus (le générique en dernier).
pub fn controllers() -> &'static [ControllerDef] {
    static LIST: OnceLock<Vec<ControllerDef>> = OnceLock::new();
    LIST.get_or_init(|| {
        FILES
            .iter()
            .filter_map(|text| match toml::from_str::<ControllerDef>(text) {
                Ok(def) => Some(def),
                Err(e) => {
                    error!("fichier de manette illisible : {e}");
                    None
                }
            })
            .collect()
    })
}

pub fn by_id(id: &str) -> Option<&'static ControllerDef> {
    controllers().iter().find(|c| c.id == id)
}

/// Modèle d'une manette branchée : identifiants USB d'abord, puis nom donné par le pilote, puis
/// fabricant ; le générique sinon.
pub fn detect(name: &str, vendor: Option<u16>, product: Option<u16>) -> &'static ControllerDef {
    let all = controllers();
    let name = name.to_lowercase();
    let by_product = all.iter().find(|c| vendor.is_some_and(|v| c.vendor_ids.contains(&v)) && product.is_some_and(|p| c.product_ids.contains(&p)));
    let by_name = || all.iter().filter_map(|c| c.names.iter().filter(|n| !n.is_empty() && name.contains(n.as_str())).map(|n| (n.len(), c)).max_by_key(|(len, _)| *len)).max_by_key(|(len, _)| *len).map(|(_, c)| c);
    let by_vendor = || all.iter().find(|c| c.id != "generic" && vendor.is_some_and(|v| c.vendor_ids.contains(&v)));
    by_product.or_else(by_name).or_else(by_vendor).or_else(|| by_id("generic")).unwrap_or(&all[0])
}

/// Nom de touche Bevy (`South`, `DPadUp`...) -> touche.
pub fn pad_button(name: &str) -> Option<GamepadButton> {
    use GamepadButton::*;
    Some(match name {
        "South" => South,
        "East" => East,
        "North" => North,
        "West" => West,
        "C" => C,
        "Z" => Z,
        "LeftTrigger" => LeftTrigger,
        "LeftTrigger2" => LeftTrigger2,
        "RightTrigger" => RightTrigger,
        "RightTrigger2" => RightTrigger2,
        "Select" => Select,
        "Start" => Start,
        "Mode" => Mode,
        "LeftThumb" => LeftThumb,
        "RightThumb" => RightThumb,
        "DPadUp" => DPadUp,
        "DPadDown" => DPadDown,
        "DPadLeft" => DPadLeft,
        "DPadRight" => DPadRight,
        _ => return None,
    })
}

/// Bouton DS commandé par cette touche de manette (voir `input`).
pub fn ds_button_for(button: GamepadButton) -> Option<DsButton> {
    DsButton::ALL.into_iter().find(|ds| crate::input::pad_buttons_of(*ds).contains(&button))
}

fn color(hex: &str) -> Color {
    Srgba::hex(hex.trim_start_matches('#')).map(Color::from).unwrap_or(Color::srgb(0.5, 0.5, 0.5))
}

/// Plus clair (touche allumée).
fn lit(c: Color) -> Color {
    c.mix(&Color::WHITE, 0.55)
}

// ─────────────────────────────────────────────────────────────────────────
//  Dessin
// ─────────────────────────────────────────────────────────────────────────

#[derive(Component)]
pub struct PadButtonView {
    pub input: GamepadButton,
    pub label: String,
    base: Color,
}

#[derive(Component)]
pub struct PadStickCap {
    pub right: bool,
    /// Débattement du chapeau (pixels).
    travel: f32,
    base: Color,
}

fn boxed(x: f32, y: f32, w: f32, h: f32, radius: f32, scale: f32) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: px(x * scale),
        top: px(y * scale),
        width: px(w * scale),
        height: px(h * scale),
        border_radius: BorderRadius::all(px(radius.min(w.min(h) / 2.0) * scale)),
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        ..default()
    }
}

/// Dessine la manette (toile de 480x300 x `scale`) comme enfant de `parent`.
pub fn spawn_drawing(parent: &mut ChildSpawnerCommands, def: &ControllerDef, scale: f32) {
    parent
        .spawn(Node { width: px(CANVAS.x * scale), height: px(CANVAS.y * scale), flex_shrink: 0.0, ..default() })
        .with_children(|canvas| {
            for part in &def.parts {
                canvas.spawn((boxed(part.x, part.y, part.w, part.h, part.radius, scale), BackgroundColor(color(&part.color))));
            }
            for stick in &def.sticks {
                let base = color(&stick.cap_color);
                let cap = stick.size * 0.62;
                let inset = (stick.size - cap) / 2.0;
                canvas
                    .spawn((boxed(stick.x, stick.y, stick.size, stick.size, stick.size / 2.0, scale), BackgroundColor(color(&stick.color))))
                    .with_child((
                        boxed(inset, inset, cap, cap, cap / 2.0, scale),
                        BackgroundColor(base),
                        BorderColor::all(Color::NONE),
                        PadStickCap { right: stick.side == "right", travel: inset * scale, base },
                    ));
            }
            for button in &def.buttons {
                let Some(input) = pad_button(&button.input) else { continue };
                let base = color(&button.color);
                let size = (button.h * 0.55).clamp(7.0, 14.0) * scale;
                let mut node = boxed(button.x, button.y, button.w, button.h, button.radius, scale);
                node.border = UiRect::all(px(2));
                // Button : un clic sur la touche dessinée affiche son nom (menu Options)
                canvas.spawn((node, bevy::ui_widgets::Button, BackgroundColor(base), BorderColor::all(Color::NONE), PadButtonView { input, label: button.label.clone(), base })).with_child((
                    Text::new(button.label.clone()),
                    TextFont::from_font_size(size),
                    TextColor(color(&button.label_color)),
                ));
            }
        });
}

/// La manette qui anime le dessin : la première branchée.
pub fn first_gamepad(pads: &Query<(Entity, &Gamepad, Option<&Name>)>) -> Option<Entity> {
    pads.iter().map(|(e, ..)| e).min()
}

/// Touches allumées et sticks qui bougent, d'après la manette branchée.
pub fn animate_drawing(
    pads: Query<(Entity, &Gamepad, Option<&Name>)>,
    mut buttons: Query<(&PadButtonView, &mut BackgroundColor, &mut BorderColor), Without<PadStickCap>>,
    mut caps: Query<(&PadStickCap, &mut Node, &mut BackgroundColor, &mut BorderColor), Without<PadButtonView>>,
) {
    let pad = first_gamepad(&pads).and_then(|e| pads.get(e).ok()).map(|(_, g, _)| g);
    for (view, mut background, mut border) in &mut buttons {
        let pressed = pad.is_some_and(|g| g.pressed(view.input));
        let (bg, bd) = if pressed { (lit(view.base), Color::WHITE) } else { (view.base, Color::NONE) };
        if background.0 != bg {
            background.0 = bg;
        }
        let wanted = BorderColor::all(bd);
        if *border != wanted {
            *border = wanted;
        }
    }
    for (cap, mut node, mut background, mut border) in &mut caps {
        let (stick, thumb) = match pad {
            Some(g) if cap.right => (g.right_stick(), g.pressed(GamepadButton::RightThumb)),
            Some(g) => (g.left_stick(), g.pressed(GamepadButton::LeftThumb)),
            None => (Vec2::ZERO, false),
        };
        let offset = stick.clamp_length_max(1.0) * cap.travel;
        let (left, top) = (px(cap.travel + offset.x), px(cap.travel - offset.y));
        if node.left != left || node.top != top {
            node.left = left;
            node.top = top;
        }
        let bg = if thumb { lit(cap.base) } else if stick.length() > 0.2 { cap.base.mix(&Color::WHITE, 0.25) } else { cap.base };
        if background.0 != bg {
            background.0 = bg;
        }
        let wanted = BorderColor::all(if thumb { Color::WHITE } else { Color::NONE });
        if *border != wanted {
            *border = wanted;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_controller_file_is_valid_and_drawn_inside_the_canvas() {
        assert_eq!(controllers().len(), FILES.len(), "un fichier de manette ne se lit pas");
        for c in controllers() {
            assert!(!c.parts.is_empty() && !c.buttons.is_empty(), "{} : rien a dessiner", c.id);
            for b in &c.buttons {
                assert!(pad_button(&b.input).is_some(), "{} : touche inconnue {}", c.id, b.input);
                assert!(b.x >= 0.0 && b.y >= 0.0 && b.x + b.w <= CANVAS.x && b.y + b.h <= CANVAS.y, "{} : {} hors de la toile", c.id, b.input);
            }
            for s in &c.sticks {
                assert!(s.side == "left" || s.side == "right", "{} : stick {}", c.id, s.side);
            }
            for want in ["South", "East", "West", "North", "Start", "DPadUp"] {
                assert!(c.buttons.iter().any(|b| b.input == want), "{} : il manque {want}", c.id);
            }
        }
    }

    #[test]
    fn plugged_controllers_are_recognised() {
        assert_eq!(detect("Xbox Wireless Controller", Some(0x045E), Some(0x0B12)).id, "xbox-series");
        assert_eq!(detect("Xbox 360 Controller", Some(0x045E), Some(0x028E)).id, "xbox-360");
        assert_eq!(detect("DualSense Wireless Controller", Some(0x054C), Some(0x0CE6)).id, "dualsense");
        assert_eq!(detect("Wireless Controller", Some(0x054C), Some(0x05C4)).id, "dualshock-4");
        assert_eq!(detect("Pro Controller", Some(0x057E), Some(0x2009)).id, "switch-pro");
        assert_eq!(detect("Unknown pad", Some(0x1234), Some(0x0001)).id, "generic");
        assert_eq!(ds_button_for(GamepadButton::East), Some(DsButton::A));
    }
}
