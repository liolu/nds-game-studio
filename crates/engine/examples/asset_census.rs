//! Les fichiers d'une rom que la fenêtre Assets ne sait pas montrer, par format et par sorte.
//!
//! `cargo run --release -p nds-engine --example asset_census -- <rom.nds> [format]`

use std::collections::BTreeMap;
use std::path::Path;

use nds_engine::asset_scan::{read_entry, scan};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let entries = scan(Path::new(&args[0]));
    let only = args.get(1);
    let mut groups: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
    let (mut total, mut viewable) = (0, 0);
    for (i, e) in entries.iter().enumerate() {
        total += 1;
        if e.kind.viewable() {
            viewable += 1;
            continue;
        }
        let head = if std::env::var("BY_HEAD").is_ok() { read_entry(e).map(|b| b.iter().take(2).map(|x| format!("{x:02x}")).collect::<String>()).unwrap_or_default() } else { String::new() };
        groups.entry((format!("{} {head}", e.format()), format!("{:?}", e.kind))).or_default().push(i);
    }
    println!("{total} fichiers, {viewable} regardables");
    let mut list: Vec<_> = groups.into_iter().collect();
    list.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
    for ((ext, kind), ids) in list {
        if only.is_some_and(|o| *o != ext) {
            continue;
        }
        println!("{:6} {ext:16} {kind}", ids.len());
        for &i in ids.iter().take(if only.is_some() { 40 } else { 3 }) {
            let e = &entries[i];
            let head = read_entry(e).map(|b| b.iter().take(24).map(|x| format!("{x:02x}")).collect::<String>()).unwrap_or_else(|err| err);
            println!("         {} ({} o) {head}", e.path, e.size);
        }
    }
}
