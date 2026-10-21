//! nds-engine : moteur de jeux DS natifs sur PC, construit sur Bevy.
//!
//! - deux écrans 256x192 (`screens`) : chaque moteur graphique DS (Main / Sub) dessine dans sa
//!   propre image basse résolution, affichée agrandie dans la fenêtre de l'écran du haut ou du bas ;
//! - les deux fenêtres sont liées (l'écran du bas, sans barre de titre, suit l'écran du haut) ou
//!   séparées, par glisser-déposer (`link`) ;
//! - entrées DS (`input`) : boutons A B X Y L R Start Select + croix, stylet sur l'écran du bas ;
//! - chaque jeu a sa spécification (`spec`, fichier game.toml) : écran principal, 2D ou 3D par
//!   écran, échelle, écart entre les écrans, cadence, touches ;
//! - le studio (`studio`) liste les jeux enregistrés, applique leur spécification et les lance.

pub mod input;
pub mod link;
pub mod screens;
pub mod spec;
pub mod studio;

use bevy::prelude::*;
use bevy::window::{ExitCondition, WindowResolution};

pub mod prelude {
    pub use crate::input::{DsButton, DsInput, Stylus};
    pub use crate::link::ScreenLink;
    pub use crate::screens::{ds_to_world, world_to_ds, DsScreens, Engine, EngineCamera3d, Screen, SCREEN_H, SCREEN_W};
    pub use crate::spec::{GameSpec, ScreenMode};
    pub use crate::studio::{in_game, playing, ActiveGame, AppGameExt, AutoStart, StudioFooter, StudioState};
    pub use crate::NdsEnginePlugin;
}

/// Échelle de départ des fenêtres (256x192 -> 512x384 : les deux écrans tiennent sur un écran 1080p).
pub const DEFAULT_SCALE: u32 = 2;

/// Tout le moteur : fenêtres, écrans, liaison, entrées et studio.
pub struct NdsEnginePlugin {
    pub title: String,
}

impl Default for NdsEnginePlugin {
    fn default() -> Self {
        Self { title: "Nds Game Studio".into() }
    }
}

impl Plugin for NdsEnginePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(
            DefaultPlugins
                .set(ImagePlugin::default_nearest())
                .set(WindowPlugin {
                    // La fenêtre principale = l'écran du haut ; celle du bas est créée par `screens`
                    primary_window: Some(Window {
                        title: self.title.clone(),
                        resolution: WindowResolution::new(screens::SCREEN_W * DEFAULT_SCALE, screens::SCREEN_H * DEFAULT_SCALE),
                        position: WindowPosition::Centered(MonitorSelection::Primary),
                        ..default()
                    }),
                    // Fermer l'une des deux fenêtres quitte tout (voir `link::close_all`)
                    exit_condition: ExitCondition::DontExit,
                    close_when_requested: false,
                    ..default()
                }),
        )
        .add_plugins((screens::ScreensPlugin, link::LinkPlugin, input::InputPlugin, studio::StudioPlugin));
    }
}
