//! nds-mod-tools : modder un jeu DS sans toucher à sa rom.
//!
//! L'outil est une option du launcher (onglet Moding), téléchargée à la demande. Pour un jeu, tout se passe dans son dossier de mod :
//!
//! ```text
//! fichiers/          les fichiers du jeu en PNG, OBJ, WAV, MP3, MP4, TXT : ce qu'on modifie
//! remplacements/     (facultatif) des fichiers au format du jeu, posés à leur chemin dans la rom, utilisés tels quels
//! jeu-modifie.nds    la copie de la rom avec les changements : c'est elle que le launcher lance
//! manifest.json      d'où vient chaque fichier exporté, et son empreinte à l'export
//! ```
//!
//! La rom n'est ouverte qu'en lecture. Rien n'est effacé dans `fichiers/` ni `remplacements/` : repasser sur les originaux revient à
//! lancer la rom, et les modifications sont toujours là pour plus tard.

mod compress;
mod container;
mod images;
mod media;
mod model;
mod sound;
mod text;
mod work;

/// Le décodage des fichiers est celui de la fenêtre Assets, partagé pour ne pas avoir deux lecteurs à tenir à jour. Ce fichier du
/// moteur n'a besoin de Bevy nulle part : les petits modules ci-dessous lui donnent ce qu'il attend, et l'outil reste léger.
#[allow(dead_code)]
#[path = "../../../crates/engine/src/asset_scan.rs"]
mod asset_scan;

mod i18n {
    use std::sync::atomic::{AtomicBool, Ordering};

    #[derive(PartialEq)]
    pub enum Lang {
        En,
        Fr,
    }

    static FRENCH: AtomicBool = AtomicBool::new(false);

    pub fn set_french(on: bool) {
        FRENCH.store(on, Ordering::Relaxed);
    }

    pub fn lang() -> Lang {
        if FRENCH.load(Ordering::Relaxed) { Lang::Fr } else { Lang::En }
    }

    pub fn tr(en: &'static str, fr: &'static str) -> &'static str {
        if lang() == Lang::Fr { fr } else { en }
    }

    macro_rules! t {
        ($en:literal, $fr:literal $(, $arg:expr)* $(,)?) => {
            if $crate::i18n::lang() == $crate::i18n::Lang::Fr { format!($fr $(, $arg)*) } else { format!($en $(, $arg)*) }
        };
    }
    pub(crate) use t;
}

#[allow(dead_code)]
mod library {
    use std::path::{Path, PathBuf};

    pub struct RomInfo {
        pub title: String,
        pub internal_name: String,
        pub code: String,
        pub path: PathBuf,
        pub icon: Vec<u8>,
        pub publisher: String,
    }

    pub fn scan_roms(_dir: &Path) -> Vec<RomInfo> {
        Vec::new()
    }

    /// Une mini-rom se montre par ses fichiers : pas d'aperçu ici.
    pub fn read_rom_bytes(_file: &str, _bytes: &[u8]) -> Result<RomInfo, String> {
        Err("mini rom".into())
    }
}

mod save {
    use std::path::PathBuf;

    /// Seul l'index de la fenêtre Assets s'en sert, et l'outil ne le lance pas.
    pub fn data_dir() -> PathBuf {
        PathBuf::from(".")
    }
}

use std::path::PathBuf;

use i18n::t;

const USAGE: &str = "nds-mod-tools export|import --rom <rom> --mod-dir <dossier> [--only <texte>] [--lang fr]";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    i18n::set_french(value("--lang").as_deref() == Some("fr"));
    let (Some(action), Some(rom), Some(dir)) = (args.first(), value("--rom"), value("--mod-dir")) else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    let (rom, dir) = (PathBuf::from(rom), PathBuf::from(dir));
    let result = match action.as_str() {
        "export" => work::export(&rom, &dir, value("--only").as_deref()),
        "import" => work::import(&rom, &dir),
        _ => Err(USAGE.to_string()),
    };
    if let Err(e) = result {
        eprintln!("{}", t!("Failed: {e}", "Echec : {e}"));
        std::process::exit(1);
    }
}
