//! Essai sans fenêtre : `cargo run -p nds-nitro --example probe -- <sortie.png> <fichier> [palette / tuiles / textures...]`
//! (un fichier dans une archive : `archive.narc#n`).

use nds_nitro::{g2d, model3d, sniff, unpack, Kind, Rgba};

fn load(arg: &str) -> Vec<u8> {
    let (path, entry) = match arg.rsplit_once('#') {
        Some((p, e)) => (p, e.parse::<usize>().ok()),
        None => (arg, None),
    };
    let bytes = unpack(std::fs::read(path).expect("lecture"));
    match entry {
        Some(i) => {
            let e = &nds_nitro::narc::read(&bytes).expect("narc")[i];
            unpack(bytes[e.start..e.end].to_vec())
        }
        None => bytes,
    }
}

fn save(out: &str, img: &Rgba) {
    image::save_buffer(out, &img.data, img.width, img.height, image::ExtendedColorType::Rgba8).expect("png");
    println!("-> {out} ({}x{})", img.width, img.height);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = &args[0];
    let files: Vec<Vec<u8>> = args[1..].iter().map(|a| load(a)).collect();
    let main = &files[0];
    let kinds: Vec<Kind> = files.iter().map(|f| sniff(f)).collect();
    println!("{kinds:?}");
    let find = |k: Kind| files.iter().zip(&kinds).find(|(_, kk)| **kk == k).map(|(f, _)| f);
    match kinds[0] {
        Kind::Model => {
            let own = model3d::read(main).unwrap();
            let others: Vec<_> = files[1..].iter().filter_map(|f| model3d::read(f).ok()).collect();
            for m in model3d::build_models(&own, &others) {
                println!("{} : {} parts, {} verts, {} tris, min {:?} max {:?}", m.name, m.parts.len(), m.vertex_count, m.triangle_count, m.min, m.max);
                for (i, mat) in m.materials.iter().enumerate() {
                    println!("  mat {i} {} tex {:?} {:?}", mat.name, mat.texture_name, mat.texture.as_ref().map(|t| (t.width, t.height)));
                }
                for w in &m.warnings {
                    println!("  ! {w}");
                }
                if let Some(t) = m.materials.iter().find_map(|m| m.texture.clone()) {
                    save(out, &t);
                }
            }
        }
        Kind::Textures => {
            let c = model3d::read(main).unwrap();
            for (name, img) in model3d::texture_sheet(&c) {
                println!("{name}: {:?}", img.as_ref().map(|i| (i.width, i.height)).map_err(|e| e.to_string()));
            }
        }
        Kind::Palette => save(out, &g2d::read_palette(main).unwrap().swatches()),
        Kind::Tiles => {
            let t = g2d::read_tiles(main).unwrap();
            let p = g2d::read_palette(find(Kind::Palette).unwrap()).unwrap();
            println!("{} tuiles, 8bpp {}, {:?}x{:?}", t.count(), t.bpp8, t.width, t.height);
            save(out, &g2d::render_tiles(&t, &p, 0, t.default_width()));
        }
        Kind::Screen => {
            let s = g2d::read_screen(main).unwrap();
            let t = g2d::read_tiles(find(Kind::Tiles).unwrap()).unwrap();
            let p = g2d::read_palette(find(Kind::Palette).unwrap()).unwrap();
            save(out, &g2d::render_screen(&s, &t, &p));
        }
        Kind::Cells => {
            let c = g2d::read_cells(main).unwrap();
            let t = g2d::read_tiles(find(Kind::Tiles).unwrap()).unwrap();
            let p = g2d::read_palette(find(Kind::Palette).unwrap()).unwrap();
            println!("{} cellules, mapping {}", c.cells.len(), c.mapping);
            save(out, &g2d::render_cell(&c, 0, &t, &p));
        }
        Kind::Archive => {
            for e in nds_nitro::narc::read(main).unwrap() {
                let head = unpack(main[e.start..e.end].to_vec());
                println!("#{} {} {:?} {} o", e.index, e.name, sniff(&head), e.end - e.start);
            }
        }
        k => println!("rien a montrer : {k:?}"),
    }
}
