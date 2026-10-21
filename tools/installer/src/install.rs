//! Téléchargement et extraction partagés entre l'installeur Windows (GUI)
//! et l'installeur Linux/macOS (terminal).

use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

fn make_agent(timeout_secs: u64) -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder()
        .provider(ureq::tls::TlsProvider::NativeTls)
        .build();
    let config = ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_global(Some(std::time::Duration::from_secs(timeout_secs)))
        .build();
    ureq::Agent::new_with_config(config)
}

pub fn fetch_version_info() -> Result<ndsgs_common::VersionInfo, String> {
    let agent = make_agent(10);

    let body: String = agent
        .get(ndsgs_common::VERSION_URL)
        .call()
        .map_err(|e| format!("Erreur reseau: {}\n\nVerifiez que GitHub Pages est active sur le depot.", e))?
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("Erreur lecture: {}", e))?;

    serde_json::from_str(&body).map_err(|e| format!("Erreur JSON: {}", e))
}

/// Télécharge l'archive de la plateforme courante et l'extrait dans `install_dir`.
/// `notify` est appelé régulièrement pour rafraîchir l'affichage de la progression.
pub fn download_and_extract(
    install_dir: &Path,
    progress: &AtomicU64,
    total: &AtomicU64,
    notify: &dyn Fn(),
) -> Result<(), String> {
    let info = fetch_version_info()?;
    notify();

    let url = info.platform_download_url().ok_or_else(|| {
        format!(
            "Aucune version de Nds Game Studio n'est encore publiee pour {}.",
            ndsgs_common::PLATFORM
        )
    })?;

    fs::create_dir_all(install_dir).map_err(|e| format!("Impossible de creer le dossier: {}", e))?;

    let temp_zip = std::env::temp_dir().join("nds-game-studio-install.zip");
    download_file(url, &temp_zip, progress, total, notify)?;

    extract_zip(&temp_zip, install_dir)?;
    let _ = fs::remove_file(&temp_zip);
    ndsgs_common::clear_macos_quarantine(install_dir);
    Ok(())
}

fn download_file(
    url: &str,
    dest: &Path,
    progress: &AtomicU64,
    total_size: &AtomicU64,
    notify: &dyn Fn(),
) -> Result<(), String> {
    let agent = make_agent(120);

    let response = agent.get(url)
        .call()
        .map_err(|e| format!("Erreur telechargement: {}", e))?;

    let content_length = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);

    total_size.store(content_length, Ordering::Relaxed);

    let mut body = response.into_body();
    let mut file = fs::File::create(dest).map_err(|e| format!("Erreur fichier: {}", e))?;
    let mut downloaded: u64 = 0;
    let mut buf = [0u8; 8192];
    let mut last_notify = std::time::Instant::now();

    loop {
        let n = body.as_reader().read(&mut buf).map_err(|e| format!("Erreur lecture: {}", e))?;
        if n == 0 { break; }
        file.write_all(&buf[..n]).map_err(|e| format!("Erreur ecriture: {}", e))?;
        downloaded += n as u64;
        progress.store(downloaded, Ordering::Relaxed);

        if last_notify.elapsed() >= std::time::Duration::from_millis(100) {
            notify();
            last_notify = std::time::Instant::now();
        }
    }
    notify();

    if content_length > 0 && downloaded != content_length {
        return Err(format!("Telechargement incomplet: {} / {}", downloaded, content_length));
    }

    Ok(())
}

fn extract_zip(zip_path: &Path, install_dir: &Path) -> Result<(), String> {
    let file = fs::File::open(zip_path).map_err(|e| format!("Erreur ouverture zip: {}", e))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("Erreur zip: {}", e))?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| format!("Erreur zip entry: {}", e))?;
        let raw_name = entry.name().to_string();

        let relative = raw_name.split('/').skip(1).collect::<Vec<_>>().join("/");
        if relative.is_empty() { continue; }

        let out_path = install_dir.join(&relative);

        if entry.is_dir() {
            let _ = fs::create_dir_all(&out_path);
        } else {
            let mode = entry.unix_mode();
            ndsgs_common::write_extracted_file(&out_path, &mut entry, mode)
                .map_err(|e| format!("Erreur extraction {}: {}", relative, e))?;
        }
    }

    Ok(())
}
