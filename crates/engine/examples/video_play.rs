//! Essai sans fenêtre du lecteur : `video_play <fichier>`.
//! Joue la vidéo en accéléré et compte les images différentes reçues par l'image Bevy.

use std::collections::HashSet;

use bevy::prelude::*;
use nds_engine::video::{VideoAudio, VideoPlayer};

fn main() {
    let path = std::env::args().nth(1).expect("fichier");
    let bytes: std::sync::Arc<[u8]> = std::fs::read(&path).unwrap().into();
    let mut images = Assets::<Image>::default();
    let mut sources = Assets::<VideoAudio>::default();
    let mut world = World::new();
    let mut player = VideoPlayer::new(bytes, &mut images).unwrap();
    let info = player.info().clone();
    let image = player.image();
    player.play(&mut world.commands(), &mut sources);
    world.flush();
    let audio_entities = world.query_filtered::<Entity, With<AudioPlayer<VideoAudio>>>().iter(&world).count();
    let mut seen = HashSet::new();
    let mut steps = 0;
    while player.update(1.0 / info.fps, &mut images) {
        if let Some(data) = images.get(&image).and_then(|i| i.data.as_ref()) {
            seen.insert(data.iter().fold(0xcbf29ce484222325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x100000001b3)));
        }
        steps += 1;
        std::thread::sleep(std::time::Duration::from_micros(300));
        assert!(steps < 4 * info.frames + 20_000, "la lecture ne finit pas");
    }
    println!("{path}: {} images, {} differentes recues en {steps} pas, {audio_entities} source de son, fini = {}", info.frames, seen.len(), !player.is_playing());
}
