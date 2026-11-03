//! Voix CRI AHX (The Simpsons Game) : même en-tête gros-boutiste qu'ADX, encodage 0x10 ou 0x11, puis des trames MPEG-2
//! Layer II (mono, en-tête fixe FF F5 E0 C0) coupées dès la fin de leurs données et terminées par `AHXE(c)CRI`. Le débit
//! de l'en-tête MPEG n'est qu'un format de conteneur : la fréquence vraie est celle de l'en-tête AHX.

use symphonia_bundle_mp3::MpaDecoder;
use symphonia_core::audio::SampleBuffer;
use symphonia_core::codecs::{CodecParameters, Decoder, DecoderOptions, CODEC_TYPE_MP2};
use symphonia_core::formats::Packet;

use crate::errors::Result;
use crate::sadl::Sound;

const SYNC: [u8; 4] = [0xFF, 0xF5, 0xE0, 0xC0];
const END: &[u8] = b"AHXE";
/// Trame complète pour cet en-tête : 144 * 160 kbit/s / 22 050 Hz.
const FRAME: usize = 1044;
const SAMPLES: usize = 1152;

fn be_at(file: &[u8], at: usize, count: usize) -> Option<usize> {
    file.get(at..at + count).map(|s| s.iter().fold(0, |v, b| v << 8 | *b as usize))
}

pub fn is_ahx(file: &[u8]) -> bool {
    file.get(0..2) == Some(&[0x80, 0]) && matches!(file.get(4), Some(0x10 | 0x11))
}

/// Les trames, de la première synchronisation à la marque de fin ou à la fin du fichier.
fn frames(file: &[u8], start: usize) -> Vec<&[u8]> {
    let end = file.windows(4).rposition(|w| w == END).filter(|at| *at >= start).unwrap_or(file.len());
    let body = file.get(start..end).unwrap_or(&[]);
    let mut starts: Vec<usize> = Vec::new();
    let mut at = 0;
    while at + 4 <= body.len() {
        if body[at..at + 4] == SYNC {
            starts.push(at);
            at += 4;
        } else {
            at += 1;
        }
    }
    starts.iter().enumerate().map(|(i, &from)| &body[from..starts.get(i + 1).copied().unwrap_or(body.len())]).collect()
}

pub fn decode(file: &[u8]) -> Result<Sound> {
    if !is_ahx(file) {
        bail!("pas un fichier AHX");
    }
    let (Some(copyright), Some(rate), Some(total)) = (be_at(file, 2, 2), be_at(file, 8, 4), be_at(file, 12, 4)) else { bail!("en-tete AHX trop court") };
    if rate == 0 || file.get(7) != Some(&1) {
        bail!("AHX : format inattendu ({} Hz)", rate);
    }
    let mut parameters = CodecParameters::new();
    parameters.for_codec(CODEC_TYPE_MP2);
    let Ok(mut decoder) = MpaDecoder::try_new(&parameters, &DecoderOptions::default()) else { bail!("AHX : decodeur indisponible") };
    let list = frames(file, copyright + 4);
    if list.is_empty() {
        bail!("AHX : aucune trame");
    }
    let mut samples: Vec<i16> = Vec::with_capacity(list.len() * SAMPLES);
    for frame in list {
        let mut full = frame.to_vec();
        full.resize(full.len().max(FRAME), 0);
        let packet = Packet::new_from_boxed_slice(0, 0, 0, full.into_boxed_slice());
        let Ok(buffer) = decoder.decode(&packet) else { bail!("AHX : trame illisible") };
        let mut out = SampleBuffer::<i16>::new(buffer.capacity() as u64, *buffer.spec());
        out.copy_interleaved_ref(buffer);
        samples.extend_from_slice(out.samples());
    }
    if total > 0 && samples.len() > total {
        samples.truncate(total);
    }
    Ok(Sound { rate: rate as u32, channels: 1, samples })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ahx_is_not_adx() {
        let mut head = vec![0x80, 0, 0, 0x20, 0x11, 0, 0, 1];
        assert!(is_ahx(&head));
        head[4] = 3;
        assert!(!is_ahx(&head));
    }

    #[test]
    fn splits_frames_on_the_sync_and_stops_at_the_end_mark() {
        let mut file = vec![9; 4];
        file.extend(SYNC);
        file.extend([1, 2]);
        file.extend(SYNC);
        file.extend([3]);
        file.extend(END);
        file.extend([0; 8]);
        let list = frames(&file, 4);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].len(), 6);
        assert_eq!(list[1].len(), 5);
    }
}
