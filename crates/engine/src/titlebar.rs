//! Barre de titre du studio, dessinée par le moteur à la place de celle de Windows : titre, puis
//! les boutons options, fusionner / séparer les écrans, réduire, agrandir et fermer.
//!
//! Windows ne permet pas d'ajouter un bouton à côté de « réduire » dans sa propre barre : les
//! fenêtres du studio sont donc sans bordure système et le moteur refait ce que fait Windows :
//! - glisser la barre déplace la fenêtre (avec l'aimantation aux bords de l'écran de Windows),
//!   double-clic agrandit / restaure ;
//! - les bords et les coins redimensionnent la fenêtre (curseur de redimensionnement au survol) ;
//! - ombre de fenêtre gardée.

use bevy::ecs::system::NonSendMarker;
use bevy::math::CompassOctant;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::Pressed;
use bevy::ui_widgets::{Activate, Button};
use bevy::window::{CursorIcon, SystemCursorIcon};
use bevy::winit::WINIT_WINDOWS;

use crate::screens::{DisplayCamera, ScreenLayout};

/// Hauteur de la barre de titre (pixels logiques), comme celle de Windows 11.
pub const TITLE_H: f32 = 32.0;
const BUTTON_W: f32 = 46.0;
/// Largeur de la zone des bords qui redimensionne.
const EDGE: f32 = 6.0;
const DOUBLE_CLICK: f64 = 0.4;

const BAR: Color = Color::srgb(0.125, 0.125, 0.14);
const HOVER: Color = Color::srgb(0.22, 0.22, 0.25);
const PRESS: Color = Color::srgb(0.3, 0.3, 0.34);
const CLOSE_HOVER: Color = Color::srgb(0.77, 0.17, 0.11);
const ICON: Color = Color::srgb(0.92, 0.92, 0.94);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    Options,
    Fusion,
    Minimize,
    Maximize,
    Close,
}

/// Barre de titre d'une fenêtre.
#[derive(Component)]
struct TitleBar(Entity);

#[derive(Component)]
struct TitleText(Entity);

#[derive(Component)]
struct TitleButton {
    window: Entity,
    action: Action,
}

/// Icône du bouton fusion : deux écrans collés (fusionnés) ou écartés (séparés).
#[derive(Component)]
struct FusionIcon;

pub struct TitleBarPlugin;

impl Plugin for TitleBarPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (spawn_title_bars, despawn_orphans, refresh_title_bars, window_chrome, frame_style))
            .add_systems(Update, button_looks);
    }
}

fn spawn_title_bars(mut commands: Commands, cameras: Query<(Entity, &DisplayCamera), Added<DisplayCamera>>) {
    for (camera, display) in &cameras {
        let window = display.0;
        commands
            .spawn((
                Node { width: percent(100), height: px(TITLE_H), flex_direction: FlexDirection::Row, align_items: AlignItems::Center, ..default() },
                BackgroundColor(BAR),
                UiTargetCamera(camera),
                TitleBar(window),
            ))
            .with_children(|bar| {
                bar.spawn((
                    Text::new(""),
                    TextFont::from_font_size(13.0),
                    TextColor(ICON),
                    Node { flex_grow: 1.0, margin: UiRect::left(px(12)), overflow: Overflow::clip(), ..default() },
                    TitleText(window),
                ));
                for action in [Action::Options, Action::Fusion, Action::Minimize, Action::Maximize, Action::Close] {
                    bar.spawn((
                        Button,
                        Hovered::default(),
                        Node { width: px(BUTTON_W), height: px(TITLE_H), justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() },
                        BackgroundColor(BAR),
                        TitleButton { window, action },
                    ))
                    .observe(click)
                    .with_children(|b| icon(b, action));
                }
            });
    }
}

/// Icônes dessinées avec des rectangles (la police par défaut n'a pas les symboles de Windows).
fn icon(b: &mut ChildSpawnerCommands, action: Action) {
    let line = |w: f32, h: f32| (Node { width: px(w), height: px(h), ..default() }, BackgroundColor(ICON));
    let frame = |w: f32, h: f32| (Node { width: px(w), height: px(h), border: UiRect::all(px(1)), ..default() }, BorderColor::all(ICON));
    match action {
        Action::Minimize => {
            b.spawn(line(10.0, 1.0));
        }
        Action::Maximize => {
            b.spawn(frame(10.0, 10.0));
        }
        Action::Close => {
            b.spawn((Text::new("X"), TextFont::from_font_size(13.0), TextColor(ICON)));
        }
        Action::Options => {
            // Trois traits (menu)
            b.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(3), ..default() }).with_children(|c| {
                for _ in 0..3 {
                    c.spawn(line(12.0, 1.5));
                }
            });
        }
        Action::Fusion => {
            b.spawn((Node { flex_direction: FlexDirection::Column, align_items: AlignItems::Center, row_gap: px(0), ..default() }, FusionIcon))
                .with_children(|c| {
                    c.spawn(frame(12.0, 7.0));
                    c.spawn(frame(12.0, 7.0));
                });
        }
    }
}

fn click(
    activate: On<Activate>,
    buttons: Query<&TitleButton>,
    mut layout: ResMut<ScreenLayout>,
    mut windows: Query<&mut Window>,
    mut exit: MessageWriter<AppExit>,
    mut options: MessageWriter<crate::options::ToggleOptions>,
    _main_thread: NonSendMarker,
) {
    let Ok(button) = buttons.get(activate.entity) else { return };
    debug!("barre de titre : bouton {:?}", button.action);
    match button.action {
        Action::Options => {
            options.write(crate::options::ToggleOptions);
        }
        Action::Fusion => {
            layout.merged = !layout.merged;
            info!("bouton fusion : ecrans {}", if layout.merged { "fusionnes" } else { "separes" });
        }
        Action::Minimize => {
            if let Ok(mut window) = windows.get_mut(button.window) {
                window.set_minimized(true);
            }
        }
        Action::Maximize => toggle_maximized(button.window, &mut windows),
        Action::Close => {
            exit.write(AppExit::Success);
        }
    }
}

fn is_maximized(window: Entity) -> bool {
    WINIT_WINDOWS.with_borrow(|w| w.get_window(window).is_some_and(|w| w.is_maximized()))
}

fn toggle_maximized(window: Entity, windows: &mut Query<&mut Window>) {
    let maximized = is_maximized(window);
    if let Ok(mut w) = windows.get_mut(window) {
        w.set_maximized(!maximized);
    }
}

/// Fenêtre fermée (écran du bas refusionné) : sa barre de titre part avec elle.
fn despawn_orphans(mut commands: Commands, bars: Query<(Entity, &TitleBar)>, windows: Query<(), With<Window>>) {
    for (bar, title) in &bars {
        if windows.get(title.0).is_err() {
            commands.entity(bar).despawn();
        }
    }
}

fn refresh_title_bars(
    layout: Res<ScreenLayout>,
    windows: Query<&Window>,
    mut texts: Query<(&TitleText, &mut Text)>,
    mut icons: Query<&mut Node, With<FusionIcon>>,
) {
    for (title, mut text) in &mut texts {
        if let Ok(window) = windows.get(title.0)
            && text.0 != window.title
        {
            text.0 = window.title.clone();
        }
    }
    // Fusionnés : les deux écrans collés ; séparés : écartés
    let gap = if layout.merged { 0.0 } else { 3.0 };
    for mut node in &mut icons {
        if node.row_gap != px(gap) {
            node.row_gap = px(gap);
        }
    }
}

fn button_looks(mut buttons: Query<(&TitleButton, &Hovered, Has<Pressed>, &mut BackgroundColor)>) {
    for (button, hovered, pressed, mut color) in &mut buttons {
        let wanted = match (button.action, hovered.get(), pressed) {
            (Action::Close, true, _) => CLOSE_HOVER,
            (_, _, true) => PRESS,
            (_, true, false) => HOVER,
            _ => BAR,
        };
        if color.0 != wanted {
            color.0 = wanted;
        }
    }
}

/// Bord ou coin sous le curseur (pixels logiques de la fenêtre).
fn edge_at(cursor: Vec2, size: Vec2) -> Option<CompassOctant> {
    let (left, right) = (cursor.x < EDGE, cursor.x > size.x - EDGE);
    let (top, bottom) = (cursor.y < EDGE, cursor.y > size.y - EDGE);
    Some(match (left, right, top, bottom) {
        (true, _, true, _) => CompassOctant::NorthWest,
        (_, true, true, _) => CompassOctant::NorthEast,
        (true, _, _, true) => CompassOctant::SouthWest,
        (_, true, _, true) => CompassOctant::SouthEast,
        (true, ..) => CompassOctant::West,
        (_, true, ..) => CompassOctant::East,
        (_, _, true, _) => CompassOctant::North,
        (_, _, _, true) => CompassOctant::South,
        _ => return None,
    })
}

fn resize_cursor(octant: CompassOctant) -> SystemCursorIcon {
    match octant {
        CompassOctant::North | CompassOctant::South => SystemCursorIcon::NsResize,
        CompassOctant::East | CompassOctant::West => SystemCursorIcon::EwResize,
        CompassOctant::NorthWest | CompassOctant::SouthEast => SystemCursorIcon::NwseResize,
        CompassOctant::NorthEast | CompassOctant::SouthWest => SystemCursorIcon::NeswResize,
    }
}

/// Déplacer par la barre, redimensionner par les bords, double-clic pour agrandir.
fn window_chrome(
    mut commands: Commands,
    time: Res<Time<Real>>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    bars: Query<&TitleBar>,
    title_buttons: Query<(&TitleButton, &Hovered)>,
    mut windows: Query<(Entity, &mut Window, Option<&CursorIcon>)>,
    mut last_click: Local<Option<(Entity, f64)>>,
    _main_thread: NonSendMarker,
) {
    let windowed: Vec<Entity> = bars.iter().map(|b| b.0).collect();
    let mut toggle = None;
    for (entity, mut window, cursor_icon) in &mut windows {
        if !windowed.contains(&entity) {
            continue;
        }
        let maximized = is_maximized(entity);
        let Some(cursor) = window.cursor_position() else { continue };
        let over_button = title_buttons.iter().any(|(b, h)| b.window == entity && h.get());
        let edge = if maximized || over_button { None } else { edge_at(cursor, window.size()) };

        let wanted = CursorIcon::System(edge.map_or(SystemCursorIcon::Default, resize_cursor));
        if cursor_icon != Some(&wanted) {
            commands.entity(entity).insert(wanted);
        }

        // Alt + clic est le geste « prendre l'écran » de `link` : on n'y touche pas
        if !buttons.just_pressed(MouseButton::Left) || keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]) {
            continue;
        }
        debug!("barre de titre : clic a {cursor} (bord {edge:?}, bouton {over_button})");
        if let Some(octant) = edge {
            window.start_drag_resize(octant);
        } else if cursor.y < TITLE_H && !over_button {
            let now = time.elapsed_secs_f64();
            if matches!(*last_click, Some((e, t)) if e == entity && now - t < DOUBLE_CLICK) {
                *last_click = None;
                toggle = Some(entity);
            } else {
                *last_click = Some((entity, now));
                window.start_drag_move();
            }
        }
    }
    if let Some(entity) = toggle {
        let maximized = is_maximized(entity);
        if let Ok((_, mut window, _)) = windows.get_mut(entity) {
            window.set_maximized(!maximized);
        }
    }
}

/// Fenêtres sans bordure système, mais avec l'ombre de Windows.
fn frame_style(bars: Query<&TitleBar>, mut windows: Query<&mut Window>, mut styled: Local<Vec<Entity>>, _main_thread: NonSendMarker) {
    for bar in &bars {
        let Ok(mut window) = windows.get_mut(bar.0) else { continue };
        if window.decorations {
            window.decorations = false;
        }
        if styled.contains(&bar.0) {
            continue;
        }
        let done = WINIT_WINDOWS.with_borrow(|w| {
            let Some(_window) = w.get_window(bar.0) else { return false };
            #[cfg(windows)]
            {
                use winit::platform::windows::WindowExtWindows;
                _window.set_undecorated_shadow(true);
            }
            true
        });
        if done {
            styled.push(bar.0);
        }
    }
    styled.retain(|e| windows.get(*e).is_ok());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_and_corners() {
        let size = Vec2::new(500.0, 800.0);
        assert_eq!(edge_at(Vec2::new(2.0, 2.0), size), Some(CompassOctant::NorthWest));
        assert_eq!(edge_at(Vec2::new(498.0, 400.0), size), Some(CompassOctant::East));
        assert_eq!(edge_at(Vec2::new(250.0, 797.0), size), Some(CompassOctant::South));
        assert_eq!(edge_at(Vec2::new(250.0, 400.0), size), None);
    }
}
