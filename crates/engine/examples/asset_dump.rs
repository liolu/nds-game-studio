//! Écrit des fichiers d'une rom tels que la fenêtre Assets les lit (dépaquetés), avec leur sorte et ce que l'aperçu
//! en montre ; les images de l'aperçu sont écrites en PNG, les sons en WAV.
//!
//! `cargo run --release -p nds-engine --example asset_dump -- <rom.nds> <dossier> <chemin>...` ; un chemin qui finit
//! par `#` prend tous les morceaux du fichier.

use std::path::Path;

use nds_engine::asset_scan::{choices_for, decode, read_entry, scan, Decoded};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let entries = scan(Path::new(&args[0]));
    let out = Path::new(&args[1]);
    for wanted in &args[2..] {
        for (i, e) in entries.iter().enumerate().filter(|(_, e)| e.path == *wanted || (wanted.ends_with('#') && e.path.starts_with(wanted.as_str()))) {
            let bytes = read_entry(e).unwrap_or_default();
            let file = out.join(e.path.replace(['/', '#'], "_"));
            let _ = std::fs::write(&file, bytes.as_slice());
            let mut c = choices_for(&entries, i);
            let shown = match decode(&entries, i, &mut c) {
                Ok(Decoded::Image { rgba, info }) => {
                    let _ = image::save_buffer(format!("{}.png", file.display()), &rgba.data, rgba.width, rgba.height, image::ColorType::Rgba8);
                    format!("image {}x{} {info}", rgba.width, rgba.height)
                }
                Ok(Decoded::Audio { wav, info, .. }) => {
                    let _ = std::fs::write(format!("{}.wav", file.display()), wav);
                    format!("son {info}")
                }
                Ok(Decoded::Text { lines, info }) => format!("texte {} lignes {info} | {}", lines.len(), lines.first().map_or("", |l| l.as_str())),
                Ok(Decoded::Frames { frames, info }) => format!("{} images {info}", frames.len()),
                Ok(Decoded::Model { .. } | Decoded::Animated { .. }) => "modele".into(),
                Ok(Decoded::Video { info, .. }) => format!("video {info}"),
                Err(err) => format!("ERREUR {err}"),
            };
            println!("{:32} {:24} {:?} {} o : {shown}", e.path, e.member_name.as_deref().unwrap_or(""), e.kind, bytes.len());
        }
    }
}
