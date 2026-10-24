//! Orbite 3D : l'écran du haut est en 3D (`main_mode = "3d"`) avec un texte 2D par-dessus ;
//! l'écran du bas sert de pavé tactile pour tourner la caméra.
//!
//! La vue est sauvegardée (emplacement `vue`) quand elle ne bouge plus, et trois « photos »
//! (`photo1`..`photo3`) gardent des vues : toucher = y aller, X + toucher = garder la vue actuelle.

use bevy::prelude::*;
use nds_engine::prelude::*;

const ID: &str = "orbit";

/// Vue par défaut (quand rien n'est sauvegardé) : angle, hauteur, distance.
const DEFAULT_VIEW: (f32, f32, f32) = (0.6, 0.35, 9.0);
/// Temps sans changement avant d'enregistrer la vue (secondes).
const SAVE_DELAY: f32 = 0.5;

/// Pavé tactile pour tourner (pixels DS : centre, taille).
const PAD_CENTER: Vec2 = Vec2::new(128.0, 72.0);
const PAD_SIZE: Vec2 = Vec2::new(236.0, 112.0);
/// Boutons photo « 1 » « 2 » « 3 » (centres en pixels DS) et leur taille.
const PHOTO_X: [f32; 3] = [164.0, 196.0, 228.0];
const PHOTO_Y: f32 = 142.0;
const PHOTO_SIZE: Vec2 = Vec2::new(26.0, 18.0);
const PHOTO_EMPTY: Color = Color::srgb(0.29, 0.31, 0.41);
const PHOTO_FULL: Color = Color::srgb(0.85, 0.65, 0.25);
const PHOTO_HELP: &str = "Photos (X+toucher: garder)";

pub struct Orbit;

impl Plugin for Orbit {
    fn build(&self, app: &mut App) {
        app.add_ds_game(include_str!("game.toml"), setup)
            .add_systems(FixedUpdate, (photos, steer, remember, spin, place_camera, show_photos).chain().run_if(playing(ID)));
    }
}

#[derive(Resource)]
struct View {
    yaw: f32,
    pitch: f32,
    distance: f32,
    last_touch: Option<Vec2>,
    /// Le stylet a été posé sur le pavé (et pas sur un bouton photo).
    dragging: bool,
    /// Vue de l'image précédente, pour savoir si elle a changé.
    previous: (f32, f32, f32),
    /// Compte à rebours avant d'enregistrer (`None` : rien à enregistrer).
    pending: Option<f32>,
}

impl View {
    fn new(angles: (f32, f32, f32)) -> Self {
        let mut view = View { yaw: 0.0, pitch: 0.0, distance: 0.0, last_touch: None, dragging: false, previous: angles, pending: None };
        view.set(angles);
        view.previous = view.angles();
        view
    }

    fn angles(&self) -> (f32, f32, f32) {
        (self.yaw, self.pitch, self.distance)
    }

    fn set(&mut self, (yaw, pitch, distance): (f32, f32, f32)) {
        self.yaw = yaw;
        self.pitch = pitch.clamp(-1.3, 1.3);
        self.distance = distance.clamp(4.0, 20.0);
    }
}

/// Photos remplies (emplacements `photo1`..`photo3`) et message affiché un moment.
#[derive(Resource, Default)]
struct Photos {
    filled: [bool; 3],
    message: Option<(String, f32)>,
}

#[derive(Component)]
struct Spin(f32);

#[derive(Component)]
struct Readout;

#[derive(Component)]
struct PhotoButton(usize);

#[derive(Component)]
struct PhotoText;

fn photo_slot(i: usize) -> String {
    format!("photo{}", i + 1)
}

fn inside(point: Vec2, center: Vec2, size: Vec2) -> bool {
    (point - center).abs().cmple(size / 2.0).all()
}

fn setup(mut commands: Commands, saves: Res<GameSaves>, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>) {
    // On repart de la dernière vue sauvegardée
    commands.insert_resource(View::new(saves.load::<(f32, f32, f32)>("vue").unwrap_or(DEFAULT_VIEW)));
    let filled = [0, 1, 2].map(|i| saves.load::<(f32, f32, f32)>(&photo_slot(i)).is_some());
    commands.insert_resource(Photos { filled, message: None });

    let main = Engine::Main.layers();
    let sun = meshes.add(Sphere::new(1.0).mesh().uv(24, 16));
    commands.spawn((
        Mesh3d(sun),
        MeshMaterial3d(materials.add(StandardMaterial { base_color: Color::srgb(1.0, 0.75, 0.2), emissive: LinearRgba::rgb(2.0, 1.2, 0.2), ..default() })),
        Transform::default(),
        Spin(0.4),
        main.clone(),
        in_game(),
    ));
    let cube = meshes.add(Cuboid::new(0.7, 0.7, 0.7));
    for (i, color) in [Color::srgb(0.3, 0.6, 1.0), Color::srgb(0.9, 0.3, 0.4), Color::srgb(0.4, 0.9, 0.5)].into_iter().enumerate() {
        let radius = 2.5 + i as f32 * 1.4;
        // Un pivot qui tourne, la planète posée dessus à `radius`
        commands
            .spawn((Transform::from_rotation(Quat::from_rotation_y(i as f32 * 2.1)), Visibility::default(), Spin(0.9 / (i as f32 + 1.0)), in_game()))
            .with_child((Mesh3d(cube.clone()), MeshMaterial3d(materials.add(color)), Transform::from_xyz(radius, 0.0, 0.0), Spin(1.5), main.clone()));
    }
    commands.spawn((PointLight { intensity: 400_000.0, range: 40.0, ..default() }, Transform::default(), main.clone(), in_game()));
    commands.spawn((DirectionalLight { illuminance: 1_500.0, ..default() }, Transform::from_xyz(3.0, 6.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y), main.clone(), in_game()));

    // 2D par-dessus la 3D, sur le même écran
    commands.spawn((Text2d::new("Orbite 3D"), TextFont::from_font_size(12.0), Transform::from_translation(ds_to_world(40.0, 10.0).extend(1.0)), main, in_game()));

    let sub = Engine::Sub.layers();
    commands.spawn((Sprite::from_color(Color::srgb(0.29, 0.31, 0.41), PAD_SIZE), Transform::from_translation(ds_to_world(PAD_CENTER.x, PAD_CENTER.y).extend(0.0)), sub.clone(), in_game()));
    commands.spawn((Text2d::new("Glissez ici pour tourner"), TextFont::from_font_size(11.0), Transform::from_translation(ds_to_world(PAD_CENTER.x, PAD_CENTER.y).extend(1.0)), sub.clone(), in_game()));
    // Boutons photo : gris = vide, dore = vue gardee
    commands.spawn((Text2d::new(PHOTO_HELP), TextFont::from_font_size(9.0), Transform::from_translation(ds_to_world(74.0, PHOTO_Y).extend(1.0)), PhotoText, sub.clone(), in_game()));
    for (i, x) in PHOTO_X.into_iter().enumerate() {
        commands.spawn((Sprite::from_color(PHOTO_EMPTY, PHOTO_SIZE), Transform::from_translation(ds_to_world(x, PHOTO_Y).extend(0.0)), PhotoButton(i), sub.clone(), in_game()));
        commands.spawn((Text2d::new(format!("{}", i + 1)), TextFont::from_font_size(11.0), Transform::from_translation(ds_to_world(x, PHOTO_Y).extend(1.0)), sub.clone(), in_game()));
    }
    commands.spawn((Text2d::new(""), TextFont::from_font_size(9.0), Transform::from_translation(ds_to_world(128.0, 164.0).extend(1.0)), Readout, sub.clone(), in_game()));
    commands.spawn((Text2d::new("Croix : tourner   L/R : zoom   Echap : menu"), TextFont::from_font_size(9.0), Transform::from_translation(ds_to_world(128.0, 182.0).extend(1.0)), sub, in_game()));
}

/// Boutons photo : toucher = aller à la vue gardée, X + toucher = garder la vue actuelle.
fn photos(input: Res<DsInput>, stylus: Res<Stylus>, saves: Res<GameSaves>, mut view: ResMut<View>, mut photos: ResMut<Photos>) {
    if !stylus.just_touched() {
        return;
    }
    let Some(touch) = stylus.touch() else { return };
    let Some(i) = PHOTO_X.iter().position(|&x| inside(touch, Vec2::new(x, PHOTO_Y), PHOTO_SIZE)) else { return };
    let slot = photo_slot(i);
    let message = if input.pressed(DsButton::X) {
        match saves.save(&slot, &view.angles()) {
            Ok(()) => {
                photos.filled[i] = true;
                format!("Photo {} gardee", i + 1)
            }
            Err(e) => {
                warn!("photo {} non sauvegardee : {e}", i + 1);
                format!("Photo {} : erreur", i + 1)
            }
        }
    } else if let Some(angles) = saves.load::<(f32, f32, f32)>(&slot) {
        view.set(angles);
        format!("Photo {}", i + 1)
    } else {
        format!("Photo {} vide : X + toucher", i + 1)
    };
    photos.message = Some((message, 1.5));
}

fn steer(time: Res<Time>, input: Res<DsInput>, stylus: Res<Stylus>, mut view: ResMut<View>) {
    let dt = time.delta_secs();
    let pad = input.dpad();
    view.yaw += pad.x * 1.5 * dt;
    view.pitch += pad.y * 1.0 * dt;
    // Le pavé ne réagit que si le stylet a été posé dessus (pas sur un bouton photo)
    if stylus.just_touched() {
        view.dragging = stylus.touch().is_some_and(|t| inside(t, PAD_CENTER, PAD_SIZE));
    }
    if let Some(touch) = stylus.touch().filter(|_| view.dragging) {
        if let Some(last) = view.last_touch {
            let delta = touch - last;
            view.yaw -= delta.x * 0.02;
            view.pitch += delta.y * 0.02;
        }
        view.last_touch = Some(touch);
    } else {
        view.last_touch = None;
        if !stylus.down() {
            view.dragging = false;
        }
    }
    let zoom = input.pressed(DsButton::R) as i32 as f32 - input.pressed(DsButton::L) as i32 as f32;
    view.distance = (view.distance - zoom * 6.0 * dt).clamp(4.0, 20.0);
    view.pitch = view.pitch.clamp(-1.3, 1.3);
}

/// Enregistre la vue quand elle n'a plus changé depuis `SAVE_DELAY` (stylet ou boutons relâchés),
/// pas à chaque image.
fn remember(time: Res<Time>, saves: Res<GameSaves>, mut view: ResMut<View>) {
    let now = view.angles();
    if now != view.previous {
        view.previous = now;
        view.pending = Some(SAVE_DELAY);
        return;
    }
    let Some(left) = view.pending else { return };
    let left = left - time.delta_secs();
    if left > 0.0 {
        view.pending = Some(left);
        return;
    }
    view.pending = None;
    if let Err(e) = saves.save("vue", &now) {
        warn!("vue non sauvegardee : {e}");
    }
}

fn spin(time: Res<Time>, mut spinning: Query<(&Spin, &mut Transform)>) {
    for (spin, mut transform) in &mut spinning {
        transform.rotate_y(spin.0 * time.delta_secs());
    }
}

fn place_camera(view: Res<View>, mut cameras: Query<(&EngineCamera3d, &mut Transform)>, mut readout: Query<&mut Text2d, With<Readout>>) {
    let rotation = Quat::from_euler(EulerRot::YXZ, view.yaw, -view.pitch, 0.0);
    for (camera, mut transform) in &mut cameras {
        if camera.0 == Engine::Main {
            *transform = Transform::from_translation(rotation * Vec3::new(0.0, 0.0, view.distance)).looking_at(Vec3::ZERO, Vec3::Y);
        }
    }
    if let Ok(mut text) = readout.single_mut() {
        text.0 = format!("angle {:.0} deg  hauteur {:.0} deg  distance {:.1}", view.yaw.to_degrees().rem_euclid(360.0), view.pitch.to_degrees(), view.distance);
    }
}

/// Couleur des boutons photo (dore = remplie) et message de la dernière photo.
fn show_photos(time: Res<Time>, mut photos: ResMut<Photos>, mut buttons: Query<(&PhotoButton, &mut Sprite)>, mut text: Query<&mut Text2d, With<PhotoText>>) {
    for (button, mut sprite) in &mut buttons {
        sprite.color = if photos.filled[button.0] { PHOTO_FULL } else { PHOTO_EMPTY };
    }
    if let Some((_, left)) = photos.message.as_mut() {
        *left -= time.delta_secs();
        if *left <= 0.0 {
            photos.message = None;
        }
    }
    if let Ok(mut text) = text.single_mut() {
        let wanted = photos.message.as_ref().map_or(PHOTO_HELP, |(m, _)| m.as_str());
        if text.0 != wanted {
            text.0 = wanted.to_string();
        }
    }
}
