//! Plateforme : le niveau (3 écrans de large) défile sur l'écran du haut, l'écran du bas montre la
//! carte du niveau et le score. Boutons seulement, pas de stylet.
//!
//! Progression sauvegardée dans l'emplacement "progression" : pièces déjà ramassées, meilleur
//! temps pour toutes les ramasser, nombre de parties finies.

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
const SLOT: &str = "progression";

/// Ce qui est écrit sur le disque : (pièces ramassées par indice dans COINS, meilleur temps en
/// secondes, parties finies, temps de la partie en cours). Types std seulement (serde vient du moteur).
type Saved = (Vec<usize>, Option<f32>, u32, f32);

/// Progression de la partie en cours, relue au lancement du jeu.
#[derive(Resource, Default)]
struct Progress {
    coins: Vec<usize>,
    best: Option<f32>,
    runs: u32,
    /// Temps de la partie en cours (compté seulement tant qu'il reste des pièces).
    time: f32,
}

impl Progress {
    fn done(&self) -> bool {
        self.coins.len() >= COINS.len()
    }

    fn save(&self, saves: &GameSaves) {
        let saved: Saved = (self.coins.clone(), self.best, self.runs, self.time);
        if let Err(e) = saves.save(SLOT, &saved) {
            warn!("sauvegarde de la progression impossible : {e}");
        }
    }

    fn score_text(&self) -> String {
        if self.done() {
            format!("Toutes les pieces en {:.1} s !\nSTART : rejouer", self.time)
        } else {
            format!("Pieces : {} / {}", self.coins.len(), COINS.len())
        }
    }

    fn record_text(&self) -> String {
        let best = self.best.map_or("-".to_string(), |b| format!("{b:.1} s"));
        format!("Record : {best}   Parties finies : {}", self.runs)
    }
}

pub struct Platformer;

impl Plugin for Platformer {
    fn build(&self, app: &mut App) {
        app.add_ds_game(include_str!("game.toml"), setup)
            .add_systems(FixedUpdate, (physics, collect, restart, follow_camera, update_map).chain().run_if(playing(ID)));
    }
}

#[derive(Component)]
struct Player {
    /// Coin haut gauche, en pixels du niveau.
    pos: Vec2,
    vel: Vec2,
    grounded: bool,
}

/// Pièce : position dans le niveau et indice dans COINS.
#[derive(Component)]
struct Coin(Vec2, usize);

#[derive(Component)]
struct MapDot;

#[derive(Component)]
struct Score;

/// Texte du record et des parties finies (écran du bas).
#[derive(Component)]
struct RecordText;

/// Centre d'un rectangle du niveau dans le repère de la caméra 2D (caméra au début du niveau).
fn level_to_world(x: f32, y: f32, size: Vec2) -> Vec3 {
    ds_to_world(x + size.x / 2.0, y + size.y / 2.0).extend(1.0)
}

fn map_point(x: f32, y: f32) -> Vec2 {
    ds_to_world(x * MAP_SCALE, MAP_TOP + y * MAP_SCALE)
}

fn spawn_coin(commands: &mut Commands, index: usize) {
    let (x, y) = COINS[index];
    commands.spawn((
        Sprite::from_color(Color::srgb(1.0, 0.85, 0.2), Vec2::splat(6.0)),
        Transform::from_translation(level_to_world(x - 3.0, y - 3.0, Vec2::splat(6.0))),
        Coin(Vec2::new(x, y), index),
        Engine::Main.layers(),
        in_game(),
    ));
}

fn setup(mut commands: Commands, saves: Res<GameSaves>) {
    let (main, sub) = (Engine::Main.layers(), Engine::Sub.layers());
    // Progression sauvegardée (indices invalides ignorés si le niveau a changé)
    let (mut coins, best, runs, time): Saved = saves.load(SLOT).unwrap_or_default();
    coins.retain(|&i| i < COINS.len());
    coins.sort_unstable();
    coins.dedup();
    let progress = Progress { coins, best, runs, time };
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
    // Les pièces déjà ramassées ne reviennent pas
    for index in (0..COINS.len()).filter(|i| !progress.coins.contains(i)) {
        spawn_coin(&mut commands, index);
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
        Text2d::new(progress.score_text()),
        TextFont::from_font_size(12.0),
        Transform::from_translation(ds_to_world(128.0, 20.0).extend(1.0)),
        Score,
        sub.clone(),
        in_game(),
    ));
    commands.spawn((
        Text2d::new(progress.record_text()),
        TextFont::from_font_size(9.0),
        Transform::from_translation(ds_to_world(128.0, 40.0).extend(1.0)),
        RecordText,
        sub.clone(),
        in_game(),
    ));
    commands.spawn((
        Text2d::new("Croix : courir   A/B : sauter   Echap : menu"),
        TextFont::from_font_size(9.0),
        Transform::from_translation(ds_to_world(128.0, 56.0).extend(1.0)),
        sub,
        in_game(),
    ));
    commands.insert_resource(progress);
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

fn collect(
    mut commands: Commands,
    time: Res<Time>,
    saves: Res<GameSaves>,
    mut progress: ResMut<Progress>,
    players: Query<&Player>,
    coins: Query<(Entity, &Coin)>,
    mut score: Query<&mut Text2d, (With<Score>, Without<RecordText>)>,
    mut record: Query<&mut Text2d, (With<RecordText>, Without<Score>)>,
) {
    if progress.done() {
        return;
    }
    progress.time += time.delta_secs();
    let Ok(player) = players.single() else { return };
    let mut changed = false;
    for (entity, coin) in &coins {
        if overlaps(player.pos, PLAYER, coin.0 - Vec2::splat(3.0), Vec2::splat(6.0)) {
            commands.entity(entity).despawn();
            if !progress.coins.contains(&coin.1) {
                progress.coins.push(coin.1);
            }
            changed = true;
        }
    }
    if !changed {
        return;
    }
    // Partie finie : le record est gardé s'il est meilleur
    if progress.done() {
        progress.runs += 1;
        let t = progress.time;
        progress.best = Some(progress.best.map_or(t, |b| b.min(t)));
    }
    // Sauvegarde tout de suite, à chaque pièce
    progress.save(&saves);
    if let Ok(mut text) = score.single_mut() {
        text.0 = progress.score_text();
    }
    if let Ok(mut text) = record.single_mut() {
        text.0 = progress.record_text();
    }
}

/// Partie finie : START remet toutes les pièces (le record et les parties finies restent).
fn restart(
    mut commands: Commands,
    input: Res<DsInput>,
    saves: Res<GameSaves>,
    mut progress: ResMut<Progress>,
    mut score: Query<&mut Text2d, With<Score>>,
) {
    if !progress.done() || !input.just_pressed(DsButton::Start) {
        return;
    }
    progress.coins.clear();
    progress.time = 0.0;
    progress.save(&saves);
    for index in 0..COINS.len() {
        spawn_coin(&mut commands, index);
    }
    if let Ok(mut text) = score.single_mut() {
        text.0 = progress.score_text();
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
