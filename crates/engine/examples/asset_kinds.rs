//! Combien de fichiers de chaque sorte, pour un format : `asset_kinds -- <rom.nds> <format> [sorte]` ; avec une sorte,
//! la liste de ces fichiers.

use std::collections::BTreeMap;
use std::path::Path;

use nds_engine::asset_scan::scan;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let entries = scan(Path::new(&args[0]));
    let mine: Vec<_> = entries.iter().filter(|e| e.format() == args[1]).collect();
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for e in &mine {
        *kinds.entry(format!("{:?}", e.kind)).or_default() += 1;
    }
    println!("{kinds:?}");
    if let Some(kind) = args.get(2) {
        for e in mine.iter().filter(|e| format!("{:?}", e.kind) == *kind) {
            println!("{}  {}  {}", e.path, e.member_name.as_deref().unwrap_or(""), e.size);
        }
    }
}
