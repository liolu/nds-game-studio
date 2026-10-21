//! Orbite 3D : l'écran du haut est en 3D (`main_mode = "3d"`) avec un texte 2D par-dessus ;
//! l'écran du bas sert de pavé tactile pour tourner la caméra.

use bevy::prelude::*;
use nds_engine::prelude::*;

const ID: &str = "orbit";

pub struct Orbit;

impl Plugin for Orbit {
    fn build(&self, app: &mut App) {
        app.add_ds_game(include_str!("game.toml"), setup)
            .add_systems(FixedUpdate, (steer, spin, place_camera).chain().run_if(playing(ID)));
    }
}

#[derive(Resource)]
struct View {
    yaw: f32,
    pitch: f32,
    distance: f32,
    last_touch: Option<Vec2>,
}

#[derive(Component)]
struct Spin(f32);

#[derive(Component)]
struct Readout;

fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>) {
    commands.insert_resource(View { yaw: 0.6, pitch: 0.35, distance: 9.0, last_touch: None });
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
    commands.spawn((Sprite::from_color(Color::srgb(0.29, 0.31, 0.41), Vec2::new(236.0, 132.0)), Transform::from_translation(ds_to_world(128.0, 82.0).extend(0.0)), sub.clone(), in_game()));
    commands.spawn((Text2d::new("Glissez ici pour tourner"), TextFont::from_font_size(11.0), Transform::from_translation(ds_to_world(128.0, 82.0).extend(1.0)), sub.clone(), in_game()));
    commands.spawn((Text2d::new(""), TextFont::from_font_size(9.0), Transform::from_translation(ds_to_world(128.0, 168.0).extend(1.0)), Readout, sub.clone(), in_game()));
    commands.spawn((Text2d::new("Croix : tourner   L/R : zoom   Echap : menu"), TextFont::from_font_size(9.0), Transform::from_translation(ds_to_world(128.0, 182.0).extend(1.0)), sub, in_game()));
}

fn steer(time: Res<Time>, input: Res<DsInput>, stylus: Res<Stylus>, mut view: ResMut<View>) {
    let dt = time.delta_secs();
    let pad = input.dpad();
    view.yaw += pad.x * 1.5 * dt;
    view.pitch += pad.y * 1.0 * dt;
    if let Some(touch) = stylus.touch() {
        if let Some(last) = view.last_touch {
            let delta = touch - last;
            view.yaw -= delta.x * 0.02;
            view.pitch += delta.y * 0.02;
        }
        view.last_touch = Some(touch);
    } else {
        view.last_touch = None;
    }
    let zoom = input.pressed(DsButton::R) as i32 as f32 - input.pressed(DsButton::L) as i32 as f32;
    view.distance = (view.distance - zoom * 6.0 * dt).clamp(4.0, 20.0);
    view.pitch = view.pitch.clamp(-1.3, 1.3);
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
