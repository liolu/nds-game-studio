//! Les compressions du BIOS de la DS, dans le sens inverse de `nds_nitro::decompress_any` : un fichier modifié
//! reprend la compression de l'original, car le jeu appelle souvent la routine d'une seule sorte.

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
        other => Err(format!("compression {other:#x} (Huffman) : reecriture pas encore geree")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nds_nitro::decompress_any;

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
