//! Langue de l'interface : anglais par défaut, français dans Options > Launcher, gardée dans
//! options.toml (voir `settings`).
//!
//! Les textes sont écrits là où ils servent, les deux langues côte à côte :
//!
//! ```ignore
//! tr("Play", "Jouer")
//! t!("{n} games", "{n} jeux")
//! ```
//!
//! La langue est lue partout, même dans les threads, donc gardée dans une variable globale ; la
//! ressource `Language` sert à savoir quand elle change. Un texte posé une fois pour toutes prend le
//! composant `Tr`, qui le refait au changement ; les pages refaites suivent `Language`.

use std::sync::atomic::{AtomicU8, Ordering};

use bevy::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Lang {
    #[default]
    En,
    Fr,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::En, Lang::Fr];

    /// Nom dans options.toml.
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Fr => "fr",
        }
    }

    /// Code inconnu ou absent : l'anglais.
    pub fn from_code(code: &str) -> Lang {
        Lang::ALL.into_iter().find(|l| l.code() == code).unwrap_or_default()
    }

    /// Nom montré dans les options, le même quelle que soit la langue.
    pub fn name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Fr => "Francais",
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn lang() -> Lang {
    if CURRENT.load(Ordering::Relaxed) == 1 { Lang::Fr } else { Lang::En }
}

/// Le texte dans la langue choisie.
pub fn tr(en: &'static str, fr: &'static str) -> &'static str {
    if lang() == Lang::Fr { fr } else { en }
}

/// Comme `tr`, avec des valeurs : `t!("{n} games", "{n} jeux")`.
macro_rules! t {
    ($en:literal, $fr:literal $(, $arg:expr)* $(,)?) => {
        if $crate::i18n::lang() == $crate::i18n::Lang::Fr { format!($fr $(, $arg)*) } else { format!($en $(, $arg)*) }
    };
}
pub(crate) use t;

/// La langue, changée par les options et remise à zéro avec elles.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Language(pub Lang);

/// Texte fixe refait quand la langue change.
#[derive(Component, Clone, Copy)]
pub struct Tr(pub &'static str, pub &'static str);

/// Change la langue : à appeler à la place d'écrire la ressource.
pub fn set_language(language: &mut Language, value: Lang) {
    CURRENT.store(if value == Lang::Fr { 1 } else { 0 }, Ordering::Relaxed);
    if language.0 != value {
        language.0 = value;
    }
}

pub struct I18nPlugin;

impl Plugin for I18nPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Language>().add_systems(PostUpdate, refresh_fixed_texts);
    }
}

fn refresh_fixed_texts(language: Res<Language>, mut texts: Query<(&Tr, &mut Text)>) {
    if !language.is_changed() {
        return;
    }
    for (tr_text, mut text) in &mut texts {
        let wanted = tr(tr_text.0, tr_text.1);
        if text.0 != wanted {
            text.0 = wanted.to_string();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_fall_back_to_english() {
        assert_eq!(Lang::from_code("fr"), Lang::Fr);
        assert_eq!(Lang::from_code("en"), Lang::En);
        assert_eq!(Lang::from_code(""), Lang::En);
        assert_eq!(Lang::from_code("de"), Lang::En);
        assert_eq!(Lang::default(), Lang::En);
    }
}
