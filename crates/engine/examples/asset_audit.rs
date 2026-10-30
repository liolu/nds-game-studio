//! Essai sans fenêtre de la fenêtre Assets : un fichier de chaque format de chaque jeu passe par le
//! code de l'aperçu, et l'index gardé (`assets_index.json`) est mis à jour.
//!
//! `cargo run --release -p nds-engine --example asset_audit -- <dossier des roms> <sortie.tsv> [--fresh]`

use std::fmt::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

use nds_engine::asset_scan::{all_formats, run_index, IndexState};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (root, out) = (Path::new(&args[0]), &args[1]);
    let fresh = args.iter().any(|a| a == "--fresh");
    let state = Arc::new(Mutex::new(IndexState::default()));
    run_index(root, &state, fresh);
    let state = state.lock().unwrap();

    let mut text = String::from("coche\tformat\tfichiers\tregardables\tjeux\tessayes\tmontres\texemple\tresultat\techec\n");
    for (ext, s) in all_formats(&state.stats) {
        writeln!(text, "{}\t{ext}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}", s.check(), s.count, s.viewable, s.games, s.tested, s.shown, s.sample.as_deref().unwrap_or(""), s.result.as_deref().unwrap_or(""), s.failure.as_deref().unwrap_or("")).unwrap();
    }
    text.push_str("\njeu\textrait %\tregardable %\tfichiers\tregardables\n");
    let mut labels: Vec<&String> = state.stats.keys().collect();
    labels.sort();
    for label in labels {
        let s = &state.stats[label];
        writeln!(text, "{label}\t{:.1}\t{:.1}\t{}\t{}", s.extracted_percent(), s.viewable_percent(), s.files, s.viewable).unwrap();
    }
    std::fs::write(out, text).unwrap();
    println!("{} jeux -> {out}", state.stats.len());
}
