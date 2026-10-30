//! Code d'invitation : adresse Internet de l'hôte encodée en base 32 (alphabet de Crockford,
//! sans I, L, O, U pour éviter les confusions). Port standard : 7 caractères « ABC-DEFG » ;
//! autre port : 10 caractères. Repris tel quel de SpaceSpore (`net.rs`).

use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs};

use crate::NET_PORT;

const CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

pub fn encode_invite(ip: Ipv4Addr, port: u16) -> String {
    let (value, len) = if port == NET_PORT { (u32::from(ip) as u64, 7) } else { (((u32::from(ip) as u64) << 16) | port as u64, 10) };
    let mut chars: Vec<u8> = (0..len).map(|i| CODE_ALPHABET[((value >> (5 * (len - 1 - i))) & 31) as usize]).collect();
    let split = if len == 7 { 3 } else { 5 };
    chars.insert(split, b'-');
    String::from_utf8(chars).unwrap_or_default()
}

pub fn decode_invite(code: &str) -> Option<SocketAddr> {
    let mut value: u64 = 0;
    let mut len = 0;
    for c in code.chars() {
        if c == '-' || c == ' ' {
            continue;
        }
        let c = match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        };
        let digit = CODE_ALPHABET.iter().position(|&a| a as char == c)? as u64;
        value = (value << 5) | digit;
        len += 1;
        if len > 10 {
            return None;
        }
    }
    match len {
        7 if value <= u32::MAX as u64 => Some(SocketAddr::new(Ipv4Addr::from(value as u32).into(), NET_PORT)),
        10 if value < (1 << 48) => {
            let port = (value & 0xFFFF) as u16;
            let ip = Ipv4Addr::from((value >> 16) as u32);
            (port != 0).then(|| SocketAddr::new(ip.into(), port))
        }
        _ => None,
    }
}

/// Code d'invitation, ou à défaut une adresse IP (avec ou sans port) / un nom de machine.
pub fn parse_target(input: &str) -> Option<SocketAddr> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    if let Some(addr) = decode_invite(input).filter(|_| !input.contains('.') && !input.contains(':')) {
        return Some(addr);
    }
    if let Ok(addr) = input.parse::<SocketAddr>() {
        return Some(addr);
    }
    if let Ok(ip) = input.parse::<Ipv4Addr>() {
        return Some(SocketAddr::new(ip.into(), NET_PORT));
    }
    let with_port = if input.contains(':') { input.to_string() } else { format!("{input}:{NET_PORT}") };
    with_port.to_socket_addrs().ok()?.find(|a| a.is_ipv4())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_codes_round_trip() {
        let ip = Ipv4Addr::new(82, 64, 155, 7);
        let code = encode_invite(ip, NET_PORT);
        assert_eq!(code.len(), 8, "{code}");
        assert_eq!(decode_invite(&code), Some(SocketAddr::new(ip.into(), NET_PORT)));
        // Minuscules, espaces et lettres ambiguës acceptés
        assert_eq!(decode_invite(&code.to_lowercase().replace('-', " ")), Some(SocketAddr::new(ip.into(), NET_PORT)));
        let other = encode_invite(ip, 30000);
        assert_eq!(other.len(), 11);
        assert_eq!(decode_invite(&other), Some(SocketAddr::new(ip.into(), 30000)));
        assert_eq!(decode_invite("ABC"), None);
        assert_eq!(decode_invite("ABC-DEF!"), None);
    }

    #[test]
    fn targets_accept_codes_and_addresses() {
        let code = encode_invite(Ipv4Addr::new(10, 0, 0, 2), NET_PORT);
        assert_eq!(parse_target(&code), Some("10.0.0.2:27778".parse().unwrap()));
        assert_eq!(parse_target("192.168.1.20"), Some("192.168.1.20:27778".parse().unwrap()));
        assert_eq!(parse_target("192.168.1.20:4000"), Some("192.168.1.20:4000".parse().unwrap()));
        assert_eq!(parse_target("  "), None);
    }
}
