//! Archives numérotées `rom.bin` (moteur Amaze : Lego Star Wars II, Les Sims 2...) : une table de u32
//! (le fichier n commence à table[n], la dernière valeur est la fin), sans noms. Le jeu charge « le
//! fichier n ».
//!
//! Chaque fichier peut être empaqueté : u32 d'en-tête, bits 4-7 = sorte, bits 8-31 = taille.
//! Sorte & 7 : 0 copie, 1 LZ77 du SDK, 2 Huffman du SDK, 3 RLE du SDK, 6 le codeur propre au jeu
//! (`own_coder`). Sorte & 8 : filtre delta sur 16 bits après le dépaquetage.
//!
//! Mêmes règles que `studio/gametools/romarc.py`, qui exécute le code ARM du jeu pour la sorte 6.

/// (position, taille) de chaque fichier ; vide si ce n'est pas une table.
pub fn entries(data: &[u8]) -> Vec<(usize, usize)> {
    let u32_at = |at: usize| data.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize);
    let Some(first) = u32_at(0) else { return Vec::new() };
    if data.len() < 8 || first % 4 != 0 || first < 8 || first > data.len() {
        return Vec::new();
    }
    let offsets: Vec<usize> = (0..first / 4).filter_map(|i| u32_at(i * 4)).collect();
    if offsets.windows(2).any(|w| w[1] < w[0]) || offsets.last().is_none_or(|last| *last > data.len()) {
        return Vec::new();
    }
    offsets.windows(2).map(|w| (w[0], w[1] - w[0])).collect()
}

/// (sorte, taille dépaquetée) si l'en-tête va avec le fichier.
fn header(blob: &[u8]) -> Option<(u8, usize)> {
    let h = u32::from_le_bytes(blob.get(0..4)?.try_into().ok()?);
    let (kind, size, len) = (((h >> 4) & 0xF) as u8, (h >> 8) as usize, blob.len());
    let k = kind & 7;
    if !matches!(k, 0 | 1 | 2 | 3 | 6) || size == 0 || size > 64 * len + 64 {
        return None;
    }
    let low = blob[0] & 0xF;
    let fits = match k {
        // LZ77 et RLE du SDK : 0x10 / 0x30, ça grandit
        1 | 3 => low == 0 && size + 8 >= len,
        // Copie : la taille est celle du fichier (plus le bourrage)
        0 => len.saturating_sub(8) < size && size + 4 <= len,
        // Huffman 4 ou 8 bits : ça grandit, 8 fois au plus
        2 => matches!(low, 4 | 8) && size + 8 >= len && len >= 8 + size * low as usize / 64,
        _ => size + 8 >= len,
    };
    (fits && (kind & 8 == 0 || size % 2 == 0)).then_some((kind, size))
}

fn lz77(src: &[u8], size: usize) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(size);
    let mut pos = 4;
    while out.len() < size {
        let flags = *src.get(pos)?;
        pos += 1;
        for bit in (0..8).rev() {
            if out.len() >= size {
                break;
            }
            if flags >> bit & 1 != 0 {
                let (a, b) = (*src.get(pos)? as usize, *src.get(pos + 1)? as usize);
                pos += 2;
                let (count, distance) = ((a >> 4) + 3, ((a & 0xF) << 8 | b) + 1);
                if distance > out.len() {
                    return None;
                }
                for _ in 0..count {
                    out.push(out[out.len() - distance]);
                }
            } else {
                out.push(*src.get(pos)?);
                pos += 1;
            }
        }
    }
    out.truncate(size);
    Some(out)
}

fn rle(src: &[u8], size: usize) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(size);
    let mut pos = 4;
    while out.len() < size {
        let flag = *src.get(pos)?;
        pos += 1;
        if flag & 0x80 != 0 {
            let value = *src.get(pos)?;
            pos += 1;
            out.resize(out.len() + (flag & 0x7F) as usize + 3, value);
        } else {
            let count = (flag & 0x7F) as usize + 1;
            out.extend_from_slice(src.get(pos..pos + count)?);
            pos += count;
        }
    }
    out.truncate(size);
    Some(out)
}

fn huffman(src: &[u8], size: usize) -> Option<Vec<u8>> {
    let bits = src[0] & 0xF;
    let tree = 4;
    let mut pos = tree + (*src.get(4)? as usize + 1) * 2;
    let mut node = tree + 1;
    let (mut out, mut current, mut half) = (Vec::with_capacity(size), 0u8, false);
    while out.len() < size && pos + 4 <= src.len() {
        let word = u32::from_le_bytes(src[pos..pos + 4].try_into().ok()?);
        pos += 4;
        for shift in (0..32).rev() {
            let bit = (word >> shift & 1) as usize;
            let offset = *src.get(node)?;
            let child = (node & !1) + (offset as usize & 0x3F) * 2 + 2 + bit;
            let leaf = offset & (if bit == 1 { 0x40 } else { 0x80 }) != 0;
            if leaf {
                let symbol = *src.get(child)?;
                if bits == 8 {
                    out.push(symbol);
                } else if half {
                    out.push(current | (symbol & 0xF) << 4);
                    half = false;
                } else {
                    current = symbol & 0xF;
                    half = true;
                }
                node = tree + 1;
                if out.len() >= size {
                    break;
                }
            } else {
                node = child;
            }
        }
    }
    out.truncate(size);
    Some(out)
}

/// Bits du codeur du jeu : des mots de 32 bits, le bit de poids fort d'abord.
struct Bits<'a> {
    src: &'a [u8],
    pos: usize,
    word: u32,
    left: u32,
}

impl Bits<'_> {
    fn fill(&mut self) -> Option<()> {
        if self.left == 0 {
            let bytes = self.src.get(self.pos..self.pos + 4)?;
            self.word = u32::from_le_bytes(bytes.try_into().ok()?);
            self.pos += 4;
            self.left = 32;
        }
        Some(())
    }

    fn peek(&mut self) -> Option<bool> {
        self.fill()?;
        Some(self.word >> 31 != 0)
    }

    fn bit(&mut self) -> Option<u32> {
        self.fill()?;
        let bit = self.word >> 31;
        self.word <<= 1;
        self.left -= 1;
        Some(bit)
    }

    fn read(&mut self, count: u32) -> Option<u32> {
        let mut value = 0;
        for _ in 0..count {
            value = value << 1 | self.bit()?;
        }
        Some(value)
    }

    /// n bits à 1 (7 au plus, suivis d'un 0 si moins), puis n bits : de 1 à 255.
    fn gamma(&mut self) -> Option<u32> {
        let mut n = 0;
        while n < 7 && self.bit()? == 1 {
            n += 1;
        }
        Some(1 << n | self.read(n)?)
    }
}

/// Le codeur propre au jeu (sorte 6, func_01ff8000 en ITCM) : un LZ à codes gamma. En-tête (u32) :
/// taille de la table, marqueur, bits b de la distance, bits a du marqueur ; puis la table des valeurs
/// fréquentes. Chaque élément commence par a bits : autre chose que le marqueur = un octet (avec les
/// 8 - a bits suivants). Le marqueur annonce :
/// - `00` + 8 bits : 2 octets copiés à cette distance ;
/// - `010` : nouveau marqueur (a bits), puis un octet dont le haut est l'ancien marqueur ;
/// - `011` : un octet répété (longueur gamma, valeur gamma : dans la table, ou calculée) ;
/// - `1...` : copie longue (longueur gamma, distance gamma + b bits + 8 bits) ; distance 255 = la fin.
fn own_coder(blob: &[u8], size: usize) -> Option<Vec<u8>> {
    let h = u32::from_le_bytes(blob.get(4..8)?.try_into().ok()?);
    let (table_size, mut marker) = ((h & 0xFF) as usize, h >> 8 & 0xFF);
    let (far_bits, marker_bits) = (h >> 16 & 0xFF, h >> 24 & 0xFF);
    if marker_bits > 8 {
        return None;
    }
    let low_bits = 8 - marker_bits;
    let table = blob.get(8..8 + table_size)?;
    let mut bits = Bits { src: blob, pos: 8 + table_size.div_ceil(4) * 4, word: 0, left: 0 };
    let mut out: Vec<u8> = Vec::with_capacity(size);
    loop {
        let head = bits.read(marker_bits)?;
        if head != marker {
            out.push((head << low_bits | bits.read(low_bits)?) as u8);
            continue;
        }
        let (length, distance);
        if bits.peek()? {
            length = bits.gamma()? as usize + 1;
            let far = bits.gamma()?;
            if far == 0xFF {
                break;
            }
            let high = (far - 1) << far_bits | bits.read(far_bits)?;
            distance = (high << 8 | bits.read(8)?) as usize + 1;
        } else {
            bits.bit()?;
            if bits.bit()? == 0 {
                (length, distance) = (2, bits.read(8)? as usize + 1);
            } else if bits.bit()? == 0 {
                let new_marker = bits.read(marker_bits)?;
                out.push((marker << low_bits | bits.read(low_bits)?) as u8);
                marker = new_marker;
                continue;
            } else {
                let mut base = bits.gamma()?;
                if base >= 0x80 {
                    let low = (base - 0x80) * 2 + bits.bit()?;
                    base = low | (bits.gamma()? - 1) << 8;
                }
                let value = bits.gamma()?;
                let byte = if value < 0x20 { *table.get(value as usize - 1)? } else { ((value - 0x20) << 3 | bits.read(3)?) as u8 };
                out.resize(out.len() + base as usize + 1, byte);
                if out.len() > size + 1 {
                    return None;
                }
                continue;
            }
        }
        if distance > out.len() || out.len() + length > size + 1 {
            return None;
        }
        for _ in 0..length {
            out.push(out[out.len() - distance]);
        }
    }
    (out.len() + 1 >= size).then(|| {
        out.resize(size, 0);
        out
    })
}

/// Le fichier dépaqueté. Un fichier dont le début ressemble seulement à un en-tête (des données brutes,
/// comme deux fichiers de Lego) est rendu tel quel.
pub fn unpack(blob: &[u8]) -> Vec<u8> {
    let Some((kind, size)) = header(blob) else { return blob.to_vec() };
    let out = match kind & 7 {
        0 => Some(blob[4..4 + size].to_vec()),
        1 => lz77(blob, size),
        2 => huffman(blob, size),
        3 => rle(blob, size),
        _ => own_coder(blob, size),
    };
    let Some(mut out) = out else { return blob.to_vec() };
    if kind & 8 != 0 {
        let mut previous = 0u16;
        for pair in out.chunks_exact_mut(2) {
            previous = previous.wrapping_add(u16::from_le_bytes([pair[0], pair[1]]));
            pair.copy_from_slice(&previous.to_le_bytes());
        }
    }
    out
}

/// Extension d'un fichier d'après sa signature, comme `romarc.py extract`.
pub fn extension(data: &[u8]) -> &'static str {
    match data.get(0..4) {
        Some(b"BMD0") => "nsbmd",
        Some(b"BTX0") => "nsbtx",
        Some(b"BCA0") => "nsbca",
        Some(b"BVA0") => "nsbva",
        Some(b"BTA0") => "nsbta",
        Some(b"BTP0") => "nsbtp",
        Some(b"BMA0") => "nsbma",
        Some(b"RGCN") => "ncgr",
        Some(b"RLCN") => "nclr",
        Some(b"RCSN") => "nscr",
        Some(b"RECN") => "ncer",
        Some(b"RNAN") => "nanr",
        Some(b"SDAT") => "sdat",
        _ => "bin",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_offset_table() {
        // 3 valeurs = 2 fichiers
        let mut data: Vec<u8> = [12u32, 14, 17].iter().flat_map(|v| v.to_le_bytes()).collect();
        data.extend(b"AABBB");
        assert_eq!(entries(&data), [(12, 2), (14, 3)]);
        assert!(entries(b"pas une table").is_empty());
        assert!(entries(&[]).is_empty());
    }

    #[test]
    fn unpacks_each_kind() {
        // Copie : sorte 0, 4 octets
        let copy = [0x00, 4, 0, 0, b'a', b'b', b'c', b'd'];
        assert_eq!(unpack(&copy), b"abcd");
        // LZ77 : 3 littéraux, puis 3 octets copiés à distance 3
        let lz = [0x10, 6, 0, 0, 0b0001_0000, b'x', b'y', b'z', 0x00, 0x02];
        assert_eq!(unpack(&lz), b"xyzxyz");
        // RLE : 4 fois 'q', puis 2 littéraux
        let run = [0x30, 6, 0, 0, 0x81, b'q', 0x01, b'r', b's'];
        assert_eq!(unpack(&run), b"qqqqrs");
        // Copie + delta : sorte 8 ; 1, +1, +1 sur 16 bits
        let delta = [0x80, 6, 0, 0, 1, 0, 1, 0, 1, 0];
        assert_eq!(unpack(&delta), [1, 0, 2, 0, 3, 0]);
    }

    /// Écrit des bits comme le codeur du jeu les lit.
    #[derive(Default)]
    struct Writer {
        bits: Vec<u8>,
    }

    impl Writer {
        fn put(&mut self, value: u32, count: u32) {
            for i in (0..count).rev() {
                self.bits.push((value >> i & 1) as u8);
            }
        }

        fn gamma(&mut self, value: u32) {
            let n = 31 - value.leading_zeros();
            for _ in 0..n {
                self.bits.push(1);
            }
            if n < 7 {
                self.bits.push(0);
            }
            self.put(value & ((1 << n) - 1), n);
        }

        fn words(&self) -> Vec<u8> {
            let mut out = Vec::new();
            for chunk in self.bits.chunks(32) {
                let word = chunk.iter().enumerate().fold(0u32, |w, (i, b)| w | (*b as u32) << (31 - i));
                out.extend(word.to_le_bytes());
            }
            out
        }
    }

    #[test]
    fn reads_the_games_own_coder() {
        // a = 4 bits de marqueur (0xA), b = 1 bit, table de 4 octets
        let mut w = Writer::default();
        w.put(0x41, 8); // A
        w.put(0x42, 8); // B
        w.put(0xA, 4); // 00 + distance 2 : copie AB
        w.put(0b00, 2);
        w.put(1, 8);
        w.put(0xA, 4); // 011 : Z depuis la table, 3 fois (gamma 2 + 1)
        w.put(0b011, 3);
        w.gamma(2);
        w.gamma(2);
        w.put(0xA, 4); // 010 : nouveau marqueur 0xB, octet 0xA7
        w.put(0b010, 3);
        w.put(0xB, 4);
        w.put(0x7, 4);
        w.put(0xA5, 8); // 0xA n'est plus le marqueur : un simple octet
        w.put(0xB, 4); // 1... : longueur 3 (gamma 2 + 1), distance 3 (0 << 9 | 2 + 1), recopie Z Z A7 -> Z A7 A5
        w.gamma(2);
        w.gamma(1);
        w.put(0, 1);
        w.put(2, 8);
        w.put(0xB, 4); // 011 : Z encore 40 fois (l'en-tête veut un fichier qui grandit)
        w.put(0b011, 3);
        w.gamma(39);
        w.gamma(2);
        w.put(0xB, 4); // fin : longueur quelconque, distance 255
        w.gamma(2);
        w.gamma(0xFF);
        let mut expected = vec![b'A', b'B', b'A', b'B', b'Z', b'Z', b'Z', 0xA7, 0xA5, b'Z', 0xA7, 0xA5];
        expected.resize(expected.len() + 40, b'Z');
        let mut blob = vec![0x60, expected.len() as u8, 0, 0, 4, 0xA, 1, 4];
        blob.extend([0, b'Z', 0, 0]);
        blob.extend(w.words());
        assert_eq!(unpack(&blob), expected);
    }

    #[test]
    fn what_only_looks_packed_is_kept_as_is() {
        let raw = [0x60, 16, 0, 0, 1, 2, 3, 4];
        assert_eq!(unpack(&raw), raw);
    }

    #[test]
    fn not_a_packed_header_is_kept_as_is() {
        let raw = *b"BMD0 du contenu qui ne ressemble pas a un en-tete";
        assert_eq!(unpack(&raw), raw);
        assert_eq!(extension(&raw), "nsbmd");
    }
}
