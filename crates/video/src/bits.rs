//! Lecture de bits et codes de longueur variable.

pub type R<T = ()> = Result<T, &'static str>;

/// Lecteur de bits, du bit de poids fort au plus faible (VX et Mobiclip, mots déjà retournés).
pub struct Msb {
    data: Vec<u8>,
    pos: usize,
}

impl Msb {
    /// Les mots de 16 bits du fichier sont en petit-boutiste : on les retourne une fois pour toutes.
    pub fn from_words(bytes: &[u8]) -> Msb {
        let mut data = bytes.to_vec();
        if data.len() % 2 == 1 {
            data.push(0);
        }
        for word in data.chunks_exact_mut(2) {
            word.swap(0, 1);
        }
        Msb { data, pos: 0 }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn left(&self) -> isize {
        (self.data.len() * 8) as isize - self.pos as isize
    }

    /// Les `n` bits suivants (n <= 32) sans avancer ; des zéros après la fin.
    #[inline]
    pub fn peek(&self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        let start = self.pos / 8;
        let mut value = 0u64;
        for i in 0..8 {
            value = (value << 8) | *self.data.get(start + i).unwrap_or(&0) as u64;
        }
        ((value << (self.pos % 8)) >> (64 - n)) as u32
    }

    #[inline]
    pub fn skip(&mut self, n: usize) {
        self.pos += n;
    }

    #[inline]
    pub fn get(&mut self, n: u32) -> u32 {
        let value = self.peek(n);
        self.pos += n as usize;
        value
    }

    #[inline]
    pub fn bit(&mut self) -> bool {
        self.get(1) != 0
    }

    #[inline]
    pub fn signed(&mut self, n: u32) -> i32 {
        let value = self.get(n) as i32;
        (value << (32 - n)) >> (32 - n)
    }

    /// Exp-Golomb non signé.
    #[inline]
    pub fn ue(&mut self) -> u32 {
        let zeros = self.peek(32).leading_zeros();
        if zeros >= 32 {
            self.pos += 32;
            return u32::MAX;
        }
        self.pos += zeros as usize + 1;
        ((1u64 << zeros) - 1 + self.get(zeros) as u64) as u32
    }

    /// Exp-Golomb signé : 0, 1, -1, 2, -2...
    #[inline]
    pub fn se(&mut self) -> i32 {
        let code = self.ue() as i64;
        (if code & 1 == 1 { (code + 1) / 2 } else { -(code / 2) }) as i32
    }

    /// Aligne sur le mot de 16 bits suivant.
    pub fn align16(&mut self) {
        self.pos = self.pos.div_ceil(16) * 16;
    }
}

/// Lecteur de bits, du bit de poids faible au plus fort (Bink).
pub struct Lsb<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Lsb<'a> {
    pub fn new(data: &'a [u8]) -> Lsb<'a> {
        Lsb { data, pos: 0 }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn left(&self) -> isize {
        (self.data.len() * 8) as isize - self.pos as isize
    }

    #[inline]
    pub fn peek(&self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        let start = self.pos / 8;
        let mut value = 0u64;
        for i in 0..8 {
            value |= (*self.data.get(start + i).unwrap_or(&0) as u64) << (8 * i);
        }
        ((value >> (self.pos % 8)) & ((1u64 << n) - 1)) as u32
    }

    #[inline]
    pub fn skip(&mut self, n: usize) {
        self.pos += n;
    }

    #[inline]
    pub fn get(&mut self, n: u32) -> u32 {
        let value = self.peek(n);
        self.pos += n as usize;
        value
    }

    #[inline]
    pub fn bit(&mut self) -> bool {
        self.get(1) != 0
    }

    /// Valeur suivie de son signe s'il n'est pas nul.
    #[inline]
    pub fn sign(&mut self, value: i32) -> i32 {
        if value != 0 && self.bit() { -value } else { value }
    }

    pub fn align32(&mut self) {
        self.pos = self.pos.div_ceil(32) * 32;
    }
}

/// Table de décodage directe : les `max` bits suivants donnent (symbole, longueur).
pub struct Vlc {
    table: Vec<(u16, u8)>,
    max: u32,
}

impl Vlc {
    fn with_max(max: u32) -> Vlc {
        Vlc { table: vec![(0, 0); 1 << max], max }
    }

    /// Codes écrits en poids fort d'abord : (longueur, code, symbole). Longueur 0 = pas de code.
    pub fn from_codes(codes: impl Iterator<Item = (u8, u32, u16)> + Clone) -> Vlc {
        let max = codes.clone().map(|c| c.0 as u32).max().unwrap_or(1).max(1);
        let mut vlc = Vlc::with_max(max);
        for (len, code, symbol) in codes {
            if len == 0 {
                continue;
            }
            let shift = max - len as u32;
            let first = (code as usize) << shift;
            for slot in &mut vlc.table[first..first + (1 << shift)] {
                *slot = (symbol, len);
            }
        }
        vlc
    }

    /// Codes donnés par leurs seules longueurs, attribués dans l'ordre (comme FFmpeg).
    pub fn from_lengths(lens: &[u8], syms: impl Fn(usize) -> u16) -> Vlc {
        let max = lens.iter().map(|&l| l as u32).max().unwrap_or(1).max(1);
        let mut vlc = Vlc::with_max(max);
        let mut code = 0u64;
        for (i, &len) in lens.iter().enumerate() {
            if len == 0 {
                continue;
            }
            let first = (code >> (32 - max)) as usize;
            let count = 1usize << (max - len as u32);
            for slot in &mut vlc.table[first..first + count] {
                *slot = (syms(i), len);
            }
            code += 1u64 << (32 - len as u32);
        }
        vlc
    }

    /// Code écrit en poids faible d'abord (Bink).
    pub fn from_codes_lsb(lens: &[u8], codes: &[u8]) -> Vlc {
        let max = lens.iter().map(|&l| l as u32).max().unwrap_or(1).max(1);
        let mut vlc = Vlc::with_max(max);
        for (i, (&len, &code)) in lens.iter().zip(codes).enumerate() {
            if len == 0 {
                continue;
            }
            for high in 0..1usize << (max - len as u32) {
                vlc.table[code as usize | high << len] = (i as u16, len);
            }
        }
        vlc
    }

    #[inline]
    pub fn read(&self, gb: &mut Msb) -> R<u16> {
        let (symbol, len) = self.table[gb.peek(self.max) as usize];
        if len == 0 {
            return Err("code inconnu");
        }
        gb.skip(len as usize);
        Ok(symbol)
    }

    #[inline]
    pub fn read_lsb(&self, gb: &mut Lsb) -> u16 {
        let (symbol, len) = self.table[gb.peek(self.max) as usize];
        gb.skip(len.max(1) as usize);
        symbol
    }
}

#[inline]
pub fn clip8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golomb_codes() {
        // 1 | 010 | 011 | 00100 | 00101, en mots retournés
        let word: u16 = 0b1010_0110_0100_0010;
        let mut gb = Msb::from_words(&word.to_le_bytes());
        assert_eq!(gb.ue(), 0);
        assert_eq!(gb.se(), 1);
        assert_eq!(gb.se(), -1);
        assert_eq!(gb.ue(), 3);
        assert_eq!(gb.get(3), 0b001);
    }

    #[test]
    fn codes_from_lengths_are_given_in_order() {
        let vlc = Vlc::from_lengths(&[1, 2, 2], |i| i as u16);
        let word: u16 = 0b0_10_11_000_0000_0000;
        let mut gb = Msb::from_words(&word.to_le_bytes());
        assert_eq!(vlc.read(&mut gb), Ok(0));
        assert_eq!(vlc.read(&mut gb), Ok(1));
        assert_eq!(vlc.read(&mut gb), Ok(2));
    }
}
