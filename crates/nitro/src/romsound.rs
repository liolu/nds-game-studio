//! `SoundData.rom` du moteur Amaze (Lego Star Wars II, Les Sims 2, Spyro Shadow Legacy) : une table de 10 positions
//! vers 9 tables, chacune une liste de positions dans le fichier entier. Table 2 = fiches des sons (36 octets),
//! table 4 = les sons eux-mêmes ; les autres (morceaux, instruments...) ne sont pas lues.
//!
//! Fiche d'un son : u32 n° du son, octet 5 = drapeaux (bit 3 : IMA ADPCM, sinon PCM 8 bits), u32 à 20 = fréquence.
//! Chaque son commence par 4 octets d'état (échantillon, index IMA), à zéro en PCM.

use crate::sadl::Sound;
use crate::sdat;

const TABLES: usize = 9;
const DESCRIPTORS: usize = 2;
const SAMPLES: usize = 4;
const DESCRIPTOR_SIZE: usize = 36;
const HEADER: usize = 4;

/// Un son de la table : sa place dans le fichier, sa fréquence et son codage.
pub struct Sample {
    pub start: usize,
    pub end: usize,
    pub rate: u32,
    pub adpcm: bool,
}

fn u32_at(b: &[u8], at: usize) -> Option<usize> {
    b.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize)
}

/// Les positions de la table `n`, croissantes et dans le fichier ; `None` si ce n'est pas un tel fichier.
fn table(file: &[u8], n: usize) -> Option<Vec<usize>> {
    let (start, end) = (u32_at(file, n * 4)?, u32_at(file, n * 4 + 4)?);
    if start > end || end > file.len() || (end - start) % 4 != 0 {
        return None;
    }
    let list: Vec<usize> = (start..end).step_by(4).filter_map(|at| u32_at(file, at)).collect();
    (list.windows(2).all(|w| w[0] <= w[1]) && list.last().is_none_or(|&last| last <= file.len())).then_some(list)
}

/// Les sons du fichier, dans l'ordre de la table ; vide si ce n'est pas un `SoundData.rom` d'Amaze. Un son sans fiche
/// n'est pas gardé : sans elle, ni sa fréquence ni son codage ne sont connus.
pub fn samples(file: &[u8]) -> Vec<Sample> {
    let read = || -> Option<Vec<Sample>> {
        if u32_at(file, 0)? != (TABLES + 1) * 4 {
            return None;
        }
        let tables: Vec<Vec<usize>> = (0..TABLES).map(|n| table(file, n)).collect::<Option<_>>()?;
        // Les sons vont jusqu'à la table suivante dans le fichier, ou jusqu'à sa fin
        let starts = &tables[SAMPLES];
        let last = starts.last()?;
        let end = tables.iter().flatten().copied().chain((0..=TABLES).filter_map(|n| u32_at(file, n * 4))).filter(|&at| at > *last).min().unwrap_or(file.len());
        let mut out: Vec<Option<Sample>> = (0..starts.len()).map(|_| None).collect();
        for &at in &tables[DESCRIPTORS] {
            let fiche = file.get(at..at + DESCRIPTOR_SIZE)?;
            let (n, rate) = (u32_at(fiche, 0)?, u32_at(fiche, 20)? as u32);
            // Une fiche qui renvoie hors de la table, ou sans fréquence : pas de son à montrer
            let Some(slot) = out.get_mut(n).filter(|_| (1000..=96000).contains(&rate)) else { continue };
            if slot.is_none() {
                let end = starts.get(n + 1).copied().unwrap_or(end);
                *slot = Some(Sample { start: starts[n], end, rate, adpcm: fiche[5] & 8 != 0 });
            }
        }
        Some(out.into_iter().flatten().filter(|s| s.end > s.start + HEADER).collect())
    };
    read().unwrap_or_default()
}

/// Le son, à partir de ses octets (`Sample::start..end`).
pub fn decode(bytes: &[u8], sample: &Sample) -> Sound {
    let samples = if sample.adpcm { sdat::voice(bytes, sdat::IMA) } else { sdat::voice(bytes.get(HEADER..).unwrap_or(&[]), sdat::PCM8) };
    Sound { rate: sample.rate, channels: 1, samples }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_sample_table() {
        // 10 positions, tables vides sauf la 2 (une fiche) et la 4 (un son)
        let mut file = vec![0u8; 40];
        let descriptor_at = 48;
        let sample_at = descriptor_at + DESCRIPTOR_SIZE;
        let ends = [40, 40, 40, 44, 44, 48, 48, 48, 48, 48];
        for (n, at) in ends.iter().enumerate() {
            file[n * 4..n * 4 + 4].copy_from_slice(&(*at as u32).to_le_bytes());
        }
        file.extend((descriptor_at as u32).to_le_bytes());
        file.extend((sample_at as u32).to_le_bytes());
        let mut fiche = [0u8; DESCRIPTOR_SIZE];
        fiche[5] = 1;
        fiche[20..24].copy_from_slice(&22050u32.to_le_bytes());
        file.extend(fiche);
        file.extend([0, 0, 0, 0, 0x7F, 0x80]);
        let found = samples(&file);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].start, found[0].end, found[0].rate, found[0].adpcm), (sample_at, file.len(), 22050, false));
        assert_eq!(decode(&file[found[0].start..found[0].end], &found[0]).samples, [0x7F00, -0x8000]);
        assert!(samples(b"pas un fichier de sons").is_empty());
    }
}
