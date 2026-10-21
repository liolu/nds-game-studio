//! Plateforme : le niveau (3 écrans de large) défile sur l'écran du haut, l'écran du bas montre la
//! carte du niveau et le score. Boutons seulement, pas de stylet.

use bevy::prelude::*;
use nds_engine::prelude::*;
use nds_engine::screens::EngineCamera2d;

const ID: &str = "platformer";
const LEVEL_W: f32 = 768.0;
const GRAVITY: f32 = -700.0;
const JUMP: f32 = 290.0;
const RUN: f32 = 110.0;
const PLAYER: Vec2 = Vec2::new(10.0, 14.0);
/// Carte de l'écran du bas : le niveau réduit 3 fois.
const MAP_SCALE: f32 = 1.0 / 3.0;
const MAP_TOP: f32 = 80.0;

/// Plateformes en pixels du niveau (x, y du coin haut gauche, largeur, hauteur ; y vers le bas).
const PLATFORMS: [(f32, f32, f32, f32); 9] = [
    (0.0, 176.0, 300.0, 16.0),
    (340.0, 176.0, 428.0, 16.0),
    (90.0, 136.0, 60.0, 8.0),
    (190.0, 104.0, 60.0, 8.0),
    (300.0, 140.0, 40.0, 8.0),
    (420.0, 120.0, 70.0, 8.0),
    (530.0, 88.0, 60.0, 8.0),
    (630.0, 128.0, 50.0, 8.0),
    (700.0, 64.0, 60.0, 8.0),
];
const COINS: [(f32, f32); 7] = [(120.0, 124.0), (220.0, 92.0), (320.0, 128.0), (455.0, 108.0), (560.0, 76.0), (655.0, 116.0), (730.0, 52.0)];

pub struct Platformer;

impl Plugin for Platformer {
    fn build(&self, app: &mut App) {
        app.add_ds_game(include_str!("game.toml"), setup)
            .add_systems(FixedUpdate, (physics, collect, follow_camera, update_map).chain().run_if(playing(ID)));
    }
}

#[derive(Component)]
struct Player {
    /// Coin haut gauche, en pixels du niveau.
    pos: Vec2,
    vel: Vec2,
    grounded: bool,
}

#[derive(Component)]
struct Coin(Vec2);

#[derive(Component)]
struct MapDot;

#[derive(Component)]
struct Score(u32);

/// Centre d'un rectangle du niveau dans le repère de la caméra 2D (caméra au début du niveau).
fn level_to_world(x: f32, y: f32, size: Vec2) -> Vec3 {
    ds_to_world(x + size.x / 2.0, y + size.y / 2.0).extend(1.0)
}

fn map_point(x: f32, y: f32) -> Vec2 {
    ds_to_world(x * MAP_SCALE, MAP_TOP + y * MAP_SCALE)
}

fn setup(mut commands: Commands) {
    let (main, sub) = (Engine::Main.layers(), Engine::Sub.layers());
    for (x, y, w, h) in PLATFORMS {
        let size = Vec2::new(w, h);
        commands.spawn((Sprite::from_color(Color::srgb(0.35, 0.6, 0.25), size), Transform::from_translation(level_to_world(x, y, size)), main.clone(), in_game()));
        commands.spawn((
            Sprite::from_color(Color::srgb(0.35, 0.6, 0.25), size * MAP_SCALE),
            Transform::from_translation((map_point(x, y) + size * MAP_SCALE * Vec2::new(0.5, -0.5)).extend(0.0)),
            sub.clone(),
            in_game(),
        ));
    }
    for (x, y) in COINS {
        commands.spawn((
            Sprite::from_color(Color::srgb(1.0, 0.85, 0.2), Vec2::splat(6.0)),
            Transform::from_translation(level_to_world(x - 3.0, y - 3.0, Vec2::splat(6.0))),
            Coin(Vec2::new(x, y)),
            main.clone(),
            in_game(),
        ));
    }
    commands.spawn((
        Sprite::from_color(Color::srgb(0.9, 0.2, 0.3), PLAYER),
        Transform::default(),
        Player { pos: Vec2::new(20.0, 140.0), vel: Vec2::ZERO, grounded: false },
        main.clone(),
        in_game(),
    ));
    commands.spawn((Sprite::from_color(Color::srgb(0.9, 0.2, 0.3), Vec2::splat(4.0)), Transform::default(), MapDot, sub.clone(), in_game()));
    commands.spawn((
        Text2d::new(format!("Pieces : 0 / {}", COINS.len())),
        TextFont::from_font_size(14.0),
        Transform::from_translation(ds_to_world(128.0, 24.0).extend(1.0)),
        Score(0),
        sub.clone(),
        in_game(),
    ));
    commands.spawn((
        Text2d::new("Croix : courir   A/B : sauter   Echap : menu"),
        TextFont::from_font_size(9.0),
        Transform::from_translation(ds_to_world(128.0, 50.0).extend(1.0)),
        sub,
        in_game(),
    ));
}

fn overlaps(a: Vec2, a_size: Vec2, b: Vec2, b_size: Vec2) -> bool {
    a.x < b.x + b_size.x && b.x < a.x + a_size.x && a.y < b.y + b_size.y && b.y < a.y + a_size.y
}

fn physics(time: Res<Time>, input: Res<DsInput>, mut players: Query<(&mut Player, &mut Transform)>) {
    let dt = time.delta_secs();
    for (mut p, mut transform) in &mut players {
        p.vel.x = input.dpad().x * RUN;
        if p.grounded && (input.just_pressed(DsButton::A) || input.just_pressed(DsButton::B)) {
            p.vel.y = JUMP;
        }
        p.vel.y += GRAVITY * dt;

        // Horizontal puis vertical (y du niveau vers le bas, vitesse vers le haut)
        let step_x = Vec2::new(p.vel.x * dt, 0.0);
        let mut pos = (p.pos + step_x).clamp(Vec2::ZERO, Vec2::new(LEVEL_W - PLAYER.x, f32::MAX));
        for (x, y, w, h) in PLATFORMS {
            let (b, size) = (Vec2::new(x, y), Vec2::new(w, h));
            if overlaps(pos, PLAYER, b, size) {
                pos.x = if p.vel.x > 0.0 { b.x - PLAYER.x } else { b.x + size.x };
            }
        }
        pos.y -= p.vel.y * dt;
        p.grounded = false;
        for (x, y, w, h) in PLATFORMS {
            let (b, size) = (Vec2::new(x, y), Vec2::new(w, h));
            if overlaps(pos, PLAYER, b, size) {
                if p.vel.y <= 0.0 {
                    pos.y = b.y - PLAYER.y;
                    p.grounded = true;
                } else {
                    pos.y = b.y + size.y;
                }
                p.vel.y = 0.0;
            }
        }
        // Tombé dans un trou : retour au départ
        if pos.y > 220.0 {
            pos = Vec2::new(20.0, 140.0);
            p.vel = Vec2::ZERO;
        }
        p.pos = pos;
        transform.translation = level_to_world(pos.x, pos.y, PLAYER).with_z(2.0);
    }
}

fn collect(mut commands: Commands, players: Query<&Player>, coins: Query<(Entity, &Coin)>, mut score: Query<(&mut Score, &mut Text2d)>) {
    let Ok(player) = players.single() else { return };
    for (entity, coin) in &coins {
        if overlaps(player.pos, PLAYER, coin.0 - Vec2::splat(3.0), Vec2::splat(6.0)) {
            commands.entity(entity).despawn();
            if let Ok((mut score, mut text)) = score.single_mut() {
                score.0 += 1;
                text.0 = if score.0 as usize == COINS.len() {
                    "Toutes les pieces ! Bravo".into()
                } else {
                    format!("Pieces : {} / {}", score.0, COINS.len())
                };
            }
        }
    }
}

/// L'écran du haut suit le joueur dans le niveau.
fn follow_camera(players: Query<&Player>, mut cameras: Query<(&EngineCamera2d, &mut Transform)>) {
    let Ok(player) = players.single() else { return };
    let x = (player.pos.x - SCREEN_W as f32 / 2.0).clamp(0.0, LEVEL_W - SCREEN_W as f32);
    for (camera, mut transform) in &mut cameras {
        if camera.0 == Engine::Main {
            transform.translation.x = x.round();
        }
    }
}

fn update_map(players: Query<&Player>, mut dots: Query<&mut Transform, With<MapDot>>) {
    let (Ok(player), Ok(mut dot)) = (players.single(), dots.single_mut()) else { return };
    dot.translation = map_point(player.pos.x + PLAYER.x / 2.0, player.pos.y + PLAYER.y / 2.0).extend(1.0);
}
