//! Messages BMG <-> texte : une ligne par message. Les commandes du jeu (couleur, pause, nom du héros...) sont gardées telles
//! quelles sous la forme `{1A:0605000000}` ; un retour à la ligne s'écrit `\n`.

use crate::container::{rd16, rd32, wr32};

fn codec(encoding: u8) -> &'static encoding_rs::Encoding {
    match encoding {
        3 => encoding_rs::SHIFT_JIS,
        4 => encoding_rs::UTF_8,
        _ => encoding_rs::WINDOWS_1252,
    }
}

fn blocks(file: &[u8]) -> Option<(usize, usize)> {
    let (mut inf, mut dat) = (None, None);
    let mut at = 0x20;
    while at + 8 <= file.len() {
        let size = rd32(file, at + 4);
        match &file[at..at + 4] {
            b"INF1" => inf = Some(at),
            b"DAT1" => dat = Some(at),
            _ => {}
        }
        if size < 8 {
            break;
        }
        at += size;
    }
    Some((inf?, dat?))
}

fn push_text(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\\' => out.push_str("\\\\"),
            '{' => out.push_str("{{"),
            c => out.push(c),
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

/// Un message, depuis sa position dans DAT1.
fn message(rest: &[u8], encoding: u8) -> String {
    let mut out = String::new();
    if encoding == 2 {
        let (mut p, mut units) = (0, Vec::new());
        let flush = |units: &mut Vec<u16>, out: &mut String| {
            push_text(out, &String::from_utf16_lossy(units));
            units.clear();
        };
        while let Some(c) = rest.get(p..p + 2).map(|s| u16::from_le_bytes([s[0], s[1]])) {
            if c == 0 {
                break;
            }
            if c == 0x1A {
                flush(&mut units, &mut out);
                let size = (rest.get(p + 2).copied().unwrap_or(4) as usize).max(4);
                out.push_str(&format!("{{1A:{}}}", hex(rest.get(p + 2..p + size).unwrap_or(&[]))));
                p += size;
                continue;
            }
            units.push(c);
            p += 2;
        }
        flush(&mut units, &mut out);
    } else {
        let (mut p, mut start) = (0, 0);
        let flush = |from: usize, to: usize, out: &mut String| push_text(out, &codec(encoding).decode(&rest[from..to]).0);
        while let Some(&b) = rest.get(p) {
            if b == 0 {
                break;
            }
            if b == 0x1A {
                flush(start, p, &mut out);
                let size = (rest.get(p + 1).copied().unwrap_or(2) as usize).max(2);
                out.push_str(&format!("{{1A:{}}}", hex(rest.get(p + 1..p + size).unwrap_or(&[]))));
                p += size;
                start = p;
                continue;
            }
            p += 1;
        }
        flush(start, p.min(rest.len()), &mut out);
    }
    out
}

/// Tous les messages, un par ligne.
pub fn bmg_to_text(file: &[u8]) -> Option<String> {
    let (inf, dat) = blocks(file)?;
    let encoding = *file.get(0x10)?;
    let (count, entry) = (rd16(file, inf + 8), rd16(file, inf + 10).max(4));
    let mut out = String::new();
    for i in 0..count {
        let offset = rd32(file, inf + 16 + i * entry);
        out.push_str(&message(file.get(dat + 8 + offset..).unwrap_or(&[]), encoding));
        out.push_str("\r\n");
    }
    Some(out)
}

fn encode_text(text: &str, encoding: u8, out: &mut Vec<u8>, lossy: &mut bool) {
    if encoding == 2 {
        out.extend(text.encode_utf16().flat_map(|c| c.to_le_bytes()));
    } else {
        let (bytes, _, bad) = codec(encoding).encode(text);
        *lossy |= bad;
        out.extend(bytes.iter());
    }
}

/// Une ligne -> les octets de son message, avec la fin de texte.
fn encode_line(line: &str, encoding: u8, lossy: &mut bool) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('n') => text.push('\n'),
                Some('\\') => text.push('\\'),
                other => return Err(format!("\\{} inconnu", other.map_or(String::new(), |c| c.to_string()))),
            },
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                text.push('{');
            }
            '{' => {
                encode_text(&text, encoding, &mut out, lossy);
                text.clear();
                let command: String = chars.by_ref().take_while(|c| *c != '}').collect();
                let Some(digits) = command.strip_prefix("1A:") else { return Err(format!("commande {{{command}}} inconnue")) };
                if digits.len() % 2 != 0 {
                    return Err(format!("commande {{{command}}} coupee"));
                }
                if encoding == 2 {
                    out.extend([0x1A, 0]);
                } else {
                    out.push(0x1A);
                }
                for pair in digits.as_bytes().chunks(2) {
                    let value = std::str::from_utf8(pair).ok().and_then(|s| u8::from_str_radix(s, 16).ok());
                    out.push(value.ok_or_else(|| format!("commande {{{command}}} illisible"))?);
                }
            }
            c => text.push(c),
        }
    }
    encode_text(&text, encoding, &mut out, lossy);
    out.extend(if encoding == 2 { &[0u8, 0][..] } else { &[0u8][..] });
    Ok(out)
}

/// Le fichier BMG avec ces messages. Rend aussi `true` si des caractères n'existent pas dans l'encodage du jeu.
pub fn text_to_bmg(file: &[u8], text: &str) -> Result<(Vec<u8>, bool), String> {
    let (inf, dat) = blocks(file).ok_or("BMG sans table")?;
    let encoding = file[0x10];
    let (count, entry) = (rd16(file, inf + 8), rd16(file, inf + 10).max(4));
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() != count {
        return Err(format!("{} lignes, {count} attendues : garder une ligne par message", lines.len()));
    }
    let mut out = file[..dat + 8].to_vec();
    let mut data = Vec::new();
    // Ce qui précède le 1er message (le plus souvent un texte vide à la position 0)
    let first = (0..count).map(|i| rd32(file, inf + 16 + i * entry)).min().unwrap_or(0);
    data.extend(file.get(dat + 8..dat + 8 + first).unwrap_or(&[]));
    let mut lossy = false;
    for (i, line) in lines.iter().enumerate() {
        wr32(&mut out, inf + 16 + i * entry, data.len());
        data.extend(encode_line(line, encoding, &mut lossy).map_err(|e| format!("ligne {} : {e}", i + 1))?);
    }
    out.extend(&data);
    out.resize(out.len().next_multiple_of(32), 0);
    let dat_end = dat + rd32(file, dat + 4);
    let new_size = out.len() - dat;
    wr32(&mut out, dat + 4, new_size);
    out.extend(file.get(dat_end..).unwrap_or(&[]));
    // La taille du fichier est en octets ou en blocs de 32
    let total = out.len();
    let old = rd32(file, 8);
    wr32(&mut out, 8, if old == file.len() || old * 32 != file.len() { total } else { total.div_ceil(32) });
    Ok((out, lossy))
}

/// Les textes d'un `GNAL`, chacun (position de sa taille, ses octets).
fn gnal_texts(file: &[u8]) -> Option<Vec<(usize, &[u8])>> {
    let mut at = 8;
    (0..rd32(file, 4))
        .map(|_| {
            let size = file.get(at + 8..at + 12).map(|_| rd32(file, at + 8))?;
            let text = file.get(at + 12..at + 12 + size)?;
            let found = (at + 8, text);
            at += 12 + size;
            Some(found)
        })
        .collect()
}

/// Un fichier `GNAL` (nombre de textes, puis pour chacun deux nombres, sa taille, ses octets en Windows-1252), une ligne par texte. Ce
/// qui ne s'écrit pas dans une ligne passe par `\\n`, `\\r`, `\\0`.
pub fn gnal_to_text(file: &[u8]) -> Option<String> {
    let mut out = String::new();
    for (_, text) in gnal_texts(file)? {
        for c in encoding_rs::WINDOWS_1252.decode_without_bom_handling(text).0.chars() {
            match c {
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\0' => out.push_str("\\0"),
                '\\' => out.push_str("\\\\"),
                c => out.push(c),
            }
        }
        out.push_str("\r\n");
    }
    Some(out)
}

/// Le fichier `GNAL` avec ces textes. Rend aussi `true` si des caractères n'existent pas en Windows-1252.
pub fn text_to_gnal(file: &[u8], text: &str) -> Result<(Vec<u8>, bool), String> {
    let texts = gnal_texts(file).ok_or("GNAL illisible")?;
    let lines: Vec<&str> = text.split("\r\n").flat_map(|line| line.split('\n')).collect();
    let lines = &lines[..lines.len().saturating_sub(usize::from(lines.last() == Some(&"")))];
    if lines.len() != texts.len() {
        return Err(format!("{} lignes, {} attendues : garder une ligne par texte", lines.len(), texts.len()));
    }
    let mut out = file[..8].to_vec();
    let mut lossy = false;
    let mut end = 8;
    for (n, ((size_at, old), line)) in texts.iter().zip(lines).enumerate() {
        let mut plain = String::new();
        let mut chars = line.chars();
        while let Some(c) = chars.next() {
            plain.push(match c {
                '\\' => match chars.next() {
                    Some('n') => '\n',
                    Some('r') => '\r',
                    Some('0') => '\0',
                    Some('\\') => '\\',
                    other => return Err(format!("ligne {} : \\{} inconnu", n + 1, other.map_or(String::new(), |c| c.to_string()))),
                },
                c => c,
            });
        }
        let (bytes, _, bad) = encoding_rs::WINDOWS_1252.encode(&plain);
        lossy |= bad;
        out.extend(&file[*size_at - 8..*size_at]);
        out.extend((bytes.len() as u32).to_le_bytes());
        out.extend(bytes.iter());
        end = *size_at + 4 + old.len();
    }
    out.extend(&file[end..]);
    Ok((out, lossy))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gnal_texts_can_change_size() {
        let mut file = b"GNAL".to_vec();
        file.extend(2u32.to_le_bytes());
        for (id, text) in [(7u32, &b"a\xE9\\b\n\0"[..]), (8, b"")] {
            file.extend(id.to_le_bytes());
            file.extend(1u32.to_le_bytes());
            file.extend((text.len() as u32).to_le_bytes());
            file.extend(text);
        }
        file.extend([1, 2, 3, 4]);
        let text = gnal_to_text(&file).unwrap();
        assert_eq!(text, "a\u{E9}\\\\b\\n\\0\r\n\r\n");
        assert_eq!(text_to_gnal(&file, &text).unwrap(), (file.clone(), false));
        let (out, _) = text_to_gnal(&file, "plus long que l'autre\r\nb\r\n").unwrap();
        assert_eq!(gnal_to_text(&out).unwrap(), "plus long que l'autre\r\nb\r\n");
        assert_eq!(&out[out.len() - 4..], [1, 2, 3, 4], "ce qui suit les textes reste");
        assert!(text_to_gnal(&file, "une seule\r\n").is_err());
    }

    fn bmg(encoding: u8, messages: &[&[u8]]) -> Vec<u8> {
        let mut file = b"MESGbmg1".to_vec();
        file.extend([0u8; 8]);
        file.push(encoding);
        file.resize(0x20, 0);
        let mut data: Vec<u8> = Vec::new();
        let mut offsets = Vec::new();
        for m in messages {
            offsets.push(data.len() as u32);
            data.extend(*m);
        }
        file.extend(b"INF1");
        file.extend((16u32 + messages.len() as u32 * 4).to_le_bytes());
        file.extend((messages.len() as u16).to_le_bytes());
        file.extend(4u16.to_le_bytes());
        file.extend([0u8; 4]);
        for o in offsets {
            file.extend(o.to_le_bytes());
        }
        file.extend(b"DAT1");
        file.extend((8 + data.len() as u32).to_le_bytes());
        file.extend(data);
        file
    }

    #[test]
    fn utf16_messages_keep_their_commands() {
        let hello: Vec<u8> = "Hi\n{".encode_utf16().flat_map(|c| c.to_le_bytes()).chain([0x1A, 0, 6, 1, 2, 3, b'!', 0, 0, 0]).collect();
        let file = bmg(2, &[&hello]);
        let text = bmg_to_text(&file).unwrap();
        assert_eq!(text, "Hi\\n{{{1A:06010203}!\r\n");
        let (out, lossy) = text_to_bmg(&file, &text).unwrap();
        assert!(!lossy);
        assert_eq!(bmg_to_text(&out).unwrap(), text);
    }

    #[test]
    fn messages_can_grow_and_the_others_follow() {
        let file = bmg(1, &[b"one\0", b"two\0"]);
        let (out, _) = text_to_bmg(&file, "une phrase bien plus longue\r\ntwo\r\n").unwrap();
        assert_eq!(bmg_to_text(&out).unwrap(), "une phrase bien plus longue\r\ntwo\r\n");
    }

    #[test]
    fn a_missing_line_is_refused() {
        let file = bmg(1, &[b"one\0", b"two\0"]);
        assert!(text_to_bmg(&file, "one\n").is_err());
    }

    #[test]
    fn shift_jis_stays_shift_jis() {
        let (bytes, _, _) = encoding_rs::SHIFT_JIS.encode("ゲーム");
        let file = bmg(3, &[&[&bytes[..], &[0]].concat()]);
        let text = bmg_to_text(&file).unwrap();
        assert_eq!(text.trim_end(), "ゲーム");
        let (_, lossy) = text_to_bmg(&file, "é").unwrap_or((Vec::new(), true));
        assert!(lossy, "un caractere absent de l'encodage est signale");
    }
}
