//! Écrans liés ou séparés.
//!
//! Liés : l'écran du bas n'a pas de barre de titre ni de bouton dans la barre des tâches, il est
//! collé sous l'écran du haut (avec `gap` pixels d'écart) et le suit quand on déplace ou
//! redimensionne l'écran du haut.
//!
//! Glisser-déposer :
//! - séparer : clic molette, ou Alt + clic gauche, sur l'écran du bas puis glisser ; la fenêtre
//!   retrouve sa barre de titre et suit la souris ;
//! - lier : glisser l'écran du bas (par sa barre de titre) ou l'écran du haut pour que le haut de
//!   l'écran du bas touche presque le bas de l'écran du haut, puis lâcher : il s'aimante ;
//! - F9 lie ou sépare sans la souris.
//!
//! Clic molette ou Alt + clic gauche sur l'écran du haut déplace les deux écrans liés.
//! Fermer l'une des deux fenêtres quitte l'application.

use bevy::ecs::system::NonSendMarker;
use bevy::prelude::*;
use bevy::window::{WindowCloseRequested, WindowMoved};
use bevy::winit::WINIT_WINDOWS;

use crate::screens::{DsScreens, Screen};

/// Distance (pixels physiques) sous laquelle l'écran du bas s'aimante à l'écran du haut.
const SNAP_DISTANCE: f32 = 64.0;
/// Délai sans déplacement après lequel on considère que la fenêtre a été lâchée.
const DROP_DELAY: f32 = 0.15;

#[derive(Resource)]
pub struct ScreenLink {
    pub linked: bool,
    /// Écart entre les deux écrans liés, en pixels DS (agrandi avec l'écran).
    pub gap: u32,
    /// Une fenêtre a bougé depuis la dernière tentative d'aimantation.
    moved: bool,
    since_move: f32,
    /// L'écran du bas a été réduit avec l'écran du haut.
    bottom_minimized: bool,
}

impl Default for ScreenLink {
    fn default() -> Self {
        Self { linked: true, gap: 0, moved: false, since_move: 0.0, bottom_minimized: false }
    }
}

pub struct LinkPlugin;

impl Plugin for LinkPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ScreenLink>()
            .add_systems(Update, (drag_gestures, toggle_key, snap_on_drop, apply_link).chain())
            .add_systems(Update, close_all);
    }
}

/// Position et taille réelles d'une fenêtre (pixels physiques) :
/// (coin de la zone client, coin extérieur, taille de la zone client).
fn window_rect(entity: Entity) -> Option<(IVec2, IVec2, UVec2)> {
    WINIT_WINDOWS.with_borrow(|windows| {
        let window = windows.get_window(entity)?;
        let inner = window.inner_position().ok()?;
        let outer = window.outer_position().ok()?;
        let size = window.inner_size();
        Some((IVec2::new(inner.x, inner.y), IVec2::new(outer.x, outer.y), UVec2::new(size.width, size.height)))
    })
}

/// Écran du bas collé (sans barre de titre ni bouton dans la barre des tâches) ou libre.
/// Directement par winit : Bevy ne renvoie un réglage que s'il diffère du dernier qu'il a appliqué,
/// ce qui ne suffit pas pour une fenêtre qu'on change souvent d'état.
fn set_docked_style(entity: Entity, docked: bool) {
    WINIT_WINDOWS.with_borrow(|windows| {
        let Some(window) = windows.get_window(entity) else { return };
        // Windows garde la taille extérieure : sans ça l'image rétrécirait de la barre de titre
        let size = window.inner_size();
        window.set_decorations(!docked);
        let _ = window.request_inner_size(size);
        #[cfg(windows)]
        {
            use winit::platform::windows::WindowExtWindows;
            window.set_skip_taskbar(docked);
        }
        debug!("ecran du bas : barre de titre {}", window.is_decorated());
    });
}

fn move_window(entity: Entity, outer: IVec2) {
    WINIT_WINDOWS.with_borrow(|windows| {
        if let Some(window) = windows.get_window(entity) {
            window.set_outer_position(winit::dpi::PhysicalPosition::new(outer.x, outer.y));
        }
    });
}

/// Position de la zone client de l'écran du bas pour qu'il soit collé sous l'écran du haut.
fn docked_position(top_inner: IVec2, top_size: UVec2, gap: u32) -> IVec2 {
    let scale = top_size.y as f32 / crate::screens::SCREEN_H as f32;
    top_inner + IVec2::new(0, top_size.y as i32 + (gap as f32 * scale).round() as i32)
}

fn left_button_held_anywhere(buttons: &ButtonInput<MouseButton>) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
        // Pendant un déplacement par la barre de titre, Windows garde le clic pour lui : on lit
        // l'état réel du bouton
        let _ = buttons;
        (unsafe { GetAsyncKeyState(VK_LBUTTON as i32) } as u16 & 0x8000) != 0
    }
    #[cfg(not(windows))]
    {
        buttons.pressed(MouseButton::Left)
    }
}

/// Le geste « prendre la fenêtre » (clic molette ou Alt + clic gauche) sur une des deux fenêtres.
pub fn grab_gesture(buttons: &ButtonInput<MouseButton>, keys: &ButtonInput<KeyCode>) -> bool {
    buttons.just_pressed(MouseButton::Middle) || (buttons.just_pressed(MouseButton::Left) && keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]))
}

fn drag_gestures(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    screens: Res<DsScreens>,
    mut link: ResMut<ScreenLink>,
    mut windows: Query<&mut Window>,
    _main_thread: NonSendMarker,
) {
    if !grab_gesture(&buttons, &keys) {
        return;
    }
    for screen in Screen::ALL {
        let Ok(mut window) = windows.get_mut(screens.window(screen)) else { continue };
        if window.cursor_position().is_none() {
            continue;
        }
        if screen == Screen::Bottom && link.linked {
            // On décroche l'écran du bas : il retrouve sa barre de titre et suit la souris
            info!("ecran du bas decroche");
            link.linked = false;
            set_docked_style(screens.window(Screen::Bottom), false);
        }
        // Écran du haut : déplace les deux écrans s'ils sont liés (l'écran du bas suit)
        window.start_drag_move();
        link.moved = false;
        return;
    }
}

fn toggle_key(keys: Res<ButtonInput<KeyCode>>, screens: Res<DsScreens>, mut link: ResMut<ScreenLink>, _main_thread: NonSendMarker) {
    if !keys.just_pressed(KeyCode::F9) {
        return;
    }
    link.linked = !link.linked;
    link.moved = false;
    info!("F9 : ecrans {}", if link.linked { "lies" } else { "separes" });
    if !link.linked {
        let bottom = screens.window(Screen::Bottom);
        set_docked_style(bottom, false);
        // Décalé un peu pour qu'on voie qu'il est séparé
        if let Some((_, outer, _)) = window_rect(bottom) {
            move_window(bottom, outer + IVec2::new(48, 48));
        }
    }
}

/// Fenêtres séparées : si on lâche l'une d'elles assez près de la position « collée », on les lie.
fn snap_on_drop(
    time: Res<Time>,
    mut moves: MessageReader<WindowMoved>,
    buttons: Res<ButtonInput<MouseButton>>,
    screens: Res<DsScreens>,
    mut link: ResMut<ScreenLink>,
    _main_thread: NonSendMarker,
) {
    let (top, bottom) = (screens.window(Screen::Top), screens.window(Screen::Bottom));
    if moves.read().any(|m| m.window == top || m.window == bottom) {
        // Liés, c'est `apply_link` qui déplace l'écran du bas : rien à aimanter
        link.moved = !link.linked;
        link.since_move = 0.0;
        if !link.linked {
            debug!("fenetre deplacee (ecrans separes)");
        }
    } else {
        link.since_move += time.delta_secs();
    }
    if link.linked || !link.moved || link.since_move < DROP_DELAY || left_button_held_anywhere(&buttons) {
        return;
    }
    link.moved = false;
    let (Some((top_inner, _, top_size)), Some((bottom_inner, _, bottom_size))) = (window_rect(top), window_rect(bottom)) else { return };
    if top_size.min_element() == 0 || bottom_size.min_element() == 0 {
        return; // fenêtre réduite : sa position (-32000) ne veut rien dire
    }
    let docked = docked_position(top_inner, top_size, link.gap);
    if (bottom_inner - docked).as_vec2().length() <= SNAP_DISTANCE {
        info!("ecrans lies (lache a {:?} de la place collee)", bottom_inner - docked);
        link.linked = true;
    }
}

/// Écrans liés : l'écran du bas, sans barre de titre, suit la position et la taille de l'écran du haut.
fn apply_link(mut link: ResMut<ScreenLink>, screens: Res<DsScreens>, mut windows: Query<&mut Window>, _main_thread: NonSendMarker) {
    if !link.linked {
        return;
    }
    let (top, bottom) = (screens.window(Screen::Top), screens.window(Screen::Bottom));
    let Ok(mut bottom_window) = windows.get_mut(bottom) else { return };
    if WINIT_WINDOWS.with_borrow(|w| w.get_window(bottom).is_some_and(|w| w.is_decorated())) {
        set_docked_style(bottom, true);
        return; // la zone client change de place : on mesure à l'image suivante
    }
    let (Some((top_inner, _, top_size)), Some((bottom_inner, bottom_outer, bottom_size))) = (window_rect(top), window_rect(bottom)) else {
        return;
    };
    // Écran du haut réduit : l'écran du bas (absent de la barre des tâches) est réduit avec lui
    let top_minimized = top_size.x == 0 || top_size.y == 0 || WINIT_WINDOWS.with_borrow(|w| w.get_window(top).and_then(|w| w.is_minimized()) == Some(true));
    if top_minimized != link.bottom_minimized {
        link.bottom_minimized = top_minimized;
        WINIT_WINDOWS.with_borrow(|windows| {
            if let Some(window) = windows.get_window(bottom) {
                window.set_minimized(top_minimized);
            }
        });
        return;
    }
    if top_minimized || bottom_size.x == 0 {
        return;
    }
    let docked = docked_position(top_inner, top_size, link.gap);
    if bottom_inner != docked {
        // La zone client peut être décalée du coin extérieur (bordures invisibles de Windows).
        // Directement par winit : Bevy ne renvoie pas une position égale à la dernière qu'il a
        // demandée, même si la fenêtre a bougé depuis (elle resterait alors décalée).
        debug!("ecran du bas : {bottom_inner} -> {docked}");
        move_window(bottom, docked - (bottom_inner - bottom_outer));
    }
    if bottom_size != top_size {
        debug!("ecran du bas : taille {bottom_size} -> {top_size}");
        bottom_window.resolution.set_physical_resolution(top_size.x, top_size.y);
    }
}

fn close_all(mut requests: MessageReader<WindowCloseRequested>, mut exit: MessageWriter<AppExit>) {
    if requests.read().next().is_some() {
        exit.write(AppExit::Success);
    }
}
