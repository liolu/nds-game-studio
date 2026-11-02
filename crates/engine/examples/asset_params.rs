//! Essaie sur chaque jeu tous les décodeurs existants, pour les formats que le code commun lit mal, et
//! donne ce qui n'est vrai que pour ce jeu. Sans fenêtre ; rien n'est écrit sans `--write`.
//!
//! `cargo run --release -p nds-engine --example asset_params -- <dossier des roms> <rapport.tsv> [--write]`
//!
//! `--write` ajoute les lignes sûres (texte, compression) à `crates/engine/src/game_params.tsv`. Les autres
//! restent dans le rapport, marquées `a voir` ; une fois vues justes, elles s'ajoutent à la main.
//! `--images=<dossier>` y écrit deux images de chaque ligne (PPM), pour les regarder avant `--write`.

use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::Path;

use nds_engine::asset_scan::game_params::{kind_name, KINDS};
use nds_engine::asset_scan::{choices_for, decode, find_games, fully_unpacked, read_entry, scan, Decoded, Entry};
use nds_nitro::{decompress_any, Kind};

const SAMPLES: usize = 8;

/// Une image ne se croit qu'en la regardant : sur 60 jeux, presque toutes celles que les décodeurs
/// acceptaient étaient du bruit, même avec les contrôles de `plausible`.
fn needs_eyes(kind: Kind) -> bool {
    !matches!(kind, Kind::PlainText | Kind::Compressed)
}

/// Les octets ressemblent-ils à cette sorte ? Le décodeur seul ne refuse pas assez.
fn plausible(kind: Kind, raw: &[u8]) -> bool {
    let bytes = fully_unpacked(raw.to_vec());
    // Des tuiles ont beaucoup de pixels vides ; un fichier quelconque n'en a qu'une fois sur seize
    let has_blank_pixels = |data: &[u8]| {
        let blank: usize = data.iter().map(|b| usize::from(b & 15 == 0) + usize::from(b >> 4 == 0)).sum();
        !data.is_empty() && blank * 100 >= data.len() * 2 * 20
    };
    // Une signature (`SBNK`, `SSEQ`...) : ce fichier a son propre format, ce n'est pas une image sans en-tête
    let signed = bytes.len() >= 4 && bytes[..4].iter().all(|b| b.is_ascii_alphanumeric() || *b == b' ');
    if signed && !matches!(kind, Kind::PlainText | Kind::Bitmap) {
        return false;
    }
    match kind {
        Kind::RawTiles => bytes.len() >= 32 && bytes.len() % 32 == 0 && has_blank_pixels(&bytes),
        Kind::PaletteTiles => bytes.len() > 512 && (has_blank_pixels(&bytes[512..]) || has_blank_pixels(&bytes[32..])),
        Kind::RawPalette => {
            let high = bytes.chunks_exact(2).filter(|c| c[1] & 0x80 != 0).count();
            let colors = bytes.len() / 2;
            bytes.len() % 2 == 0 && (16..=4096).contains(&colors) && (high * 10 <= colors || high * 10 >= colors * 9)
        }
        Kind::RawScreen => matches!(bytes.len(), 1536 | 2048 | 4096 | 8192),
        Kind::RawTexture => false,
        Kind::Compressed => decompress_any(raw).is_some(),
        Kind::PlainText => !bytes.is_empty() && bytes.iter().filter(|b| matches!(**b, 9 | 10 | 13 | 32..=126)).count() * 100 >= bytes.len() * 98,
        Kind::Bitmap => bytes.starts_with(b"BM"),
        _ => true,
    }
}

/// Un texte de secours ne prouve rien : seule une sorte de texte peut finir en texte.
fn really_shown(entries: &[Entry], i: usize, kind: Kind) -> bool {
    let mut choices = choices_for(entries, i);
    match decode(entries, i, &mut choices) {
        Ok(Decoded::Image { rgba, .. }) => {
            let first = rgba.data.get(..4).unwrap_or(&[]);
            rgba.data.chunks_exact(4).any(|p| p[3] != 0 && p != first)
        }
        Ok(Decoded::Frames { frames, .. }) => !frames.is_empty(),
        Ok(Decoded::Model { model, .. }) => model.triangle_count > 0,
        Ok(Decoded::Text { .. }) => matches!(kind, Kind::PlainText | Kind::Compressed),
        Ok(_) | Err(_) => false,
    }
}

/// Le nombre de fichiers que `kind` montre vraiment.
fn shown_as(entries: &mut [Entry], group: &[usize], samples: &[usize], kind: Kind) -> usize {
    let old: Vec<Kind> = group.iter().map(|&i| entries[i].kind).collect();
    group.iter().for_each(|&i| entries[i].kind = kind);
    let count = samples.iter().filter(|&&i| read_entry(&entries[i]).is_ok_and(|raw| plausible(kind, &raw)) && really_shown(entries, i, kind)).count();
    group.iter().zip(old).for_each(|(&i, k)| entries[i].kind = k);
    count
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (root, out) = (Path::new(&args[0]), &args[1]);
    let write = args.iter().any(|a| a == "--write");
    let images = args.iter().find_map(|a| a.strip_prefix("--images=")).map(str::to_string);
    let mut report = String::from("code\tjeu\tmotif\tfichiers\tessayes\tmontres avant\tsorte\tmontres\tverdict\n");
    let mut sure = String::new();
    let games = find_games(root);
    for (n, game) in games.iter().enumerate() {
        let code = game.label.rsplit_once('(').map_or("", |(_, c)| c.trim_end_matches(')')).to_string();
        println!("{}/{} {}", n + 1, games.len(), game.label);
        let mut entries = scan(&game.rom);
        let mut groups: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
        for (i, e) in entries.iter().enumerate() {
            // Les formats que le code commun ne sait pas montrer ; un format reconnu par signature est déjà juste
            if e.kind == Kind::Other {
                groups.entry((e.format(), e.dir.clone())).or_default().push(i);
            }
        }
        let mut by_format: BTreeMap<String, Vec<(String, Vec<usize>)>> = BTreeMap::new();
        for ((format, dir), group) in groups {
            by_format.entry(format).or_default().push((dir, group));
        }
        for (format, dirs) in by_format {
            let whole: Vec<usize> = dirs.iter().flat_map(|(_, g)| g.iter().copied()).collect();
            // Le jeu entier d'abord ; sinon dossier par dossier
            let mut tries = vec![(format!("*.{format}"), whole)];
            if dirs.len() > 1 {
                tries.extend(dirs.into_iter().map(|(dir, group)| (format!("{dir}/*.{format}"), group)));
            }
            for (pattern, group) in tries {
                let step = (group.len() / SAMPLES).max(1);
                let samples: Vec<usize> = group.iter().copied().step_by(step).take(SAMPLES).collect();
                let before = shown_as(&mut entries, &group, &samples, Kind::Other);
                let fits = KINDS.iter().filter(|(_, k)| !matches!(k, Kind::Other | Kind::AnmFile)).map(|(_, k)| (*k, shown_as(&mut entries, &group, &samples, *k))).find(|(_, shown)| *shown == samples.len());
                let Some((kind, shown)) = fits else { continue };
                let safe = !needs_eyes(kind) && samples.len() >= 2;
                let name = kind_name(kind).unwrap_or("?");
                writeln!(report, "{code}\t{}\t{pattern}\t{}\t{}\t{before}\t{name}\t{shown}\t{}", game.label, group.len(), samples.len(), if safe { "sur" } else { "a voir" }).unwrap();
                if let Some(dir) = &images {
                    let group_kinds: Vec<Kind> = group.iter().map(|&i| entries[i].kind).collect();
                    group.iter().for_each(|&i| entries[i].kind = kind);
                    for (k, &i) in samples.iter().take(2).enumerate() {
                        let mut choices = choices_for(&entries, i);
                        if let Ok(Decoded::Image { rgba, .. }) = decode(&entries, i, &mut choices) {
                            let mut ppm = format!("P6 {} {} 255
", rgba.width, rgba.height).into_bytes();
                            rgba.data.chunks_exact(4).for_each(|p| ppm.extend_from_slice(&p[..3]));
                            let safe_name = pattern.replace(['/', '*', '.', '(', ')', ' '], "_");
                            std::fs::write(Path::new(dir).join(format!("{}_{code}_{name}_{safe_name}_{k}.ppm", if safe { "sur" } else { "voir" })), ppm).unwrap();
                        }
                    }
                    group.iter().zip(group_kinds).for_each(|(&i, k)| entries[i].kind = k);
                }
                if safe {
                    writeln!(sure, "{code}\t{pattern}\t{name}\t{} fichiers, {} essayes", group.len(), samples.len()).unwrap();
                }
                break; // tout le jeu passe : pas besoin des dossiers
            }
        }
    }
    std::fs::write(out, report).unwrap();
    println!("rapport : {out}");
    if write {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/game_params.tsv");
        let old = std::fs::read_to_string(&path).unwrap_or_default();
        std::fs::write(&path, old + &sure).unwrap();
        println!("table : {}", path.display());
    }
}
