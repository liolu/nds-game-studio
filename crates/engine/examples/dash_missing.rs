use std::collections::BTreeMap;
use std::path::Path;

use nds_engine::asset_scan::{choices_for, decode, game_stats, scan, Decoded};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (rom, out) = (Path::new(&args[0]), Path::new(&args[1]));
    let formats: Vec<&str> = args[2..].iter().map(String::as_str).collect();
    std::fs::create_dir_all(out).unwrap();
    let entries = scan(rom);
    let mut by: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for e in &entries {
        if !e.kind.viewable() {
            by.entry(format!("{} {:?}", e.format(), e.kind)).or_default().push(e.path.clone());
        }
    }
    for (k, v) in &by {
        println!("non montrable {k} : {} ({})", v.len(), v[0]);
    }
    let mut summary = String::new();
    for (i, e) in entries.iter().enumerate() {
        if !formats.is_empty() && !formats.contains(&e.format().as_str()) {
            continue;
        }
        let mut c = choices_for(&entries, i);
        let name = format!("{i:03}_{}", e.short_name().replace(['/', '#'], "_"));
        let line = match decode(&entries, i, &mut c) {
            Ok(Decoded::Image { rgba, info }) => {
                let mut bytes = format!("{} {}\n", rgba.width, rgba.height).into_bytes();
                bytes.extend(&rgba.data);
                std::fs::write(out.join(format!("{name}.rgba")), bytes).unwrap();
                format!("image {info}")
            }
            Ok(Decoded::Audio { wav, info, .. }) => {
                std::fs::write(out.join(format!("{name}.wav")), wav).unwrap();
                format!("son {info}")
            }
            Ok(Decoded::Text { lines, info }) => {
                std::fs::write(out.join(format!("{name}.txt")), lines.join("\n")).unwrap();
                format!("texte {info} ({} lignes)", lines.len())
            }
            Ok(_) => "autre".into(),
            Err(e) => format!("ECHEC {e}"),
        };
        summary.push_str(&format!("{}\t{:?}\t{line}\n", e.path, e.kind));
    }
    std::fs::write(out.join("resume.tsv"), summary).unwrap();
    let stats = game_stats(rom);
    println!("{} fichiers, {} regardables", stats.files, stats.viewable);
}
