//! nds-engine : moteur de jeux DS natifs sur PC, construit sur Bevy.
//!
//! - deux écrans 256x192 (`screens`) : chaque moteur graphique DS (Main / Sub) dessine dans sa
//!   propre image basse résolution, affichée agrandie dans la fenêtre de l'écran du haut ou du bas ;
//! - les deux fenêtres sont liées (l'écran du bas, sans barre de titre, suit l'écran du haut) ou
//!   séparées, par glisser-déposer (`link`) ;
//! - entrées DS (`input`) : boutons A B X Y L R Start Select + croix, stylet sur l'écran du bas ;
//! - chaque jeu a sa spécification (`spec`, fichier game.toml) : écran principal, 2D ou 3D par
//!   écran, échelle, écart entre les écrans, cadence, touches ;
//! - le launcher (`launcher`) montre les roms de l'utilisateur (`library`) ; le studio (`studio`)
//!   applique la spécification des jeux enregistrés et les lance ;
//! - F5 montre le code du jeu, et enregistrer un fichier recompile et relance le studio (`devtools`) ;
//! - F6 montre les fichiers (modèles 3D, images) des jeux décompilés (`asset_browser`).

pub mod activity;
pub mod asset_browser;
pub mod asset_scan;
pub mod controllers;
pub mod covers;
pub mod devtools;
pub mod i18n;
pub mod input;
pub mod launcher;
pub mod library;
pub mod link;
pub mod logfile;
pub mod native_game;
pub mod options;
pub mod save;
pub mod saves_page;
pub mod screens;
pub mod settings;
pub mod spec;
pub mod studio;
pub mod titlebar;
pub mod video;

use bevy::prelude::*;
use bevy::window::{ExitCondition, WindowResolution};

pub mod prelude {
    pub use crate::input::{DsButton, DsInput, Stylus};
    pub use crate::save::GameSaves;
    pub use crate::screens::{ds_to_world, world_to_ds, Aspect, DsScreens, Engine, EngineCamera3d, RenderStyle, Screen, ScreenLayout, SCREEN_H, SCREEN_W};
    pub use crate::spec::{GameSpec, ScreenMode};
    pub use crate::launcher::UpdateOffer;
    pub use crate::studio::{in_game, playing, ActiveGame, ActiveRom, AppGameExt, AutoStart, StudioFooter, StudioState};
    pub use crate::NdsEnginePlugin;
}

/// Taille de départ de la fenêtre : celle du launcher.
pub const DEFAULT_WINDOW: (u32, u32) = (1280, 800);

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
        // Jeu, place des fenêtres... passés en ligne de commande (aussi pour se relancer, voir `devtools`)
        let options = devtools::LaunchOptions::from_args();
        let mut primary = Window {
            title: self.title.clone(),
            resolution: WindowResolution::new(DEFAULT_WINDOW.0, DEFAULT_WINDOW.1),
            position: WindowPosition::Centered(MonitorSelection::Primary),
            // Barre de titre dessinée par le studio (bouton fusion à côté de « réduire »)
            decorations: false,
            ..default()
        };
        options.apply_to_window(&mut primary);
        app.add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    // La fenêtre principale : les deux écrans fusionnés au départ (voir `link`)
                    primary_window: Some(primary),
                    // Fermer l'une des deux fenêtres quitte tout (voir `link::close_all`)
                    exit_condition: ExitCondition::DontExit,
                    close_when_requested: false,
                    ..default()
                })
                .set(bevy::log::LogPlugin { custom_layer: logfile::file_layer, ..default() }),
        )
        .add_plugins((screens::ScreensPlugin, link::LinkPlugin, input::InputPlugin, studio::StudioPlugin, titlebar::TitleBarPlugin, devtools::DevToolsPlugin, options::OptionsPlugin))
            .add_plugins((save::SavePlugin, settings::SettingsPlugin, saves_page::SavesPagePlugin, asset_browser::AssetBrowserPlugin, launcher::LauncherPlugin, i18n::I18nPlugin, video::VideoPlugin, native_game::NativeGamesPlugin));
        options.apply_to_app(app);
    }
}
