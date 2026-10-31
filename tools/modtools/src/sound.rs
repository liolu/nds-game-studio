//! Le son : du PCM vers les formats du jeu (STRM, SWAV, SWAR, sadl, ADX), avec la fréquence et le nombre de voies de
//! l'original. Lecture des WAV sans outil ; MP3 et le reste passent par FFmpeg (`media`).

use crate::container::{rd16, rd32, wr16, wr32};

const IMA_STEPS: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871,
    5358, 5894, 6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];
const IMA_INDEX: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];

/// Un codeur IMA ADPCM, l'inverse de `sadl::ima` : le quartet faible de chaque octet d'abord.
struct Ima {
    sample: i32,
    index: i32,
}

impl Ima {
    fn nibble(&mut self, target: i16) -> u8 {
        let step = IMA_STEPS[self.index as usize];
        let difference = target as i32 - self.sample;
        let mut nibble = if difference < 0 { 8u8 } else { 0 };
        let mut rest = difference.abs();
        let mut delta = step >> 3;
        for (bit, size) in [(4u8, step), (2, step >> 1), (1, step >> 2)] {
            if rest >= size {
                nibble |= bit;
                rest -= size;
                delta += size;
            }
        }
        self.sample = (if nibble & 8 != 0 { self.sample - delta } else { self.sample + delta }).clamp(-32768, 32767);
        self.index = (self.index + IMA_INDEX[(nibble & 7) as usize]).clamp(0, 88);
        nibble
    }

    fn bytes(&mut self, samples: &[i16]) -> Vec<u8> {
        samples.chunks(2).map(|pair| {
            let low = self.nibble(pair[0]);
            let high = pair.get(1).map_or(0, |s| self.nibble(*s));
            low | (high << 4)
        }).collect()
    }
}

/// Un bloc en IMA : 4 octets d'état, puis les quartets. L'état continue d'un bloc à l'autre, comme à l'encodage d'origine.
fn ima_block(samples: &[i16], coder: &mut Ima) -> Vec<u8> {
    let mut out = (coder.sample as i16).to_le_bytes().to_vec();
    out.extend([coder.index as u8, 0]);
    out.extend(coder.bytes(samples));
    out
}

const PCM8: u8 = 0;
const PCM16: u8 = 1;
const IMA: u8 = 2;

fn encode(kind: u8, samples: &[i16]) -> Result<Vec<u8>, String> {
    match kind {
        PCM8 => Ok(samples.iter().map(|s| (*s >> 8) as u8).collect()),
        PCM16 => Ok(samples.iter().flat_map(|s| s.to_le_bytes()).collect()),
        IMA => {
            let mut coder = Ima { sample: samples.first().copied().unwrap_or(0) as i32, index: 0 };
            let mut bytes = ima_block(samples, &mut coder);
            bytes.resize(bytes.len().next_multiple_of(4), 0);
            Ok(bytes)
        }
        other => Err(format!("codage de son {other} inconnu")),
    }
}

/// Le son entrelacé `samples` dans le STRM `file`, qui garde son codage, sa fréquence et ses voies.
pub fn write_strm(file: &[u8], samples: &[i16]) -> Result<Vec<u8>, String> {
    const HEAD: usize = 0x18;
    let (kind, channels) = (file[HEAD], file[HEAD + 2] as usize);
    let (data_at, old_blocks, block_size, block_samples) = (rd32(file, HEAD + 0x10), rd32(file, HEAD + 0x14), rd32(file, HEAD + 0x18), rd32(file, HEAD + 0x1C));
    if block_samples == 0 || channels == 0 {
        return Err("STRM : en-tete inattendu".into());
    }
    // Certains fichiers comptent aussi l'échantillon de l'en-tête du bloc : on suit ce qui tient vraiment dans un bloc
    let block_samples = if kind == IMA { block_samples.min(block_size.saturating_sub(4) * 2).max(1) } else { block_samples };
    let length = samples.len() / channels;
    let old_length = rd32(file, HEAD + 0x0C);
    let voices: Vec<Vec<i16>> = (0..channels).map(|c| samples.iter().skip(c).step_by(channels).copied().collect()).collect();
    let blocks = length.div_ceil(block_samples).max(1);
    let mut out = file[..data_at].to_vec();
    let mut last_size = 0;
    let mut coders: Vec<Ima> = voices.iter().map(|v| Ima { sample: v.first().copied().unwrap_or(0) as i32, index: 0 }).collect();
    for b in 0..blocks {
        let range = b * block_samples..((b + 1) * block_samples).min(length);
        for (voice, coder) in voices.iter().zip(&mut coders) {
            let part = voice.get(range.clone()).unwrap_or(&[]);
            let bytes = if kind == IMA { ima_block(part, coder) } else { encode(kind, part)? };
            last_size = bytes.len();
            let target = if b + 1 == blocks { bytes.len() } else { block_size };
            let at = out.len();
            out.extend(&bytes);
            out.resize(at + target.max(bytes.len()), 0);
            out.resize(out.len().next_multiple_of(4), 0);
        }
    }
    let last_samples = length - (blocks - 1) * block_samples;
    wr32(&mut out, HEAD + 0x0C, length);
    wr32(&mut out, HEAD + 0x14, blocks);
    wr32(&mut out, HEAD + 0x20, last_size);
    wr32(&mut out, HEAD + 0x24, last_samples);
    // Une boucle n'a plus de sens si la durée change : le son reprend au début
    if length != old_length || old_blocks != blocks {
        wr32(&mut out, HEAD + 8, 0);
    }
    let total = out.len();
    wr32(&mut out, data_at - 4, total - (data_at - 8));
    wr32(&mut out, 8, total);
    Ok(out)
}

/// Un son de banque à la position `at` : son en-tête de 12 octets et ses données, recodés.
fn write_wave(old: &[u8], samples: &[i16]) -> Result<Vec<u8>, String> {
    let kind = old[0];
    let bytes = encode(kind, samples)?;
    let words = bytes.len().div_ceil(4);
    let mut out = old[..12].to_vec();
    let loop_at = rd16(old, 6).min(words);
    wr16(&mut out, 6, if old[1] != 0 { loop_at } else { 0 });
    wr32(&mut out, 8, words - if old[1] != 0 { loop_at } else { 0 });
    out.extend(bytes);
    out.resize(12 + words * 4, 0);
    Ok(out)
}

pub fn write_swav(file: &[u8], samples: &[i16]) -> Result<Vec<u8>, String> {
    let wave = write_wave(file.get(0x18..).ok_or("SWAV trop court")?, samples)?;
    let mut out = file[..0x18].to_vec();
    out.extend(wave);
    let total = out.len();
    wr32(&mut out, 0x14, total - 0x10);
    wr32(&mut out, 8, total);
    Ok(out)
}

/// Une banque `SWAR` où le son `index` est remplacé ; les autres ne bougent pas, seule leur position change.
pub fn write_swar(file: &[u8], index: usize, samples: &[i16]) -> Result<Vec<u8>, String> {
    const COUNT_AT: usize = 0x38;
    const TABLE_AT: usize = 0x3C;
    let count = rd32(file, COUNT_AT);
    if index >= count {
        return Err(format!("SWAR : pas de son {index}"));
    }
    let table_end = TABLE_AT + count * 4;
    let mut out = file[..table_end].to_vec();
    let starts: Vec<usize> = (0..count).map(|i| rd32(file, TABLE_AT + i * 4)).collect();
    if let Some(first) = starts.iter().min() {
        out.extend(file.get(table_end..*first).unwrap_or(&[]));
    }
    for i in 0..count {
        let end = starts.iter().filter(|s| **s > starts[i]).min().copied().unwrap_or(file.len());
        let old = file.get(starts[i]..end).ok_or("SWAR : son hors du fichier")?;
        let at = out.len();
        wr32(&mut out, TABLE_AT + i * 4, at);
        if i == index {
            out.extend(write_wave(old, samples)?);
        } else {
            out.extend(old);
        }
    }
    let total = out.len();
    wr32(&mut out, 0x14, total - 0x10);
    wr32(&mut out, 8, total);
    Ok(out)
}

/// La durée d'origine : la table des tailles d'un sadl ou d'un ADX n'est pas connue, alors le fichier garde sa taille.
fn fit(samples: &[i16], length: usize) -> Vec<i16> {
    let mut out = samples.to_vec();
    out.resize(length, 0);
    out
}

const PROCYON: [[i32; 2]; 5] = [[0, 0], [60, 0], [115, -52], [98, -55], [122, -60]];

/// Le meilleur bloc Procyon : essaie chaque filtre et chaque échelle, garde la plus petite erreur.
fn procyon_block(samples: &[i16], history: &mut (i32, i32)) -> [u8; 16] {
    let mut best = (i64::MAX, [0u8; 16], *history);
    for filter in 0..PROCYON.len() {
        let [coef1, coef2] = PROCYON[filter];
        for shift in 0..=12i32 {
            let scale = 12 - shift;
            let (mut hist1, mut hist2) = *history;
            let mut block = [0u8; 16];
            let mut error = 0i64;
            for (i, sample) in samples.iter().enumerate() {
                let predicted = (hist1 * coef1 + hist2 * coef2 + 32) / 64;
                let unit = 64 * 64 * 64 >> scale;
                let wanted = (*sample as i32) * 64 - predicted;
                let nibble = ((wanted as f32 / unit.max(1) as f32).round() as i32).clamp(-8, 7);
                let value = predicted + (nibble * 64 * 64 >> scale) * 64;
                hist2 = hist1;
                hist1 = value;
                let difference = ((value + 32) / 64).clamp(-32768, 32767) as i64 - *sample as i64;
                error += difference * difference;
                let n = (nibble & 15) as u8;
                block[i / 2] |= if i & 1 == 1 { n << 4 } else { n };
            }
            if error < best.0 {
                for byte in &mut block[..15] {
                    *byte ^= 0x80;
                }
                block[15] = (((filter as u8) << 4) | shift as u8) ^ 0x80;
                best = (error, block, (hist1, hist2));
            }
        }
    }
    *history = best.2;
    best.1
}

/// Un `sadl` avec ce son, à la même taille : plus court, il finit par du silence ; plus long, il est coupé.
pub fn write_sadl(file: &[u8], samples: &[i16]) -> Result<Vec<u8>, String> {
    const DATA_AT: usize = 0x100;
    const BLOCK: usize = 16;
    const SAMPLES: usize = 30;
    let (channels, flags) = (file[0x32] as usize, file[0x33]);
    let blocks = file.len().saturating_sub(DATA_AT) / BLOCK / channels.max(1);
    let mut out = file.to_vec();
    for c in 0..channels {
        let voice: Vec<i16> = samples.iter().skip(c).step_by(channels).copied().collect();
        match flags & 0xF0 {
            0x70 => {
                let mut coder = Ima { sample: 0, index: 0 };
                let bytes = coder.bytes(&fit(&voice, blocks * BLOCK * 2));
                for (b, chunk) in bytes.chunks_exact(BLOCK).enumerate() {
                    let at = DATA_AT + (b * channels + c) * BLOCK;
                    out[at..at + BLOCK].copy_from_slice(chunk);
                }
            }
            0xB0 => {
                let voice = fit(&voice, blocks * SAMPLES);
                let mut history = (0, 0);
                for (b, chunk) in voice.chunks_exact(SAMPLES).enumerate() {
                    let at = DATA_AT + (b * channels + c) * BLOCK;
                    out[at..at + BLOCK].copy_from_slice(&procyon_block(chunk, &mut history));
                }
            }
            other => return Err(format!("sadl : codage {other:#x} inconnu")),
        }
    }
    Ok(out)
}

/// Un `ADX` avec ce son, à la même taille et avec le même filtre que l'original.
pub fn write_adx(file: &[u8], samples: &[i16]) -> Result<Vec<u8>, String> {
    const BLOCK: usize = 18;
    const SAMPLES: usize = 32;
    let be16 = |at: usize| u16::from_be_bytes([file[at], file[at + 1]]) as usize;
    let be32 = |at: usize| u32::from_be_bytes([file[at], file[at + 1], file[at + 2], file[at + 3]]) as usize;
    let (start, channels, rate, cutoff) = (be16(2) + 4, file[7] as usize, be32(8), be16(16));
    let z = (2.0 * std::f64::consts::PI * cutoff as f64 / rate as f64).cos();
    let a = std::f64::consts::SQRT_2 - z;
    let b = std::f64::consts::SQRT_2 - 1.0;
    let c = (a - ((a + b) * (a - b)).max(0.0).sqrt()) / b;
    let (coef1, coef2) = ((c * 8192.0).floor() as i32, (c * c * -4096.0).floor() as i32);
    let mut out = file.to_vec();
    let frames = (file.len() - start) / (BLOCK * channels);
    for channel in 0..channels {
        let voice: Vec<i16> = samples.iter().skip(channel).step_by(channels).copied().collect();
        let voice = fit(&voice, frames * SAMPLES);
        let (mut hist1, mut hist2) = (0i32, 0i32);
        for frame in 0..frames {
            let at = start + (frame * channels + channel) * BLOCK;
            // Le bloc de fin garde sa marque
            if file[at] & 0x80 != 0 {
                break;
            }
            let chunk = &voice[frame * SAMPLES..(frame + 1) * SAMPLES];
            let mut best = (i64::MAX, [0u8; BLOCK], (hist1, hist2));
            for scale in [1, 2, 3, 4, 6, 8, 12, 16, 24, 32, 48, 64, 96, 128, 192, 256, 384, 512, 768, 1024, 1536, 2048, 3072, 4096, 6144, 8192] {
                let (mut h1, mut h2) = (hist1, hist2);
                let mut block = [0u8; BLOCK];
                block[..2].copy_from_slice(&((scale - 1) as u16).to_be_bytes());
                let mut error = 0i64;
                for (i, sample) in chunk.iter().enumerate() {
                    let predicted = (coef1 * h1 + coef2 * h2) >> 12;
                    let nibble = (((*sample as i32 - predicted) as f32 / scale as f32).round() as i32).clamp(-8, 7);
                    let value = (nibble * scale + predicted).clamp(-32768, 32767);
                    h2 = h1;
                    h1 = value;
                    let difference = (value - *sample as i32) as i64;
                    error += difference * difference;
                    let n = (nibble & 15) as u8;
                    block[2 + i / 2] |= if i & 1 == 0 { n << 4 } else { n };
                }
                if error < best.0 {
                    best = (error, block, (h1, h2));
                }
            }
            out[at..at + BLOCK].copy_from_slice(&best.1);
            (hist1, hist2) = best.2;
        }
    }
    Ok(out)
}

/// Le PCM d'un WAV : (fréquence, voies, échantillons entrelacés). `None` si ce n'est pas du PCM 8, 16, 24 ou 32 bits flottant.
pub fn read_wav(file: &[u8]) -> Option<(u32, u16, Vec<i16>)> {
    if file.get(0..4)? != b"RIFF" || file.get(8..12)? != b"WAVE" {
        return None;
    }
    let (mut at, mut format) = (12, None);
    while at + 8 <= file.len() {
        let size = rd32(file, at + 4);
        match &file[at..at + 4] {
            b"fmt " => format = Some((rd16(file, at + 8), rd16(file, at + 10) as u16, rd32(file, at + 12) as u32, rd16(file, at + 22))),
            b"data" => {
                let (tag, channels, rate, bits) = format?;
                let data = &file[at + 8..(at + 8 + size).min(file.len())];
                let samples: Vec<i16> = match (tag, bits) {
                    (1, 8) => data.iter().map(|b| ((*b as i16) - 128) << 8).collect(),
                    (1 | 0xFFFE, 16) => data.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect(),
                    (1 | 0xFFFE, 24) => data.chunks_exact(3).map(|c| i16::from_le_bytes([c[1], c[2]])).collect(),
                    (3, 32) => data.chunks_exact(4).map(|c| (f32::from_le_bytes([c[0], c[1], c[2], c[3]]).clamp(-1.0, 1.0) * 32767.0) as i16).collect(),
                    _ => return None,
                };
                return Some((rate, channels, samples));
            }
            _ => {}
        }
        at += 8 + size + (size & 1);
    }
    None
}

/// Change la fréquence et le nombre de voies, simplement (interpolation). FFmpeg fait mieux s'il est là.
pub fn convert(samples: &[i16], rate: u32, channels: u16, to_rate: u32, to_channels: u16) -> Vec<i16> {
    let (channels, to_channels) = (channels.max(1) as usize, to_channels.max(1) as usize);
    let frames = samples.len() / channels;
    let out_frames = (frames as u64 * to_rate as u64 / rate.max(1) as u64) as usize;
    let mut out = Vec::with_capacity(out_frames * to_channels);
    let at = |frame: usize, channel: usize| samples.get(frame * channels + channel.min(channels - 1)).copied().unwrap_or(0) as f32;
    for i in 0..out_frames {
        let position = i as f64 * rate as f64 / to_rate.max(1) as f64;
        let (first, part) = (position as usize, position.fract() as f32);
        for channel in 0..to_channels {
            let value = |frame: usize| if to_channels == 1 { (0..channels).map(|c| at(frame, c)).sum::<f32>() / channels as f32 } else { at(frame, channel) };
            out.push((value(first) * (1.0 - part) + value(first + 1) * part) as i16);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use nds_nitro::{adx, sadl, sdat};

    fn tone(count: usize) -> Vec<i16> {
        (0..count).map(|i| ((i as f32 * 0.05).sin() * 12000.0) as i16).collect()
    }

    fn close(a: &[i16], b: &[i16], limit: f64) {
        assert_eq!(a.len(), b.len());
        let error: f64 = a.iter().zip(b).map(|(x, y)| (*x as f64 - *y as f64).powi(2)).sum::<f64>() / a.len() as f64;
        assert!(error.sqrt() < limit, "erreur {}", error.sqrt());
    }

    fn strm(kind: u8, channels: u8, block_size: usize, block_samples: usize) -> Vec<u8> {
        let mut file = vec![0u8; 0x68 + 8];
        file[0..4].copy_from_slice(b"STRM");
        file[0x18] = kind;
        file[0x1A] = channels;
        wr16(&mut file, 0x1C, 16000);
        wr32(&mut file, 0x28, 0x68 + 8);
        wr32(&mut file, 0x2C, 1);
        wr32(&mut file, 0x30, block_size);
        wr32(&mut file, 0x34, block_samples);
        wr32(&mut file, 0x38, 0);
        wr32(&mut file, 0x3C, 0);
        file[0x68..0x6C].copy_from_slice(b"DATA");
        file
    }

    #[test]
    fn stream_kinds_read_back_close_to_the_input() {
        for (kind, limit) in [(PCM8, 200.0), (PCM16, 1.0), (IMA, 400.0)] {
            let samples = tone(4000);
            let block_samples = 1000;
            let block_size = match kind {
                PCM8 => 1000,
                PCM16 => 2000,
                _ => 504,
            };
            let out = write_strm(&strm(kind, 2, block_size, block_samples), &samples).unwrap();
            let sound = sdat::read_strm(&out).unwrap();
            assert_eq!((sound.rate, sound.channels), (16000, 2));
            close(&sound.samples[..samples.len()], &samples, limit);
        }
    }

    fn wave(kind: u8, loop_flag: u8, data: &[u8]) -> Vec<u8> {
        let mut out = vec![kind, loop_flag, 0x10, 0x27, 0, 0, 0, 0];
        out.extend(((data.len() / 4) as u32).to_le_bytes());
        out.extend(data);
        out
    }

    #[test]
    fn single_sounds_can_grow() {
        let mut file = vec![0u8; 0x18];
        file[0..4].copy_from_slice(b"SWAV");
        file.extend(wave(PCM16, 0, &[0, 0, 0, 0]));
        let samples = tone(100);
        let out = write_swav(&file, &samples).unwrap();
        let sound = sdat::read_swav(&out).unwrap();
        close(&sound.samples, &samples, 1.0);
        assert_eq!(rd32(&out, 8), out.len());
    }

    #[test]
    fn a_bank_changes_one_sound_and_keeps_the_others() {
        let mut file = vec![0u8; 0x38];
        file[0..4].copy_from_slice(b"SWAR");
        file.extend(2u32.to_le_bytes());
        let (a, b) = (wave(PCM8, 0, &[1, 2, 3, 4]), wave(PCM8, 0, &[5, 6, 7, 8]));
        let start = 0x3C + 8;
        file.extend((start as u32).to_le_bytes());
        file.extend(((start + a.len()) as u32).to_le_bytes());
        file.extend(&a);
        file.extend(&b);
        let samples: Vec<i16> = vec![256; 16];
        let out = write_swar(&file, 0, &samples).unwrap();
        assert_eq!(sdat::swar_count(&out).unwrap(), 2);
        assert_eq!(sdat::read_swar(&out, 0).unwrap().samples, samples);
        assert_eq!(sdat::read_swar(&out, 1).unwrap().samples, [5 << 8, 6 << 8, 7 << 8, 8 << 8]);
    }

    fn sadl_file(flags: u8, blocks: usize) -> Vec<u8> {
        let mut file = vec![0u8; 0x100 + blocks * 16];
        file[0..4].copy_from_slice(b"sadl");
        file[0x32] = 1;
        file[0x33] = flags | 4;
        file
    }

    #[test]
    fn sadl_keeps_its_size_with_both_codecs() {
        for (flags, per_block, limit) in [(0x70, 32, 500.0), (0xB0, 30, 900.0)] {
            let samples = tone(per_block * 40);
            let file = sadl_file(flags, 40);
            let out = write_sadl(&file, &samples).unwrap();
            assert_eq!(out.len(), file.len());
            let sound = sadl::decode(&out).unwrap();
            close(&sound.samples, &samples, limit);
        }
    }

    #[test]
    fn adx_keeps_its_size() {
        let mut file = vec![0x80, 0, 0, 0x24, 3, 18, 4, 1, 0, 0, 0x7F, 0xD8, 0, 0, 0x02, 0x80, 1, 0xF4, 4, 0];
        file.resize(0x28, 0);
        file.extend(vec![0u8; 18 * 20]);
        file.extend([0x80, 1]);
        file.extend([0u8; 16]);
        let samples = tone(640);
        let out = write_adx(&file, &samples).unwrap();
        assert_eq!(out.len(), file.len());
        let sound = adx::decode(&out).unwrap();
        close(&sound.samples, &samples, 500.0);
    }

    #[test]
    fn wav_files_are_read() {
        let sound = sadl::Sound { rate: 22050, channels: 2, samples: vec![1, -1, 300, -300] };
        let (rate, channels, samples) = read_wav(&sound.wav()).unwrap();
        assert_eq!((rate, channels, samples), (22050, 2, vec![1, -1, 300, -300]));
        assert!(read_wav(b"pas un wav").is_none());
    }

    #[test]
    fn conversion_changes_rate_and_channels() {
        let stereo = [100i16, 300, 100, 300, 100, 300, 100, 300];
        let mono = convert(&stereo, 8000, 2, 4000, 1);
        assert_eq!(mono, [200, 200]);
    }
}
