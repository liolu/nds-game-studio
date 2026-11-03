//! Écrans fusionnés (une seule fenêtre) ou séparés (deux fenêtres).
//!
//! Fusionnés : la fenêtre principale montre l'écran du haut et l'écran du bas l'un sous l'autre.
//! C'est une fenêtre normale : on la déplace, la redimensionne par n'importe quel bord, l'aimante
//! à une moitié d'écran (Windows) ou la réduit, et les deux écrans suivent.
//!
//! Glisser-déposer (Alt + clic gauche, ou clic molette, puis glisser) :
//! - sur l'écran du bas d'une fenêtre fusionnée : il se détache dans sa propre fenêtre, qui suit
//!   la souris ;
//! - sur une fenêtre séparée : elle se déplace (sa barre de titre marche aussi) ;
//! - lâcher la fenêtre de l'écran du bas juste sous l'écran du haut (ou l'écran du haut juste
//!   au-dessus) : les deux se fusionnent.
//!
//! F9 fusionne / sépare, F8 change le format (remplir, 4:3, 16:9, 16:10).
//! Fermer une des fenêtres quitte l'application.

use bevy::ecs::system::NonSendMarker;
use bevy::input::mouse::MouseButtonInput;
use bevy::prelude::*;
use bevy::sprite::Anchor;
use bevy::window::{WindowCloseRequested, WindowMoved, WindowResolution};
use bevy::winit::WINIT_WINDOWS;

use crate::titlebar::{is_maximized, release_for_system_drag, TITLE_H};
use crate::screens::{display_camera, DisplayCamera, DsScreens, Screen, ScreenLayout, ScreenPlaces, SCREEN_H};
use crate::studio::StudioState;

/// Distance (pixels physiques) sous laquelle une fenêtre lâchée se fusionne.
const SNAP_DISTANCE: f32 = 64.0;
/// Délai sans déplacement après lequel on considère qu'une fenêtre a été lâchée.
const DROP_DELAY: f32 = 0.15;
/// Le moteur suit lui-même la souris pendant le geste Alt + clic. Ailleurs, le système déplace la fenêtre.
const MANUAL_DRAG: bool = cfg!(any(windows, target_os = "macos"));
/// Couche du message affiché dans la fenêtre principale (format choisi...).
pub(crate) const TOAST_LAYER: usize = 20;

pub struct LinkPlugin;

impl Plugin for LinkPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Drag>()
            .init_resource::<DropWatch>()
            .add_systems(Startup, spawn_toast)
            .add_systems(Update, (keys, gestures, reconcile, manual_drag, snap_on_drop).chain())
            .add_message::<ShowToast>()
            .add_systems(Update, (toast_messages, show_toast, close_all).chain());
    }
}

/// Fenêtre déplacée à la souris par le moteur (geste Alt + clic / clic molette).
#[derive(Resource, Default)]
struct Drag {
    window: Option<Entity>,
    /// Position du curseur dans la zone client au moment de la prise (pixels physiques).
    grab: IVec2,
    button: Option<MouseButton>,
}

#[derive(Resource, Default)]
struct DropWatch {
    moved: bool,
    since: f32,
    /// Temps pendant lequel on ignore les déplacements (fenêtre que le moteur vient de placer).
    ignore: f32,
}

/// Afficher un message quelques secondes en haut à gauche de la fenêtre principale.
#[derive(Message, Clone)]
pub struct ShowToast(pub String);

/// Message en haut à gauche de la fenêtre principale.
#[derive(Component)]
struct Toast {
    left: f32,
}

// ─────────────────────────────────────────────────────────────────────────
//  Accès direct aux fenêtres (winit) et à la souris du système
// ─────────────────────────────────────────────────────────────────────────

/// (coin de la zone client, coin extérieur, taille de la zone client), en pixels physiques.
fn window_rect(entity: Entity) -> Option<(IVec2, IVec2, UVec2)> {
    WINIT_WINDOWS.with_borrow(|windows| {
        let window = windows.get_window(entity)?;
        let inner = window.inner_position().ok()?;
        let outer = window.outer_position().ok()?;
        let size = window.inner_size();
        Some((IVec2::new(inner.x, inner.y), IVec2::new(outer.x, outer.y), UVec2::new(size.width, size.height)))
    })
}

/// Place la zone client de la fenêtre à `inner` (pixels physiques). Directement par winit : Bevy ne
/// renvoie pas une position égale à la dernière qu'il a demandée, même si la fenêtre a bougé.
fn move_inner_to(entity: Entity, inner: IVec2) {
    let Some((current_inner, outer, _)) = window_rect(entity) else { return };
    let target = inner - (current_inner - outer);
    WINIT_WINDOWS.with_borrow(|windows| {
        if let Some(window) = windows.get_window(entity) {
            window.set_outer_position(winit::dpi::PhysicalPosition::new(target.x, target.y));
        }
    });
}

/// Hauteur de la zone client en pixels logiques (pour convertir la barre de titre en physique).
fn main_window_logical_h(entity: Entity) -> f32 {
    WINIT_WINDOWS.with_borrow(|w| w.get_window(entity).map_or(0.0, |w| w.inner_size().height as f32 / w.scale_factor() as f32))
}

#[cfg(windows)]
fn global_cursor(_window: Entity) -> Option<IVec2> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut p = POINT { x: 0, y: 0 };
    (unsafe { GetCursorPos(&mut p) } != 0).then(|| IVec2::new(p.x, p.y))
}

/// Bouton enfoncé, même hors des fenêtres (pendant un glisser, Windows garde le clic pour lui).
#[cfg(windows)]
fn button_held(button: MouseButton) -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_MBUTTON};
    let vk = if button == MouseButton::Middle { VK_MBUTTON } else { VK_LBUTTON };
    (unsafe { GetAsyncKeyState(vk as i32) } as u16 & 0x8000) != 0
}

/// Position du curseur sur tout le bureau, dans le repère des positions de winit pour cette fenêtre.
#[cfg(target_os = "macos")]
fn global_cursor(window: Entity) -> Option<IVec2> {
    use std::ffi::c_void;
    #[repr(C)]
    struct CGPoint {
        x: f64,
        y: f64,
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventCreate(source: *const c_void) -> *const c_void;
        fn CGEventGetLocation(event: *const c_void) -> CGPoint;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(object: *const c_void);
    }
    let scale = WINIT_WINDOWS.with_borrow(|w| w.get_window(window).map(|w| w.scale_factor()))?;
    // SAFETY: l'événement vide créé ici ne sert qu'à lire la position, et il est rendu aussitôt.
    let point = unsafe {
        let event = CGEventCreate(std::ptr::null());
        if event.is_null() {
            return None;
        }
        let point = CGEventGetLocation(event);
        CFRelease(event);
        point
    };
    Some(IVec2::new((point.x * scale) as i32, (point.y * scale) as i32))
}

/// Bouton enfoncé, même hors des fenêtres : le système peut garder le clic pendant un déplacement.
#[cfg(target_os = "macos")]
fn button_held(button: MouseButton) -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventSourceButtonState(state: i32, button: u32) -> bool;
    }
    const COMBINED_SESSION_STATE: i32 = 0;
    let index = if button == MouseButton::Middle { 2 } else { 0 };
    // SAFETY: simple lecture de l'état de la souris.
    unsafe { CGEventSourceButtonState(COMBINED_SESSION_STATE, index) }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn button_held(_button: MouseButton) -> bool {
    false
}

// ─────────────────────────────────────────────────────────────────────────
//  Touches, gestes, fusion / séparation
// ─────────────────────────────────────────────────────────────────────────

fn keys(keys: Res<ButtonInput<KeyCode>>, mut layout: ResMut<ScreenLayout>, mut toasts: Query<(&mut Text2d, &mut Toast)>) {
    let mut say = |text: String| {
        if let Ok((mut t, mut toast)) = toasts.single_mut() {
            t.0 = text;
            toast.left = 2.0;
        }
    };
    if keys.just_pressed(KeyCode::F9) {
        layout.merged = !layout.merged;
        info!("F9 : ecrans {}", if layout.merged { "fusionnes" } else { "separes" });
    }
    if keys.just_pressed(KeyCode::F8) {
        layout.aspect = layout.aspect.next();
        info!("F8 : format {}", layout.aspect.label());
        say(crate::i18n::t!("Format: {}  (F8)", "Format : {}  (F8)", layout.aspect.label()));
    }
}

/// Geste « prendre » : clic molette, ou Alt + clic gauche.
pub fn grab_button(buttons: &ButtonInput<MouseButton>, keys: &ButtonInput<KeyCode>) -> Option<MouseButton> {
    if buttons.just_pressed(MouseButton::Middle) {
        return Some(MouseButton::Middle);
    }
    (buttons.just_pressed(MouseButton::Left) && keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight])).then_some(MouseButton::Left)
}

fn gestures(
    state: Res<State<StudioState>>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    screens: Res<DsScreens>,
    places: Res<ScreenPlaces>,
    mut layout: ResMut<ScreenLayout>,
    mut drag: ResMut<Drag>,
    mut windows: Query<&mut Window>,
    mut released: MessageWriter<MouseButtonInput>,
) {
    // Le launcher n'a pas d'écrans à détacher
    if *state.get() != StudioState::Playing {
        return;
    }
    let Some(button) = grab_button(&buttons, &keys) else { return };
    for screen in [Screen::Bottom, Screen::Top] {
        let Some(place) = places.get(screen) else { continue };
        let Ok(mut window) = windows.get_mut(place.window) else { continue };
        let Some(cursor) = window.cursor_position() else { continue };
        let detach = screen == Screen::Bottom && screens.bottom_window.is_none() && place.rect.contains(cursor);
        if detach {
            // L'écran du bas sort de la fenêtre fusionnée : sa nouvelle fenêtre suit la souris,
            // prise au même endroit de l'écran
            info!("ecran du bas detache");
            layout.merged = false;
            // Même largeur de fenêtre : x garde sa place, y se compte depuis le haut de l'écran du bas
            let grab = Vec2::new(cursor.x, cursor.y - place.rect.min.y + TITLE_H);
            *drag = Drag { window: None, grab: (grab * window.scale_factor()).as_ivec2(), button: Some(button) };
            return;
        }
        if screen == Screen::Bottom && screens.bottom_window.is_none() {
            continue; // fusionné, clic sur l'écran du haut : c'est la fenêtre principale qui bouge
        }
        if MANUAL_DRAG {
            *drag = Drag { window: Some(place.window), grab: (cursor * window.scale_factor()).as_ivec2(), button: Some(button) };
        } else {
            window.start_drag_move();
            release_for_system_drag(&mut released, button, place.window);
        }
        return;
    }
}

/// Met les fenêtres dans l'état demandé par `ScreenLayout::merged`.
fn reconcile(
    mut commands: Commands,
    state: Res<State<StudioState>>,
    layout: Res<ScreenLayout>,
    mut screens: ResMut<DsScreens>,
    mut windows: Query<&mut Window>,
    cameras: Query<(Entity, &DisplayCamera)>,
    mut drag: ResMut<Drag>,
    mut watch: ResMut<DropWatch>,
    _main_thread: NonSendMarker,
) {
    let main = screens.main_window;
    let Ok(mut main_window) = windows.get_mut(main) else { return };
    let rows = (2 * SCREEN_H + layout.gap) as f32 / SCREEN_H as f32;
    // Le launcher tient dans une seule fenêtre ; le choix du joueur s'applique quand un jeu tourne
    let merged = layout.merged || *state.get() != StudioState::Playing;
    match (merged, screens.bottom_window) {
        (true, Some(bottom)) => {
            // Fusion : la fenêtre principale s'agrandit vers le bas pour montrer les deux écrans
            commands.entity(bottom).despawn();
            for (camera, display) in &cameras {
                if display.0 == bottom {
                    commands.entity(camera).despawn();
                }
            }
            screens.bottom_window = None;
            if !is_maximized(main) {
                let (w, h) = (main_window.width(), main_window.height());
                main_window.resolution.set(w, (h - TITLE_H) * rows + TITLE_H);
            }
            info!("ecrans fusionnes");
        }
        (false, None) => {
            // Séparation : la fenêtre principale garde l'écran du haut, l'écran du bas a sa fenêtre
            let (w, h) = (main_window.width(), main_window.height());
            // Hauteur d'une fenêtre à un écran, barre de titre comprise
            let single_h = ((h - TITLE_H) / rows).max(96.0) + TITLE_H;
            if !is_maximized(main) {
                main_window.resolution.set(w, single_h);
            }
            let scale = main_window.scale_factor();
            let position = match window_rect(main) {
                // Assez loin pour ne pas être aimantée tout de suite
                Some((inner, _, _)) => WindowPosition::At(inner + IVec2::new((48.0 * scale) as i32, ((single_h + 120.0) * scale) as i32)),
                None => WindowPosition::Automatic,
            };
            let title = crate::i18n::t!("{} - touch screen", "{} - ecran tactile", main_window.title);
            let mut resolution = WindowResolution::new(w.max(128.0) as u32, single_h as u32);
            resolution.set_scale_factor_override(main_window.resolution.scale_factor_override());
            let bottom = commands.spawn(Window { title, resolution, position, decorations: false, ..default() }).id();
            commands.spawn(display_camera(bottom, 1));
            screens.bottom_window = Some(bottom);
            *watch = DropWatch { ignore: 0.5, ..default() };
            // Détaché à la souris : c'est cette nouvelle fenêtre qui suit le curseur
            if drag.button.is_some() && drag.window.is_none() {
                drag.window = Some(bottom);
            }
            info!("ecrans separes");
        }
        _ => {}
    }
}

/// Fenêtre tenue par le geste Alt + clic / clic molette : elle suit la souris jusqu'au relâchement.
fn manual_drag(mut drag: ResMut<Drag>, mut watch: ResMut<DropWatch>, mut released: MessageWriter<MouseButtonInput>, _main_thread: NonSendMarker) {
    let (Some(window), Some(button)) = (drag.window, drag.button) else { return };
    if !button_held(button) {
        // La fenêtre détachée a pu prendre le relâchement à celle qui avait reçu le clic
        release_for_system_drag(&mut released, button, window);
        *drag = Drag::default();
        // Lâchée : on regarde tout de suite si elle doit se fusionner
        watch.moved = true;
        watch.since = DROP_DELAY;
        return;
    }
    #[cfg(any(windows, target_os = "macos"))]
    if let Some(cursor) = global_cursor(window) {
        move_inner_to(window, cursor - drag.grab);
    }
}

/// Écrans séparés : une fenêtre lâchée près de la place « fusionnée » fusionne les écrans.
fn snap_on_drop(
    time: Res<Time>,
    mut moves: MessageReader<WindowMoved>,
    drag: Res<Drag>,
    screens: Res<DsScreens>,
    mut layout: ResMut<ScreenLayout>,
    mut watch: ResMut<DropWatch>,
    _main_thread: NonSendMarker,
) {
    let Some(bottom) = screens.bottom_window else {
        moves.clear();
        watch.moved = false;
        return;
    };
    let main = screens.main_window;
    if watch.ignore > 0.0 {
        watch.ignore -= time.delta_secs();
        moves.clear();
        return;
    }
    if moves.read().any(|m| m.window == main || m.window == bottom) {
        watch.moved = true;
        watch.since = 0.0;
    } else {
        watch.since += time.delta_secs();
    }
    if !watch.moved || watch.since < DROP_DELAY || drag.window.is_some() || button_held(MouseButton::Left) {
        return;
    }
    watch.moved = false;
    let (Some((main_inner, _, main_size)), Some((bottom_inner, _, bottom_size))) = (window_rect(main), window_rect(bottom)) else { return };
    if main_size.min_element() == 0 || bottom_size.min_element() == 0 {
        return; // fenêtre réduite : sa position (-32000) ne veut rien dire
    }
    // La fenêtre principale ne montre que l'écran du haut : sa hauteur = 192 pixels DS
    let title = (TITLE_H * main_size.y as f32 / main_window_logical_h(main).max(1.0)) as i32;
    let gap = (layout.gap as f32 * (main_size.y as i32 - title) as f32 / SCREEN_H as f32) as i32;
    let docked = main_inner + IVec2::new(0, main_size.y as i32 + gap);
    let distance = (bottom_inner - docked).as_vec2().length();
    if distance <= SNAP_DISTANCE {
        info!("fusion : ecran du bas lache a {:?} de sa place", bottom_inner - docked);
        layout.merged = true;
    }
}

fn spawn_toast(mut commands: Commands) {
    commands.spawn((
        Text2d::default(),
        TextFont::from_font_size(18.0),
        TextColor(Color::WHITE),
        Anchor::TOP_LEFT,
        Transform::from_xyz(0.0, 0.0, 10.0),
        Toast { left: 0.0 },
        bevy::camera::visibility::RenderLayers::layer(TOAST_LAYER),
        Visibility::Hidden,
    ));
}

fn show_toast(time: Res<Time>, screens: Res<DsScreens>, windows: Query<&Window>, mut toasts: Query<(&mut Toast, &mut Transform, &mut Visibility)>) {
    let Ok(window) = windows.get(screens.main_window) else { return };
    for (mut toast, mut transform, mut visibility) in &mut toasts {
        toast.left -= time.delta_secs();
        *visibility = if toast.left > 0.0 { Visibility::Visible } else { Visibility::Hidden };
        transform.translation = Vec3::new(-window.width() / 2.0 + 12.0, window.height() / 2.0 - TITLE_H - 10.0, 10.0);
    }
}

fn toast_messages(mut messages: MessageReader<ShowToast>, mut toasts: Query<(&mut Text2d, &mut Toast)>) {
    if let (Some(message), Ok((mut text, mut toast))) = (messages.read().last(), toasts.single_mut()) {
        text.0 = message.0.clone();
        toast.left = 3.0;
    }
}

/// Fermer un écran quitte le studio ; fermer la fenêtre du code (F5), des options ou des assets (F6) ne ferme qu'elle.
fn close_all(
    mut commands: Commands,
    mut requests: MessageReader<WindowCloseRequested>,
    code_windows: Query<(), Or<(With<crate::devtools::CodeWindow>, With<crate::options::OptionsWindow>, With<crate::asset_browser::AssetsWindow>)>>,
    mut exit: MessageWriter<AppExit>,
) {
    for request in requests.read() {
        if code_windows.get(request.window).is_ok() {
            commands.entity(request.window).despawn();
        } else {
            exit.write(AppExit::Success);
        }
    }
}
