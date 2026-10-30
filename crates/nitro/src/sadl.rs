//! Son `sadl` (Professor Layton, Luminous Arc...) : un en-tête de 0x100 octets puis des blocs de 16 octets
//! par voie, en IMA ADPCM (2 échantillons par octet) ou en Procyon ADPCM (30 échantillons par bloc).
//! Octet 0x32 = nombre de voies ; octet 0x33 : bits 1-2 = fréquence, bits 4-7 = codage.

use crate::errors::Result;

pub struct Sound {
    pub rate: u32,
    pub channels: u16,
    /// Échantillons entrelacés.
    pub samples: Vec<i16>,
}

const DATA_AT: usize = 0x100;
const BLOCK: usize = 16;

const IMA_STEPS: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871,
    5358, 5894, 6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];
const IMA_INDEX: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];
const PROCYON: [[i32; 2]; 5] = [[0, 0], [60, 0], [115, -52], [98, -55], [122, -60]];

fn clamp16(value: i32) -> i16 {
    value.clamp(-32768, 32767) as i16
}

/// Une voie en IMA ADPCM : le quartet faible de chaque octet d'abord.
fn ima(blocks: impl Iterator<Item = u8>) -> Vec<i16> {
    let (mut sample, mut index) = (0i32, 0i32);
    let mut out = Vec::new();
    for byte in blocks {
        for nibble in [byte & 0xF, byte >> 4] {
            let step = IMA_STEPS[index as usize];
            let mut delta = step >> 3;
            if nibble & 4 != 0 {
                delta += step;
            }
            if nibble & 2 != 0 {
                delta += step >> 1;
            }
            if nibble & 1 != 0 {
                delta += step >> 2;
            }
            sample = (if nibble & 8 != 0 { sample - delta } else { sample + delta }).clamp(-32768, 32767);
            index = (index + IMA_INDEX[(nibble & 7) as usize]).clamp(0, 88);
            out.push(sample as i16);
        }
    }
    out
}

/// Une voie en Procyon ADPCM : 15 octets de quartets, puis l'octet qui donne l'échelle et le filtre.
fn procyon(blocks: impl Iterator<Item = [u8; BLOCK]>) -> Vec<i16> {
    let (mut hist1, mut hist2) = (0i32, 0i32);
    let mut out = Vec::new();
    for block in blocks {
        let header = block[15] ^ 0x80;
        let scale = 12 - (header & 0xF) as i32;
        let [coef1, coef2] = PROCYON[((header >> 4) as usize).min(4)];
        for i in 0..30 {
            let byte = block[i / 2] ^ 0x80;
            let nibble = if i & 1 == 1 { byte >> 4 } else { byte & 0xF };
            let signed = (((nibble as i32) << 28) >> 28) * 64 * 64;
            let scaled = if scale < 0 { signed << -scale } else { signed >> scale };
            let sample = (hist1 * coef1 + hist2 * coef2 + 32) / 64 + scaled * 64;
            hist2 = hist1;
            hist1 = sample;
            out.push(clamp16((sample + 32) / 64));
        }
    }
    out
}

pub fn decode(file: &[u8]) -> Result<Sound> {
    if file.get(0..4) != Some(b"sadl") {
        bail!("pas un fichier sadl");
    }
    let (Some(&channels), Some(&flags)) = (file.get(0x32), file.get(0x33)) else { bail!("en-tete sadl trop court") };
    let channels = channels as usize;
    if channels == 0 || channels > 8 {
        bail!("sadl : {} voies", channels);
    }
    let rate = match flags & 6 {
        4 => 32728,
        2 => 16364,
        _ => bail!("sadl : frequence inconnue ({:#x})", flags),
    };
    let data = file.get(DATA_AT..).unwrap_or(&[]);
    let blocks = data.len() / BLOCK / channels;
    let voice = |c: usize| (0..blocks).map(move |b| (b * channels + c) * BLOCK);
    let voices: Vec<Vec<i16>> = match flags & 0xF0 {
        0x70 => (0..channels).map(|c| ima(voice(c).flat_map(|at| data[at..at + BLOCK].iter().copied()))).collect(),
        0xB0 => (0..channels).map(|c| procyon(voice(c).map(|at| data[at..at + BLOCK].try_into().unwrap_or([0; BLOCK])))).collect(),
        _ => bail!("sadl : codage inconnu ({:#x})", flags),
    };
    let length = voices.iter().map(Vec::len).min().unwrap_or(0);
    let mut samples = Vec::with_capacity(length * channels);
    for i in 0..length {
        samples.extend(voices.iter().map(|v| v[i]));
    }
    Ok(Sound { rate, channels: channels as u16, samples })
}

impl Sound {
    pub fn seconds(&self) -> f32 {
        self.samples.len() as f32 / self.channels.max(1) as f32 / self.rate.max(1) as f32
    }

    /// Le son en fichier WAV (pour le jouer).
    pub fn wav(&self) -> Vec<u8> {
        let bytes = self.samples.len() as u32 * 2;
        let channels = self.channels as u32;
        let mut out = Vec::with_capacity(44 + bytes as usize);
        out.extend(b"RIFF");
        out.extend((36 + bytes).to_le_bytes());
        out.extend(b"WAVEfmt ");
        out.extend(16u32.to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(self.channels.to_le_bytes());
        out.extend(self.rate.to_le_bytes());
        out.extend((self.rate * channels * 2).to_le_bytes());
        out.extend(((channels * 2) as u16).to_le_bytes());
        out.extend(16u16.to_le_bytes());
        out.extend(b"data");
        out.extend(bytes.to_le_bytes());
        for sample in &self.samples {
            out.extend(sample.to_le_bytes());
        }
        out
    }

    /// La forme d'onde, pour voir le son.
    pub fn waveform(&self) -> crate::Rgba {
        const WIDTH: usize = 1024;
        const HEIGHT: usize = 256;
        let mut data = vec![0u8; WIDTH * HEIGHT * 4];
        let per_column = (self.samples.len() / WIDTH).max(1);
        for (x, column) in self.samples.chunks(per_column).take(WIDTH).enumerate() {
            let (low, high) = column.iter().fold((0i32, 0i32), |(l, h), s| (l.min(*s as i32), h.max(*s as i32)));
            let to_y = |v: i32| ((32768 - v) as usize * (HEIGHT - 1)) / 65535;
            for y in to_y(high)..=to_y(low) {
                data[(y * WIDTH + x) * 4..(y * WIDTH + x) * 4 + 4].copy_from_slice(&[110, 190, 255, 255]);
            }
        }
        crate::Rgba { width: WIDTH as u32, height: HEIGHT as u32, data }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(flags: u8, channels: u8, data: &[u8]) -> Vec<u8> {
        let mut f = vec![0u8; DATA_AT];
        f[0..4].copy_from_slice(b"sadl");
        f[0x32] = channels;
        f[0x33] = flags;
        f.extend(data);
        f
    }

    #[test]
    fn decodes_ima_with_low_nibble_first() {
        // Quartet 7 : monte ; 0xF : redescend
        let sound = decode(&file(0x72, 1, &[0xF7; 16])).unwrap();
        assert_eq!((sound.rate, sound.channels, sound.samples.len()), (16364, 1, 32));
        assert_eq!(sound.samples[0], 11);
        assert!(sound.samples[1] < sound.samples[0]);
    }

    #[test]
    fn interleaves_voices_in_blocks_of_16() {
        let mut data = vec![0x77u8; 16];
        data.extend([0xFFu8; 16]);
        let sound = decode(&file(0x74, 2, &data)).unwrap();
        assert_eq!((sound.rate, sound.channels, sound.samples.len()), (32728, 2, 64));
        assert!(sound.samples[0] > 0 && sound.samples[1] < 0, "voie 0 monte, voie 1 descend");
    }

    #[test]
    fn decodes_procyon_silence() {
        // 0x80 = zéro une fois le xor fait, filtre 0
        let sound = decode(&file(0xB2, 1, &[0x80; 16])).unwrap();
        assert_eq!(sound.samples, vec![0i16; 30]);
    }

    #[test]
    fn refuses_what_it_does_not_know() {
        assert!(decode(b"pas ca").is_err());
        assert!(decode(&file(0x10, 1, &[])).is_err());
        assert!(decode(&file(0x70, 1, &[])).is_err());
    }

    #[test]
    fn writes_a_wav() {
        let wav = Sound { rate: 16364, channels: 1, samples: vec![1, 2] }.wav();
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(wav.len(), 48);
        assert_eq!(&wav[44..], &[1, 0, 2, 0]);
    }
}
