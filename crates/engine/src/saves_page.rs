//! Page Options > Sauvegardes : les sauvegardes de tous les jeux.
//! A ECRIRE : voir la tâche de l'agent « page Sauvegardes ».

use bevy::prelude::*;

pub struct SavesPagePlugin;

impl Plugin for SavesPagePlugin {
    fn build(&self, _app: &mut App) {}
}

/// Contenu de la page (appelé par `options` quand on ouvre la page).
pub fn spawn_page(c: &mut ChildSpawnerCommands) {
    c.spawn((Text::new("Sauvegardes"), TextFont::from_font_size(24.0)));
}
