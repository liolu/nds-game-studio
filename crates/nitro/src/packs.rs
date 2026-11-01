//! Les archives maison qui rangent des vidéos : `farc` (Blue Dragon Plus), `.bundle` et sa table `.toc` (Clone Wars), `ANAG`
//! (Heroes of Mana). Lecture des vidéos qu'elles contiennent, et copie de l'archive où certaines ont changé de taille.
//!
//! Ces règles viennent du code des jeux : une position change pour un multiple de 32 octets, ce qui garde l'alignement de tout ce qui suit.

use std::collections::HashMap;

const SHIFT_STEP: isize = 32;

fn rd32(b: &[u8], at: usize) -> usize {
    b.get(at..at + 4).map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize)
}

fn wr32(b: &mut [u8], at: usize, value: usize) {
    b[at..at + 4].copy_from_slice(&(value as u32).to_le_bytes());
}

fn is_video(bytes: &[u8]) -> bool {
    matches!(bytes.get(..4), Some(b"MODS" | b"VXDS" | [b'B', b'I', b'K', _]))
}

/// Ce que devient un morceau de `old` octets qui reçoit `new` octets : un écart multiple de 32, jamais moins que la place voulue.
fn slot_after(old: usize, new: usize) -> usize {
    let gap = new as isize - old as isize;
    (old as isize + (gap + SHIFT_STEP - 1).div_euclid(SHIFT_STEP) * SHIFT_STEP) as usize
}

// --------------------------------------------------------------------------------------------
//  farc : "farc", nombre, positions depuis la fin de la table ; chaque morceau commence par sa taille
// --------------------------------------------------------------------------------------------

fn farc_table(bytes: &[u8]) -> Option<(usize, Vec<usize>)> {
    if bytes.get(..4)? != b"farc" {
        return None;
    }
    let count = rd32(bytes, 4);
    let base = 8 + count.checked_mul(4)?;
    if count == 0 || base > bytes.len() {
        return None;
    }
    let offsets: Vec<usize> = (0..count).map(|i| rd32(bytes, 8 + i * 4)).collect();
    let good = offsets.windows(2).all(|w| w[0] <= w[1]) && base + offsets[count - 1] <= bytes.len();
    good.then_some((base, offsets))
}

/// Le contenu de chaque morceau d'un `farc`, sans sa taille.
fn farc_members(bytes: &[u8]) -> Option<Vec<(usize, usize)>> {
    let (base, offsets) = farc_table(bytes)?;
    offsets
        .iter()
        .map(|&offset| {
            let start = base + offset + 4;
            let end = start.checked_add(rd32(bytes, base + offset))?;
            (end <= bytes.len()).then_some((start, end))
        })
        .collect()
}

fn rebuild_farc(bytes: &[u8], new: &HashMap<usize, Vec<u8>>) -> Option<Vec<u8>> {
    let (base, offsets) = farc_table(bytes)?;
    let mut out = bytes[..base].to_vec();
    for (i, &offset) in offsets.iter().enumerate() {
        let start = base + offset;
        let end = offsets.get(i + 1).map_or(bytes.len(), |next| base + next);
        let position = out.len() - base;
        wr32(&mut out, 8 + i * 4, position);
        match new.get(&(start + 4)) {
            Some(data) => {
                let at = out.len();
                out.resize(at + slot_after(end - start, data.len() + 4), 0);
                wr32(&mut out, at, data.len());
                out[at + 4..at + 4 + data.len()].copy_from_slice(data);
            }
            None => out.extend_from_slice(&bytes[start..end]),
        }
    }
    Some(out)
}

// --------------------------------------------------------------------------------------------
//  .toc : nombre, puis (position, taille sur 24 bits et numéro de source sur 8)
// --------------------------------------------------------------------------------------------

fn toc_table(toc: &[u8], bundle_size: usize) -> Option<Vec<(usize, usize)>> {
    let count = rd32(toc, 0);
    if count == 0 || 4 + count.checked_mul(8)? > toc.len() {
        return None;
    }
    let pairs: Vec<(usize, usize)> = (0..count).map(|i| (rd32(toc, 4 + i * 8), rd32(toc, 8 + i * 8))).collect();
    let good = pairs.windows(2).all(|w| w[0].0 <= w[1].0) && pairs.iter().all(|p| p.0 + (p.1 & 0xFF_FFFF) <= bundle_size);
    good.then_some(pairs)
}

fn rebuild_bundle(bytes: &[u8], toc: &[u8], new: &HashMap<usize, Vec<u8>>) -> Option<(Vec<u8>, Vec<u8>)> {
    let pairs = toc_table(toc, bytes.len())?;
    let mut new_toc = toc.to_vec();
    let mut out = Vec::with_capacity(bytes.len());
    for (i, &(start, size)) in pairs.iter().enumerate() {
        let end = pairs.get(i + 1).map_or(bytes.len(), |next| next.0);
        wr32(&mut new_toc, 4 + i * 8, out.len());
        match new.get(&start) {
            Some(data) => {
                let at = out.len();
                out.resize(at + slot_after(end - start, data.len()), 0);
                out[at..at + data.len()].copy_from_slice(data);
                wr32(&mut new_toc, 8 + i * 8, (size & 0xFF00_0000) | data.len());
            }
            None => out.extend_from_slice(&bytes[start..end]),
        }
    }
    Some((out, new_toc))
}

// --------------------------------------------------------------------------------------------
//  ANAG : un arbre de tables de positions absolues. Les vidéos sont dans une même table : nombre, puis nombre + 1 positions croissantes.
//  D'autres tables pointent ailleurs dans le fichier, et rien ne les décrit toutes : le reste du fichier ne bouge jamais.
// --------------------------------------------------------------------------------------------

/// Ne regarde que le début du fichier, où sont les tables de premier niveau.
const ANAG_HEAD_WORDS: usize = 0x400;

/// La table des vidéos : sa position, et les positions qu'elle contient.
fn anag_table(bytes: &[u8]) -> Option<(usize, Vec<usize>)> {
    if bytes.get(..4)? != b"ANAG" {
        return None;
    }
    (1..ANAG_HEAD_WORDS).find_map(|word| {
        let at = rd32(bytes, word * 4);
        let count = rd32(bytes, at);
        if at == 0 || count == 0 || count > 0x10000 || at + 4 + (count + 1) * 4 > bytes.len() {
            return None;
        }
        let table: Vec<usize> = (0..=count).map(|i| rd32(bytes, at + 4 + i * 4)).collect();
        // Le premier morceau est une vidéo : là où elle soit, car une rom déjà modifiée a pu déplacer tout le groupe
        let good = table.windows(2).all(|w| w[0] <= w[1]) && table[count] <= bytes.len() && bytes.get(table[0]..).is_some_and(is_video);
        good.then_some((at, table))
    })
}

fn anag_members(bytes: &[u8]) -> Option<Vec<(usize, usize)>> {
    let (_, table) = anag_table(bytes)?;
    Some(table.windows(2).filter(|w| w[0] < w[1]).map(|w| (w[0], w[1])).collect())
}

fn rebuild_anag(bytes: &[u8], new: &HashMap<usize, Vec<u8>>) -> Option<Vec<u8>> {
    let (at, table) = anag_table(bytes)?;
    let (first, last) = (table[0], *table.last()?);
    let mut group = Vec::with_capacity(last - first);
    let mut moved: HashMap<usize, usize> = HashMap::new();
    for pair in table.windows(2).filter(|w| w[0] < w[1]) {
        moved.insert(pair[0], group.len());
        match new.get(&pair[0]) {
            Some(data) => {
                let start = group.len();
                group.resize(start + slot_after(pair[1] - pair[0], data.len()), 0);
                group[start..start + data.len()].copy_from_slice(data);
            }
            None => group.extend_from_slice(&bytes[pair[0]..pair[1]]),
        }
    }
    moved.insert(last, group.len());
    let mut out = bytes.to_vec();
    // Tout le groupe reste à sa place s'il y tient, sinon il part entier à la fin : ses tailles se lisent entre deux positions voisines
    let base = if group.len() <= last - first {
        first
    } else {
        out.resize(out.len().next_multiple_of(SHIFT_STEP as usize), 0);
        out.resize(out.len() + group.len(), 0);
        out.len() - group.len()
    };
    out[base..base + group.len()].copy_from_slice(&group);
    for (i, position) in table.iter().enumerate() {
        wr32(&mut out, at + 4 + i * 4, if *position == last && base == first { last } else { base + moved[position] });
    }
    Some(out)
}

// --------------------------------------------------------------------------------------------
//  Ce que voient l'outil de mod et le scan
// --------------------------------------------------------------------------------------------

/// Le chemin de la table d'une archive qui en a une à côté d'elle.
pub fn companion(path: &str) -> Option<String> {
    let lower = path.to_lowercase();
    lower.ends_with(".bundle").then(|| format!("{}.toc", &path[..path.len() - ".bundle".len()]))
}

/// Les vidéos rangées dans cette archive (position de début, de fin).
pub fn videos(bytes: &[u8], table: Option<&[u8]>) -> Vec<(usize, usize)> {
    let members = match bytes.get(..4) {
        Some(b"farc") => farc_members(bytes),
        Some(b"ANAG") => anag_members(bytes),
        _ => table.and_then(|toc| toc_table(toc, bytes.len())).map(|pairs| pairs.iter().map(|p| (p.0, p.0 + (p.1 & 0xFF_FFFF))).collect()),
    };
    members.unwrap_or_default().into_iter().filter(|m| m.0 < m.1 && is_video(&bytes[m.0..m.1])).collect()
}

/// La copie de l'archive où ces morceaux (par position de début) ont changé, et sa nouvelle table s'il y en a une.
pub fn rebuild(bytes: &[u8], table: Option<&[u8]>, new: &HashMap<usize, Vec<u8>>) -> Option<(Vec<u8>, Option<Vec<u8>>)> {
    match bytes.get(..4) {
        Some(b"farc") => rebuild_farc(bytes, new).map(|out| (out, None)),
        Some(b"ANAG") => rebuild_anag(bytes, new).map(|out| (out, None)),
        _ => rebuild_bundle(bytes, table?, new).map(|(out, toc)| (out, Some(toc))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(tag: u8, size: usize) -> Vec<u8> {
        let mut out = b"MODS".to_vec();
        out.resize(size, tag);
        out
    }

    fn farc(members: &[Vec<u8>]) -> Vec<u8> {
        let mut out = b"farc".to_vec();
        out.extend_from_slice(&(members.len() as u32).to_le_bytes());
        let mut at = 0u32;
        for member in members {
            out.extend_from_slice(&at.to_le_bytes());
            at += (member.len() as u32 + 4).next_multiple_of(4);
        }
        for member in members {
            out.extend_from_slice(&(member.len() as u32).to_le_bytes());
            out.extend_from_slice(member);
            out.resize(out.len().next_multiple_of(4), 0);
        }
        out
    }

    #[test]
    fn farc_members_keep_their_neighbours_when_one_grows_or_shrinks() {
        let (first, second, third) = (vec![9u8; 40], video(1, 100), video(2, 60));
        let file = farc(&[first.clone(), second, third.clone()]);
        let found = videos(&file, None);
        assert_eq!(found.len(), 2);
        for size in [1000, 20] {
            let out = rebuild(&file, None, &HashMap::from([(found[0].0, video(7, size))])).unwrap().0;
            let members = farc_members(&out).unwrap();
            assert_eq!(&out[members[0].0..members[0].1], &first);
            assert_eq!(members[1].1 - members[1].0, size);
            assert_eq!(&out[members[2].0..members[2].1], &third);
        }
    }

    #[test]
    fn a_bundle_moves_what_follows_and_keeps_its_alignment() {
        let mut bundle = video(1, 64);
        bundle.extend(vec![3u8; 32]);
        let mut toc = 2u32.to_le_bytes().to_vec();
        for (at, size) in [(0u32, 64u32), (64, 30 | 5 << 24)] {
            toc.extend_from_slice(&at.to_le_bytes());
            toc.extend_from_slice(&size.to_le_bytes());
        }
        assert_eq!(videos(&bundle, Some(&toc)), vec![(0, 64)]);
        let (out, new_toc) = rebuild(&bundle, Some(&toc), &HashMap::from([(0, video(7, 70))])).unwrap();
        let new_toc = new_toc.unwrap();
        assert_eq!(rd32(&new_toc, 8), 70);
        assert_eq!(rd32(&new_toc, 12) % 32, 0);
        assert_eq!(rd32(&new_toc, 16), 30 | 5 << 24);
        assert_eq!(out[rd32(&new_toc, 12)], 3);
    }

    fn anag() -> Vec<u8> {
        let mut out = b"ANAG".to_vec();
        out.extend_from_slice(&0x20u32.to_le_bytes());
        out.resize(0x20, 0xEE);
        // Deux emplacements vides pointent sur la vidéo suivante, comme dans le jeu
        let table = [0x38u32, 0x38, 0x78, 0xB8, 0xB8];
        out.extend_from_slice(&4u32.to_le_bytes());
        for position in table {
            out.extend_from_slice(&position.to_le_bytes());
        }
        out.extend(video(1, 0x40));
        out.extend(video(2, 0x40));
        out.extend(vec![0xAA; 16]);
        out
    }

    #[test]
    fn anag_videos_stay_in_place_when_they_fit_and_move_together_when_not() {
        let file = anag();
        assert_eq!(videos(&file, None), vec![(0x38, 0x78), (0x78, 0xB8)]);
        let small = rebuild(&file, None, &HashMap::from([(0x38, video(7, 0x20))])).unwrap().0;
        assert_eq!(small.len(), file.len());
        // La dernière garde la place libérée : sa fin ne bouge pas
        assert_eq!(videos(&small, None), vec![(0x38, 0x58), (0x58, 0xB8)]);
        let big = rebuild(&file, None, &HashMap::from([(0x38, video(7, 0x100))])).unwrap().0;
        assert_eq!(&big[0xB8..0xC8], &[0xAA; 16]);
        let found = videos(&big, None);
        assert_eq!(found[0].1 - found[0].0, 0x100);
        assert_eq!(big[found[1].0 + 4], 2);
        assert_eq!(found[1].1, big.len());
        assert_eq!(found[0].0 % 32, 0);
    }
}
