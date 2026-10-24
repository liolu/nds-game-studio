// Pas de console derrière les fenêtres dans une version installée
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Nds Game Studio : le moteur et les jeux enregistrés.
//! `nds-game-studio <id>` lance directement un jeu (ex. `nds-game-studio bubbles`) ; les autres
//! options (place des fenêtres, `--split`, `--aspect`, `--code`) sont lues par le moteur.

mod games;

use bevy::prelude::*;
use nds_engine::prelude::*;

fn main() {
    // Raccourcis qui ouvriraient le studio sans passer par le launcher : redirigés vers lui
    std::thread::spawn(ndsgs_common::repair_shortcuts);

    App::new()
        .add_plugins(NdsEnginePlugin::default())
        .insert_resource(StudioFooter(ndsgs_common::installed_label()))
        .add_plugins((games::platformer::Platformer, games::bubbles::Bubbles, games::orbit::Orbit))
        .run();
}
