//! Son CRI ADX (Children of Mana...) : en-tête gros-boutiste, puis des blocs de 18 octets par voie : 2 octets
//! d'échelle, 32 échantillons de 4 bits (le quartet fort d'abord).

use crate::errors::Result;
use crate::sadl::Sound;

const BLOCK: usize = 18;
const SAMPLES: usize = 32;

fn be16(b: &[u8], at: usize) -> Option<usize> {
    b.get(at..at + 2).map(|s| u16::from_be_bytes([s[0], s[1]]) as usize)
}

fn be32(b: &[u8], at: usize) -> Option<usize> {
    b.get(at..at + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]) as usize)
}

pub fn is_adx(file: &[u8]) -> bool {
    file.get(0..2) == Some(&[0x80, 0])
}

pub fn decode(file: &[u8]) -> Result<Sound> {
    if !is_adx(file) {
        bail!("pas un fichier ADX");
    }
    let (Some(copyright), Some(&channels), Some(rate), Some(total), Some(cutoff)) = (be16(file, 2), file.get(7), be32(file, 8), be32(file, 12), be16(file, 16)) else { bail!("en-tete ADX trop court") };
    let channels = channels as usize;
    if file.get(4) != Some(&3) || file.get(5) != Some(&(BLOCK as u8)) || channels == 0 || channels > 2 || rate == 0 {
        bail!("ADX : format inattendu ({} voies, {} Hz)", channels, rate);
    }
    let data = file.get(copyright + 4..).unwrap_or(&[]);
    // Le filtre se déduit de la fréquence de coupure (nombres à virgule fixe sur 12 bits)
    let z = (2.0 * std::f64::consts::PI * cutoff as f64 / rate as f64).cos();
    let a = std::f64::consts::SQRT_2 - z;
    let b = std::f64::consts::SQRT_2 - 1.0;
    let c = (a - ((a + b) * (a - b)).max(0.0).sqrt()) / b;
    let (coef1, coef2) = ((c * 8192.0).floor() as i32, (c * c * -4096.0).floor() as i32);

    let mut history = vec![(0i32, 0i32); channels];
    let mut samples = Vec::with_capacity(total.min(data.len() * 2) * channels);
    'blocks: for frame in data.chunks_exact(BLOCK * channels) {
        for (c, block) in frame.chunks_exact(BLOCK).enumerate() {
            let header = u16::from_be_bytes([block[0], block[1]]);
            if header & 0x8000 != 0 {
                break 'blocks;
            }
            let scale = (header & 0x1FFF) as i32 + 1;
            let (mut hist1, mut hist2) = history[c];
            for i in 0..SAMPLES {
                let byte = block[2 + i / 2];
                let nibble = if i & 1 == 0 { byte >> 4 } else { byte & 0xF };
                let signed = ((nibble as i32) << 28) >> 28;
                let sample = (signed * scale + ((coef1 * hist1 + coef2 * hist2) >> 12)).clamp(-32768, 32767);
                hist2 = hist1;
                hist1 = sample;
                samples.push(sample as i16);
            }
            history[c] = (hist1, hist2);
        }
    }
    // Les voies ont été écrites bloc par bloc : on les entrelace échantillon par échantillon
    let mut out = Vec::with_capacity(samples.len());
    for frame in samples.chunks_exact(SAMPLES * channels) {
        for i in 0..SAMPLES {
            for c in 0..channels {
                out.push(frame[c * SAMPLES + i]);
            }
        }
    }
    out.truncate(total.saturating_mul(channels).max(1).min(out.len()));
    Ok(Sound { rate: rate as u32, channels: channels as u16, samples: out })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_blocks_until_the_end_marker() {
        let mut file = vec![0x80, 0, 0, 0x24, 3, 18, 4, 1, 0, 0, 0x7F, 0xD8, 0, 0, 0, 32, 1, 0xF4, 4, 0];
        file.resize(0x28, 0);
        // Échelle 1, premier échantillon 1
        file.extend([0, 0, 0x10]);
        file.extend([0u8; 15]);
        file.extend([0x80, 0x01]);
        file.extend([0u8; 16]);
        let sound = decode(&file).unwrap();
        assert_eq!((sound.rate, sound.channels, sound.samples.len()), (32728, 1, 32));
        assert_eq!(sound.samples[0], 1);
        assert!(decode(b"pas ca").is_err());
    }
}
