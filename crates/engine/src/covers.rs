//! Jaquettes des jeux installés, téléchargées depuis GameTDB (art.gametdb.com) et gardées dans `covers/` du dossier
//! des données. Actif par défaut, coupé dans Options > Launcher ; rien n'est téléchargé tant que l'option est coupée.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const REGIONS: [&str; 10] = ["EN", "US", "JA", "EU", "FR", "DE", "IT", "ES", "NL", "PT"];
const THREADS: usize = 4;
const TIMEOUT_SECS: u64 = 20;

/// Où en est un téléchargement : lu par le launcher à chaque image.
#[derive(Default)]
pub struct Progress {
    pub total: usize,
    pub done: usize,
    pub found: usize,
    /// Codes dont la jaquette vient d'arriver, à charger par le launcher.
    pub arrived: Vec<String>,
}

fn dir() -> PathBuf {
    crate::save::data_dir().join("covers")
}

/// Fichier d'une jaquette : le contenu reste celui reçu (PNG ou JPEG).
pub fn path(code: &str) -> PathBuf {
    dir().join(format!("{code}.img"))
}

/// Les régions à essayer : d'abord celle de la rom (dernière lettre du code), puis toutes les autres.
fn regions_for(code: &str) -> Vec<&'static str> {
    let first: &[&'static str] = match code.chars().last() {
        Some('E') => &["US", "EN"],
        Some('J') => &["JA"],
        Some('P') | Some('X') | Some('Y') | Some('Z') | Some('V') => &["EN", "EU"],
        Some('F') => &["FR"],
        Some('D') => &["DE"],
        Some('I') => &["IT"],
        Some('S') => &["ES"],
        _ => &[],
    };
    let mut regions = first.to_vec();
    regions.extend(REGIONS.iter().filter(|r| !first.contains(r)));
    regions
}

fn is_image(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG") || bytes.starts_with(&[0xFF, 0xD8, 0xFF])
}

fn fetch(agent: &ureq::Agent, code: &str) -> Option<Vec<u8>> {
    for region in regions_for(code) {
        // coverM : bien nette sur la carte du launcher ; cover : plus petite, parfois seule disponible
        for file in ["coverM", "cover"] {
            let url = format!("https://art.gametdb.com/ds/{file}/{region}/{code}.jpg");
            let Ok(mut response) = agent.get(&url).call() else { continue };
            let Ok(bytes) = response.body_mut().read_to_vec() else { continue };
            if is_image(&bytes) {
                return Some(bytes);
            }
        }
    }
    None
}

/// Télécharge les jaquettes de ces jeux dans des fils à part.
pub fn start(codes: Vec<String>) -> Arc<Mutex<Progress>> {
    let progress = Arc::new(Mutex::new(Progress { total: codes.len(), ..Default::default() }));
    let queue = Arc::new(Mutex::new(codes));
    let _ = std::fs::create_dir_all(dir());
    for _ in 0..THREADS {
        let (queue, progress) = (queue.clone(), progress.clone());
        std::thread::spawn(move || {
            let tls = ureq::tls::TlsConfig::builder().provider(ureq::tls::TlsProvider::NativeTls).build();
            let config = ureq::Agent::config_builder().tls_config(tls).timeout_global(Some(std::time::Duration::from_secs(TIMEOUT_SECS))).build();
            let agent = ureq::Agent::new_with_config(config);
            loop {
                let Some(code) = queue.lock().unwrap().pop() else { break };
                let saved = fetch(&agent, &code).is_some_and(|bytes| crate::save::write_atomic_bytes(&path(&code), &bytes).is_ok());
                let mut p = progress.lock().unwrap();
                p.done += 1;
                if saved {
                    p.found += 1;
                    p.arrived.push(code);
                }
            }
        });
    }
    progress
}

/// La jaquette de ce jeu, si elle est là : largeur, hauteur, pixels RGBA.
pub fn load(code: &str) -> Option<(u32, u32, Vec<u8>)> {
    let bytes = std::fs::read(path(code)).ok()?;
    let image = image::load_from_memory(&bytes).ok()?.to_rgba8();
    Some((image.width(), image.height(), image.into_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_region_de_la_rom_passe_en_premier() {
        assert_eq!(regions_for("AL7E")[..2], ["US", "EN"]);
        assert_eq!(regions_for("YPPJ")[0], "JA");
        assert_eq!(regions_for("AJTP")[..2], ["EN", "EU"]);
        for code in ["AL7E", "YPPJ", "AJTP", "XXXQ"] {
            assert_eq!(regions_for(code).len(), REGIONS.len());
        }
    }

    #[test]
    fn seules_les_images_sont_gardees() {
        assert!(is_image(b"\x89PNG\r\n"));
        assert!(is_image(&[0xFF, 0xD8, 0xFF, 0xE0]));
        assert!(!is_image(b"<html>404</html>"));
    }
}
