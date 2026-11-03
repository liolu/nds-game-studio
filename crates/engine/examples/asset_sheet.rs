//! Une planche PNG des aperçus d'une sorte de fichiers d'une rom, pour les juger à l'œil.
//!
//! `cargo run --release -p nds-engine --example asset_sheet -- <rom.nds> <sorte, ex. RomSprite> <sortie.png> [nombre]`

use std::path::Path;

use nds_engine::asset_scan::{choices_for, decode, scan, Decoded};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let entries = scan(Path::new(&args[0]));
    let wanted: usize = args.get(3).and_then(|n| n.parse().ok()).unwrap_or(40);
    let picked: Vec<usize> = (0..entries.len()).filter(|&i| format!("{:?}", entries[i].kind) == args[1]).collect();
    let step = (picked.len() / wanted).max(1);
    let mut images = Vec::new();
    let (mut failed, mut text) = (0, 0);
    for &i in picked.iter().step_by(step).take(wanted) {
        let mut c = choices_for(&entries, i);
        match decode(&entries, i, &mut c) {
            Ok(Decoded::Image { rgba, .. }) => images.push(rgba),
            Ok(Decoded::Frames { frames, .. }) => images.extend(frames.into_iter().take(6).map(|f| f.0)),
            Ok(Decoded::Text { lines, info }) => {
                text += 1;
                if text <= 3 {
                    println!("{} : {info}", entries[i].path);
                    lines.iter().take(12).for_each(|l| println!("   {l}"));
                }
            }
            Ok(_) => {}
            Err(e) => {
                failed += 1;
                println!("{} : ECHEC {e}", entries[i].path);
            }
        }
    }
    println!("{} fichiers de cette sorte, {} essayes, {failed} echecs, {text} en texte", picked.len(), picked.len().min(wanted));
    if images.is_empty() {
        return;
    }
    // Chaque image agrandie 2 fois, en lignes de 1200 pixels au plus
    let (gap, scale, width) = (4u32, 2u32, 1200u32);
    let (mut x, mut y, mut line, mut places) = (0, 0, 0, Vec::new());
    for img in &images {
        let w = img.width * scale;
        if x > 0 && x + w > width {
            (x, y, line) = (0, y + line + gap, 0);
        }
        places.push((x, y));
        x += w + gap;
        line = line.max(img.height * scale);
    }
    let mut out = image::RgbaImage::from_pixel(width, y + line, image::Rgba([60, 0, 60, 255]));
    for (img, (px, py)) in images.iter().zip(places) {
        for yy in 0..img.height * scale {
            for xx in 0..img.width * scale {
                let at = (((yy / scale) * img.width + xx / scale) * 4) as usize;
                let p = &img.data[at..at + 4];
                if p[3] > 0 && px + xx < width {
                    out.put_pixel(px + xx, py + yy, image::Rgba([p[0], p[1], p[2], 255]));
                }
            }
        }
    }
    out.save(&args[2]).unwrap();
}
