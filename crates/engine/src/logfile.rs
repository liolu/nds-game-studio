//! Le journal du launcher : `log/launcher.log` à côté de l'exécutable (`NDSGS_LOG_DIR` pour les tests),
//! pour comprendre ce qui se passe sur un PC ou un Mac qu'on n'a pas sous la main.
//!
//! Le fichier est refait à chaque lancement et le précédent reste dans `launcher.old.log`. Si le
//! dossier de l'exécutable n'est pas modifiable (application macOS déplacée par le système, Program
//! Files), le journal va dans `log` du dossier des données.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use bevy::log::tracing_subscriber::{fmt, Layer, Registry};
use bevy::log::BoxedLayer;
use bevy::prelude::*;

const LOG_FILE: &str = "launcher.log";
const OLD_LOG_FILE: &str = "launcher.old.log";

/// Le dossier `log` à côté de l'exécutable, comme le dossier `rom`.
pub fn log_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("NDSGS_LOG_DIR") {
        return PathBuf::from(dir);
    }
    let exe = std::env::current_exe().unwrap_or_default();
    exe.parent().unwrap_or(Path::new(".")).join("log")
}

/// Ouvre un journal neuf en gardant le précédent.
fn open_log(dir: &Path) -> std::io::Result<File> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(LOG_FILE);
    if path.exists() {
        std::fs::rename(&path, dir.join(OLD_LOG_FILE))?;
    }
    File::create(path)
}

fn file_writer<S>(file: File) -> impl Layer<S> + Send + Sync + 'static
where
    S: bevy::log::tracing::Subscriber + for<'a> bevy::log::tracing_subscriber::registry::LookupSpan<'a>,
{
    fmt::Layer::default().with_ansi(false).with_writer(Mutex::new(file))
}

/// Pour `LogPlugin::custom_layer` : tout ce que Bevy journalise va aussi dans le fichier.
pub fn file_layer(_: &mut App) -> Option<BoxedLayer> {
    let file = open_log(&log_dir()).or_else(|_| open_log(&crate::save::data_dir().join("log")));
    let file = match file {
        Ok(file) => file,
        Err(e) => {
            eprintln!("journal impossible a ouvrir : {e}");
            return None;
        }
    };
    log_panics();
    let layer: Box<dyn Layer<Registry> + Send + Sync> = Box::new(file_writer(file));
    Some(layer)
}

/// Un plantage est la première chose qu'on cherche dans le journal.
fn log_panics() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        error!("plantage : {info}");
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::log::tracing_subscriber::layer::SubscriberExt;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ndsgs-logfile-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn keeps_the_previous_log() {
        let dir = temp_dir("rotate").join("log");
        std::fs::write(open_log(&dir).map(|_| dir.join(LOG_FILE)).unwrap(), "premier").unwrap();
        open_log(&dir).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join(OLD_LOG_FILE)).unwrap(), "premier");
        assert_eq!(std::fs::read_to_string(dir.join(LOG_FILE)).unwrap(), "");
    }

    #[test]
    fn messages_reach_the_file_without_colors() {
        let dir = temp_dir("write");
        let subscriber = Registry::default().with(file_writer(open_log(&dir).unwrap()));
        bevy::log::tracing::subscriber::with_default(subscriber, || info!("bonjour launcher"));
        let text = std::fs::read_to_string(dir.join(LOG_FILE)).unwrap();
        assert!(text.contains("bonjour launcher"), "{text}");
        assert!(!text.contains('\u{1b}'), "{text}");
    }
}
