// Pas de console derrière les fenêtres dans une version installée
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Nds Game Studio : le moteur et le launcher.
//! `nds-game-studio <id>` lance directement un jeu ; les autres
//! options (place des fenêtres, `--split`, `--aspect`, `--code`) sont lues par le moteur.

use bevy::prelude::*;
use nds_engine::prelude::*;

fn main() {
    // L'app de validation des fichiers (Options > Developpement, avec l'acces developpeur)
    if std::env::args().any(|arg| arg == nds_engine::asset_browser::review::REVIEW_ARG) {
        nds_engine::asset_browser::review::run();
        return;
    }

    // Raccourcis qui ouvriraient le studio sans passer par le launcher : redirigés vers lui
    std::thread::spawn(ndsgs_common::repair_shortcuts);

    // Une version plus récente : le launcher la propose
    let offer = UpdateOffer::default();
    {
        let offer = offer.0.clone();
        std::thread::spawn(move || {
            let channel = ndsgs_common::CHANNEL;
            if ndsgs_common::is_pinned() {
                return;
            }
            if let Ok(info) = ndsgs_common::fetch_channel(channel, 10) {
                if ndsgs_common::should_install(channel, &info) {
                    *offer.lock().unwrap() = Some(nds_engine::launcher::UpdateInfo { version: info.label(), notes: info.release_notes });
                }
            }
        });
    }

    App::new()
        .add_plugins(NdsEnginePlugin::default())
        .insert_resource(offer)
        .insert_resource(StudioFooter(ndsgs_common::installed_label()))
        .run();
}
