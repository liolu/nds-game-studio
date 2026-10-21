//! Bulles : jeu au stylet. Le moteur principal est sur l'écran du bas (`main = "bottom"`), le
//! score sur celui du haut. Select échange les écrans en jeu : les bulles passent en haut, où le
//! stylet ne peut plus les toucher.

use bevy::prelude::*;
use nds_engine::prelude::*;

const ID: &str = "bubbles";
const DURATION: f32 = 45.0;

pub struct Bubbles;

impl Plugin for Bubbles {
    fn build(&self, app: &mut App) {
        app.add_ds_game(include_str!("game.toml"), setup)
            .add_systems(FixedUpdate, (spawn_bubbles, rise, pop, tick, swap_screens).run_if(playing(ID)));
    }
}

#[derive(Resource)]
struct Round {
    score: u32,
    missed: u32,
    left: f32,
    until_next: f32,
    /// Générateur pseudo-aléatoire (xorshift) : pas besoin d'une dépendance pour des bulles.
    seed: u32,
    mesh: Handle<Mesh>,
}

impl Round {
    fn random(&mut self) -> f32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        (self.seed % 10_000) as f32 / 10_000.0
    }
}

#[derive(Component)]
struct Bubble {
    radius: f32,
    speed: f32,
}

#[derive(Component)]
struct Hud;

fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>) {
    commands.insert_resource(Round { score: 0, missed: 0, left: DURATION, until_next: 0.0, seed: 0x9e37_79b9, mesh: meshes.add(Circle::new(1.0)) });
    let sub = Engine::Sub.layers();
    commands.spawn((Text2d::new("Bulles Tactiles"), TextFont::from_font_size(18.0), Transform::from_translation(ds_to_world(128.0, 24.0).extend(1.0)), sub.clone(), in_game()));
    commands.spawn((Text2d::new(""), TextFont::from_font_size(13.0), Transform::from_translation(ds_to_world(128.0, 90.0).extend(1.0)), Hud, sub.clone(), in_game()));
    commands.spawn((
        Text2d::new("Stylet : eclater   Select : echanger les ecrans\nA : rejouer   Echap : menu"),
        TextFont::from_font_size(9.0),
        Transform::from_translation(ds_to_world(128.0, 170.0).extend(1.0)),
        sub,
        in_game(),
    ));
}

fn spawn_bubbles(time: Res<Time>, mut round: ResMut<Round>, mut materials: ResMut<Assets<ColorMaterial>>, mut commands: Commands) {
    if round.left <= 0.0 {
        return;
    }
    round.until_next -= time.delta_secs();
    if round.until_next > 0.0 {
        return;
    }
    round.until_next = 0.35 + round.random() * 0.5;
    let radius = 8.0 + round.random() * 14.0;
    let x = radius + round.random() * (SCREEN_W as f32 - 2.0 * radius);
    let color = Color::hsl(round.random() * 360.0, 0.7, 0.6);
    let speed = 30.0 + round.random() * 50.0;
    commands.spawn((
        Mesh2d(round.mesh.clone()),
        MeshMaterial2d(materials.add(color)),
        Transform::from_translation(ds_to_world(x, SCREEN_H as f32 + radius).extend(0.0)).with_scale(Vec3::splat(radius)),
        Bubble { radius, speed },
        Engine::Main.layers(),
        in_game(),
    ));
}

fn rise(time: Res<Time>, mut round: ResMut<Round>, mut bubbles: Query<(Entity, &Bubble, &mut Transform)>, mut commands: Commands) {
    for (entity, bubble, mut transform) in &mut bubbles {
        transform.translation.y += bubble.speed * time.delta_secs();
        if transform.translation.y - bubble.radius > SCREEN_H as f32 / 2.0 {
            commands.entity(entity).despawn();
            round.missed += 1;
        }
    }
}

fn pop(stylus: Res<Stylus>, screens: Res<DsScreens>, mut round: ResMut<Round>, bubbles: Query<(Entity, &Bubble, &Transform)>, mut commands: Commands) {
    // Le stylet touche l'écran du bas : les bulles n'y sont que si le moteur principal y est
    if !stylus.just_touched() || screens.screen_of(Engine::Main) != Screen::Bottom || round.left <= 0.0 {
        return;
    }
    let touch = ds_to_world(stylus.position.x, stylus.position.y);
    if let Some((entity, ..)) = bubbles.iter().find(|(_, b, t)| t.translation.truncate().distance(touch) <= b.radius) {
        commands.entity(entity).despawn();
        round.score += 1;
    }
}

fn tick(time: Res<Time>, input: Res<DsInput>, mut round: ResMut<Round>, mut hud: Query<&mut Text2d, With<Hud>>, bubbles: Query<Entity, With<Bubble>>, mut commands: Commands) {
    round.left = (round.left - time.delta_secs()).max(0.0);
    if round.left == 0.0 && input.just_pressed(DsButton::A) {
        bubbles.iter().for_each(|e| commands.entity(e).despawn());
        (round.score, round.missed, round.left) = (0, 0, DURATION);
    }
    if let Ok(mut text) = hud.single_mut() {
        text.0 = if round.left > 0.0 {
            format!("Score : {}\nRatees : {}\nTemps : {:.0} s", round.score, round.missed, round.left.ceil())
        } else {
            format!("Fini !\nScore : {}   Ratees : {}\nA pour rejouer", round.score, round.missed)
        };
    }
}

fn swap_screens(input: Res<DsInput>, mut screens: ResMut<DsScreens>) {
    if input.just_pressed(DsButton::Select) && !input.pressed(DsButton::Start) {
        screens.swap();
    }
}
