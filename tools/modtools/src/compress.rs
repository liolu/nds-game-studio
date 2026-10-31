//! Les compressions du BIOS de la DS, dans le sens inverse de `nds_nitro::decompress_any` : un fichier modifié
//! reprend la compression de l'original, car le jeu appelle souvent la routine d'une seule sorte.

use nds_nitro::decompress_any;

const MIN_MATCH: usize = 3;
const WINDOW: usize = 4096;
/// Essais par position : au-delà, on gagne peu et les gros fichiers deviennent lents.
const MAX_TRIES: usize = 256;

/// Trouve à chaque position la plus longue répétition dans les `WINDOW` derniers octets, à 2 octets au moins :
/// une distance de 1 casse la décompression vers la VRAM, qui s'écrit par 16 bits.
struct Matcher<'a> {
    data: &'a [u8],
    heads: Vec<i32>,
    previous: Vec<i32>,
}

impl<'a> Matcher<'a> {
    fn new(data: &'a [u8]) -> Self {
        Matcher { data, heads: vec![-1; 1 << 16], previous: vec![-1; data.len()] }
    }

    fn hash(&self, at: usize) -> usize {
        let d = self.data;
        (d[at] as usize ^ ((d[at + 1] as usize) << 5) ^ ((d[at + 2] as usize) << 10)) & 0xFFFF
    }

    fn insert(&mut self, at: usize) {
        if at + MIN_MATCH <= self.data.len() {
            let hash = self.hash(at);
            self.previous[at] = self.heads[hash];
            self.heads[hash] = at as i32;
        }
    }

    fn find(&self, at: usize, longest: usize) -> Option<(usize, usize)> {
        if at + MIN_MATCH > self.data.len() {
            return None;
        }
        let limit = longest.min(self.data.len() - at);
        let mut best: Option<(usize, usize)> = None;
        let mut candidate = self.heads[self.hash(at)];
        for _ in 0..MAX_TRIES {
            if candidate < 0 {
                break;
            }
            let from = candidate as usize;
            let distance = at - from;
            if distance > WINDOW {
                break;
            }
            if distance >= 2 {
                let length = (0..limit).take_while(|i| self.data[from + i] == self.data[at + i]).count();
                if length >= MIN_MATCH && best.is_none_or(|(l, _)| length > l) {
                    best = Some((length, distance));
                    if length == limit {
                        break;
                    }
                }
            }
            candidate = self.previous[from];
        }
        best
    }
}

fn header(kind: u8, size: usize) -> Result<Vec<u8>, String> {
    if size >= 1 << 24 {
        return Err(format!("fichier trop grand pour etre compresse ({size} octets)"));
    }
    Ok(vec![kind, size as u8, (size >> 8) as u8, (size >> 16) as u8])
}

fn pad4(out: &mut Vec<u8>) {
    while out.len() % 4 != 0 {
        out.push(0);
    }
}

fn lz(data: &[u8], kind: u8, longest: usize, token: impl Fn(&mut Vec<u8>, usize, usize)) -> Result<Vec<u8>, String> {
    let mut out = header(kind, data.len())?;
    let mut matcher = Matcher::new(data);
    let mut at = 0;
    while at < data.len() {
        let flags_at = out.len();
        out.push(0);
        for bit in (0..8).rev() {
            if at >= data.len() {
                break;
            }
            match matcher.find(at, longest) {
                Some((length, distance)) => {
                    out[flags_at] |= 1 << bit;
                    token(&mut out, length, distance);
                    for i in at..at + length {
                        matcher.insert(i);
                    }
                    at += length;
                }
                None => {
                    out.push(data[at]);
                    matcher.insert(at);
                    at += 1;
                }
            }
        }
    }
    pad4(&mut out);
    Ok(out)
}

/// LZ77 du BIOS, sorte 0x10.
pub fn lz10(data: &[u8]) -> Result<Vec<u8>, String> {
    lz(data, 0x10, 18, |out, length, distance| {
        out.push((((length - 3) << 4) | ((distance - 1) >> 8)) as u8);
        out.push((distance - 1) as u8);
    })
}

/// LZ77 étendu, sorte 0x11 : trois tailles de répétition.
pub fn lz11(data: &[u8]) -> Result<Vec<u8>, String> {
    lz(data, 0x11, 0x10110, |out, length, distance| {
        let d = distance - 1;
        if length <= 0x10 {
            out.push((((length - 1) << 4) | (d >> 8)) as u8);
        } else if length <= 0x110 {
            let l = length - 0x11;
            out.push((l >> 4) as u8);
            out.push((((l & 15) << 4) | (d >> 8)) as u8);
        } else {
            let l = length - 0x111;
            out.push((0x10 | (l >> 12)) as u8);
            out.push((l >> 4) as u8);
            out.push((((l & 15) << 4) | (d >> 8)) as u8);
        }
        out.push(d as u8);
    })
}

/// RLE du BIOS, sorte 0x30.
pub fn rle(data: &[u8]) -> Result<Vec<u8>, String> {
    const MAX_RUN: usize = 130;
    const MAX_COPY: usize = 128;
    let mut out = header(0x30, data.len())?;
    let mut at = 0;
    let mut literal = 0;
    while at <= data.len() {
        let run = if at < data.len() { data[at..].iter().take(MAX_RUN).take_while(|b| **b == data[at]).count() } else { 0 };
        if run >= 3 || at == data.len() || literal == MAX_COPY {
            if literal > 0 {
                out.push((literal - 1) as u8);
                out.extend(&data[at - literal..at]);
                literal = 0;
            }
            if at == data.len() {
                break;
            }
            if run >= 3 {
                out.push(0x80 | (run - 3) as u8);
                out.push(data[at]);
                at += run;
                continue;
            }
        }
        literal += 1;
        at += 1;
    }
    pad4(&mut out);
    Ok(out)
}

/// Écrit des bits dans des mots de 32 bits, le bit de poids fort d'abord.
struct BitWriter {
    out: Vec<u8>,
    word: u32,
    used: u32,
}

impl BitWriter {
    fn put(&mut self, value: u32, count: u32) {
        for shift in (0..count).rev() {
            self.word = (self.word << 1) | ((value >> shift) & 1);
            self.used += 1;
            if self.used == 32 {
                self.out.extend(self.word.to_le_bytes());
                (self.word, self.used) = (0, 0);
            }
        }
    }

    /// n bits à 1 (7 au plus, suivis d'un 0 si moins), puis les n bits bas : de 1 à 255.
    fn gamma(&mut self, value: u32) {
        let n = 31 - value.leading_zeros();
        self.put((1 << n) - 1, n);
        if n < 7 {
            self.put(0, 1);
        }
        self.put(value & ((1 << n) - 1), n);
    }

    fn finish(mut self) -> Vec<u8> {
        if self.used > 0 {
            self.word <<= 32 - self.used;
            self.out.extend(self.word.to_le_bytes());
        }
        self.out
    }
}

/// Le codeur propre aux jeux à `rom.bin` (sorte 6, voir `romarc::own_coder`), avec les réglages qu'il a souvent : 3 bits de
/// marqueur, pas de bits de distance supplémentaires, une table sans usage.
pub fn game_coder(data: &[u8]) -> Result<Vec<u8>, String> {
    const MARKER_BITS: u32 = 3;
    const LOW_BITS: u32 = 8 - MARKER_BITS;
    const LONGEST: usize = 256;
    /// Une distance gamma de 255 annonce la fin.
    const END: u32 = 255;
    let mut counts = [0usize; 1 << MARKER_BITS];
    for byte in data {
        counts[(*byte >> LOW_BITS) as usize] += 1;
    }
    let marker = (0..counts.len()).min_by_key(|m| counts[*m]).unwrap_or(0) as u32;
    let mut bits = BitWriter { out: header(0x60, data.len())?, word: 0, used: 0 };
    // Le jeu lit toujours un mot de table au moins (func_01ff8000) : une table vide décalerait tout de 4 octets
    bits.out.extend([4, marker as u8, 0, MARKER_BITS as u8, 0, 0, 0, 0]);
    let mut matcher = Matcher::new(data);
    let mut at = 0;
    while at < data.len() {
        match matcher.find(at, LONGEST) {
            Some((length, distance)) => {
                bits.put(marker, MARKER_BITS);
                bits.gamma(length as u32 - 1);
                let distance = distance as u32 - 1;
                bits.gamma((distance >> 8) + 1);
                bits.put(distance & 0xFF, 8);
                for i in at..at + length {
                    matcher.insert(i);
                }
                at += length;
            }
            None => {
                let byte = data[at] as u32;
                if byte >> LOW_BITS == marker {
                    // Octet qui commence comme le marqueur : `010`, le marqueur qui suit (le même), le bas de l'octet
                    bits.put(marker, MARKER_BITS);
                    bits.put(0b010, 3);
                    bits.put(marker, MARKER_BITS);
                } else {
                    bits.put(byte >> LOW_BITS, MARKER_BITS);
                }
                bits.put(byte & ((1 << LOW_BITS) - 1), LOW_BITS);
                matcher.insert(at);
                at += 1;
            }
        }
    }
    bits.put(marker, MARKER_BITS);
    bits.gamma(2);
    bits.gamma(END);
    Ok(bits.finish())
}

/// Un noeud de l'arbre de Huffman : une feuille (un symbole), ou deux enfants.
enum HuffNode {
    Leaf(u8),
    Pair(usize, usize),
}

/// L'arbre de ces fréquences : (noeuds, racine). Il a toujours deux feuilles au moins, le BIOS n'en lit pas moins.
fn huffman_tree(counts: &[usize]) -> (Vec<HuffNode>, usize) {
    let mut nodes: Vec<HuffNode> = Vec::new();
    let mut open: Vec<(usize, usize)> = Vec::new();
    for (symbol, count) in counts.iter().enumerate() {
        if *count > 0 {
            nodes.push(HuffNode::Leaf(symbol as u8));
            open.push((*count, nodes.len() - 1));
        }
    }
    while open.len() < 2 {
        let symbol = (0..counts.len()).find(|s| counts[*s] == 0).unwrap_or(0);
        nodes.push(HuffNode::Leaf(symbol as u8));
        open.push((0, nodes.len() - 1));
    }
    while open.len() > 1 {
        // Du plus lourd au plus léger : les deux derniers se rejoignent
        open.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
        let (Some(a), Some(b)) = (open.pop(), open.pop()) else { break };
        nodes.push(HuffNode::Pair(a.1, b.1));
        open.push((a.0 + b.0, nodes.len() - 1));
    }
    let root = nodes.len() - 1;
    (nodes, root)
}

/// La table de l'arbre telle que le BIOS la lit : sa taille, la racine, puis les paires d'enfants. Un noeud dit où sont ses enfants par
/// un décalage de 6 bits, soit 64 paires au plus devant lui : on range d'abord les plus petits sous-arbres, et un noeud dont le temps
/// presse passe devant. `None` si l'arbre n'y tient pas.
fn huffman_table(nodes: &[HuffNode], root: usize) -> Option<Vec<u8>> {
    const REACH: usize = 64;
    fn weight(nodes: &[HuffNode], node: usize, sizes: &mut Vec<usize>) -> usize {
        if let HuffNode::Pair(a, b) = nodes[node] {
            sizes[node] = 1 + weight(nodes, a, sizes) + weight(nodes, b, sizes);
        }
        sizes[node]
    }
    let mut sizes = vec![0; nodes.len()];
    let pairs = weight(nodes, root, &mut sizes);
    // Où poser les enfants de chaque noeud en attente : (noeud, position de son octet, dernière paire possible)
    let mut pending = vec![(root, 1usize, REACH - 1)];
    let mut table = vec![0u8; 2 + pairs * 2];
    for slot in 0..pairs {
        let late = pending.iter().enumerate().any(|(i, p)| p.2 < slot + i + 1);
        let choice = if late { 0 } else { (0..pending.len()).min_by_key(|&i| sizes[pending[i].0])? };
        let (node, at, last) = pending.remove(choice);
        if slot > last {
            return None;
        }
        let HuffNode::Pair(left, right) = nodes[node] else { return None };
        let first = 2 + slot * 2;
        table[at] = (slot + 1 - at / 2 - 1) as u8;
        for (child, leaf_flag, place) in [(left, 0x80u8, first), (right, 0x40u8, first + 1)] {
            match nodes[child] {
                HuffNode::Leaf(symbol) => {
                    table[place] = symbol;
                    table[at] |= leaf_flag;
                }
                HuffNode::Pair(..) => pending.push((child, place, slot + REACH)),
            }
        }
    }
    table.resize(table.len().next_multiple_of(4), 0);
    table[0] = (table.len() / 2 - 1) as u8;
    Some(table)
}

/// Huffman du BIOS, sortes 0x24 (par 4 bits, le bas de l'octet d'abord) et 0x28 (par octets).
pub fn huffman(data: &[u8], kind: u8) -> Result<Vec<u8>, String> {
    let symbols: Vec<u8> = if kind == 0x24 { data.iter().flat_map(|b| [b & 15, b >> 4]).collect() } else { data.to_vec() };
    let mut counts = vec![0usize; if kind == 0x24 { 16 } else { 256 }];
    for symbol in &symbols {
        counts[*symbol as usize] += 1;
    }
    let (nodes, root) = huffman_tree(&counts);
    let table = huffman_table(&nodes, root).ok_or("arbre de Huffman trop large pour la DS")?;
    let mut codes = vec![Vec::new(); counts.len()];
    let mut walk = vec![(root, Vec::new())];
    while let Some((node, code)) = walk.pop() {
        match nodes[node] {
            HuffNode::Leaf(symbol) => codes[symbol as usize] = code,
            HuffNode::Pair(left, right) => {
                let with = |bit: u32| code.iter().copied().chain([bit]).collect::<Vec<u32>>();
                walk.push((left, with(0)));
                walk.push((right, with(1)));
            }
        }
    }
    let mut out = header(kind, data.len())?;
    out.extend(table);
    let mut bits = BitWriter { out, word: 0, used: 0 };
    for symbol in symbols {
        for bit in &codes[symbol as usize] {
            bits.put(*bit, 1);
        }
    }
    Ok(bits.finish())
}

/// `new` compressé comme l'est `original`. Un fichier qui n'était pas compressé reste tel quel.
pub fn like(original: &[u8], new: &[u8]) -> Result<Vec<u8>, String> {
    if nds_nitro::unpack(original.to_vec()) == original {
        return Ok(new.to_vec());
    }
    same_method(original, new)
}

/// `new` compressé par la méthode que dit le premier octet de `original`.
pub fn same_method(original: &[u8], new: &[u8]) -> Result<Vec<u8>, String> {
    match original[0] {
        0x10 => lz10(new),
        0x11 => lz11(new),
        0x30 => rle(new),
        0x00 => {
            let mut out = header(0, new.len())?;
            out.extend(new);
            pad4(&mut out);
            Ok(out)
        }
        0x24 | 0x28 => huffman(new, original[0]),
        other => Err(format!("compression {other:#x} inconnue")),
    }
}

/// `new` repassé dans toutes les compressions de `original`, de la plus profonde à la première, comme `fully_unpacked` les retire.
pub fn same_layers(original: &[u8], new: &[u8]) -> Result<Vec<u8>, String> {
    fn pack(original: &[u8], new: &[u8], left: usize) -> Result<Vec<u8>, String> {
        match decompress_any(original).filter(|_| left > 0) {
            Some(inner) => same_method(original, &pack(&inner, new, left - 1)?),
            None => Ok(new.to_vec()),
        }
    }
    pack(original, new, crate::asset_scan::MAX_LAYERS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn huffman_comes_back_identical() {
        let random = |n: usize, seed: u32| -> Vec<u8> { (0..n as u32).map(|i| (i.wrapping_add(seed).wrapping_mul(2654435761).rotate_left(13).wrapping_mul(40503) >> 20) as u8).collect() };
        // Toutes les feuilles à la même profondeur (l'arbre le plus large), une pente (le plus profond), peu de symboles, un seul
        let flat: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        let steep: Vec<u8> = (0..24u32).flat_map(|s| vec![s as u8; 1usize << s.min(14)]).collect();
        let cases = [sample(), flat, steep, random(5000, 1), random(300, 7), b"abababab".to_vec(), vec![42; 100], vec![1]];
        for data in cases {
            for kind in [0x24, 0x28] {
                let packed = huffman(&data, kind).unwrap();
                assert_eq!(packed[0], kind);
                assert_eq!(packed.len() % 4, 0);
                assert_eq!((packed[4] as usize + 1) * 2 % 4, 0, "les bits commencent sur un mot");
                assert_eq!(decompress_any(&packed).as_deref(), Some(&data[..]), "sorte {kind:#x}, {} octets", data.len());
            }
        }
    }

    #[test]
    fn huffman_fits_any_tree_shape() {
        // Des fréquences très différentes d'un essai à l'autre : les enfants doivent toujours tenir dans les 6 bits
        let mut seed = 12345u32;
        for _ in 0..300 {
            let mut counts = vec![0usize; 256];
            for count in counts.iter_mut() {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let shift = seed >> 28;
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                *count = ((seed >> 16) as usize >> shift) + 1;
            }
            let (nodes, root) = huffman_tree(&counts);
            assert!(huffman_table(&nodes, root).is_some());
        }
    }

    #[test]
    fn layers_are_put_back_in_the_same_order() {
        let data = sample();
        let inner = lz10(&data).unwrap();
        let outer = huffman(&inner, 0x28).unwrap();
        let changed = [&data[..100], &[9, 9, 9, 9], &data[104..]].concat();
        let packed = same_layers(&outer, &changed).unwrap();
        assert_eq!(packed[0], 0x28);
        let middle = decompress_any(&packed).unwrap();
        assert_eq!(middle[0], 0x10);
        assert_eq!(decompress_any(&middle).unwrap(), changed);
    }

    fn sample() -> Vec<u8> {
        let mut data: Vec<u8> = b"RGCN motif motif motif ".iter().copied().cycle().take(5000).collect();
        data.extend((0..2000u32).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8));
        data.extend([7u8; 400]);
        data
    }

    #[test]
    fn every_method_comes_back_identical() {
        let data = sample();
        for packed in [lz10(&data).unwrap(), lz11(&data).unwrap(), rle(&data).unwrap()] {
            assert_eq!(decompress_any(&packed).as_deref(), Some(&data[..]));
            assert_eq!(packed.len() % 4, 0);
        }
    }

    #[test]
    fn the_game_coder_reads_back_with_the_games_rules() {
        let data = sample();
        let packed = game_coder(&data).unwrap();
        assert!(packed.len() < data.len());
        assert_eq!(nds_nitro::romarc::unpack(&packed), data);
        // Tous les octets possibles, aucun motif : le marqueur est forcément rencontré
        let noise: Vec<u8> = (0..4096u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
        let packed = game_coder(&noise).unwrap();
        if packed.len() <= noise.len() + 8 {
            assert_eq!(nds_nitro::romarc::unpack(&packed), noise);
        }
        let wide: Vec<u8> = (0..=255u8).cycle().take(3000).collect();
        assert_eq!(nds_nitro::romarc::unpack(&game_coder(&wide).unwrap()), wide);
    }

    #[test]
    fn repeats_keep_a_distance_of_2_for_vram() {
        // Une distance de 1 se reconnaît au bit de poids faible de la répétition : il vaut 0 pour distance - 1 = 0
        let data = vec![9u8; 100];
        let packed = lz10(&data).unwrap();
        assert_eq!(decompress_any(&[&packed[..], &[0; 40]].concat()).map(|d| d.len()), Some(100));
        assert_ne!(packed[7], 0, "la 1re repetition ne doit pas etre a distance 1");
    }

    #[test]
    fn keeps_the_compression_of_the_original() {
        let nitro = |extra: u8| {
            let mut file = b"RGCN".to_vec();
            file.extend(vec![extra; 200]);
            file
        };
        let original = lz10(&nitro(1)).unwrap();
        let packed = like(&original, &nitro(2)).unwrap();
        assert_eq!(packed[0], 0x10);
        assert_eq!(decompress_any(&packed).unwrap(), nitro(2));
        assert_eq!(like(&nitro(1), &nitro(3)).unwrap(), nitro(3), "pas compresse : reste tel quel");
    }
}
