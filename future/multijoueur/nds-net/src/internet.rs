//! Jouer par Internet sans rien régler : ouverture automatique du port de la box (UPnP), et à
//! défaut adresse publique trouvée par STUN. Les deux tournent dans un fil en arrière-plan ;
//! `InternetAccess::invite` donne le code à envoyer à ses amis. Repris de SpaceSpore (`net.rs`).

use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::invite::encode_invite;

/// Où en est l'accès depuis Internet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Access {
    Pending,
    /// Code d'invitation prêt (port ouvert sur la box, ou adresse trouvée par STUN).
    Ready { code: String, upnp: bool },
    /// Raison lisible par le joueur.
    Unavailable(String),
}

/// Recherche en arrière-plan, lancée quand le joueur ouvre une partie.
pub struct InternetAccess {
    state: Arc<Mutex<Access>>,
}

impl InternetAccess {
    pub fn start(port: u16, app_name: &'static str) -> Self {
        let state = Arc::new(Mutex::new(Access::Pending));
        let shared = state.clone();
        std::thread::spawn(move || {
            let result = match open_with_upnp(port, app_name) {
                Ok((ip, port, gateway, local)) => {
                    set(&shared, Access::Ready { code: encode_invite(ip, port), upnp: true });
                    // Renouvelle l'ouverture tant que le jeu tourne
                    loop {
                        std::thread::sleep(Duration::from_secs(30 * 60));
                        let _ = add_mapping(&gateway, port, local, app_name);
                    }
                }
                Err(upnp_error) => match public_ip_with_stun() {
                    Some(ip) => Access::Ready { code: encode_invite(ip, port), upnp: false },
                    None => Access::Unavailable(upnp_error),
                },
            };
            set(&shared, result);
        });
        Self { state }
    }

    pub fn access(&self) -> Access {
        self.state.lock().map(|s| s.clone()).unwrap_or(Access::Pending)
    }
}

fn set(state: &Mutex<Access>, value: Access) {
    if let Ok(mut s) = state.lock() {
        *s = value;
    }
}

/// IP locale utilisée pour sortir sur le réseau (aucun paquet n'est envoyé).
pub fn local_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(8, 8, 8, 8), 80)).ok()?;
    let ip = socket.local_addr().ok()?.ip();
    if ip.is_unspecified() { None } else { Some(ip) }
}

pub fn is_non_public(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || (o[0] == 100 && (64..128).contains(&o[1])) // CGNAT 100.64.0.0/10
}

fn add_mapping(gw: &igd_next::Gateway, port: u16, local: SocketAddr, name: &str) -> Result<(), igd_next::AddPortError> {
    use igd_next::{AddPortError, PortMappingProtocol};
    match gw.add_port(PortMappingProtocol::UDP, port, local, 3600, name) {
        Err(AddPortError::OnlyPermanentLeasesSupported) => gw.add_port(PortMappingProtocol::UDP, port, local, 0, name),
        r => r,
    }
}

fn open_with_upnp(port: u16, name: &str) -> Result<(Ipv4Addr, u16, igd_next::Gateway, SocketAddr), String> {
    let local = match local_ip() {
        Some(IpAddr::V4(ip)) => ip,
        _ => return Err("Pas de connexion reseau.".into()),
    };
    // Recherche de la box depuis la bonne carte réseau (important sous Windows avec VPN / cartes
    // virtuelles), puis sur toutes à défaut.
    let search = |bind: SocketAddr| igd_next::search_gateway(igd_next::SearchOptions { bind_addr: bind, timeout: Some(Duration::from_secs(8)), ..Default::default() });
    let gw = search(SocketAddr::new(IpAddr::V4(local), 0))
        .or_else(|_| search((Ipv4Addr::UNSPECIFIED, 0).into()))
        .map_err(|_| "Votre box n'a pas repondu a la demande d'ouverture automatique (UPnP desactive sur la box ?).".to_string())?;
    let ext = match gw.get_external_ip() {
        Ok(IpAddr::V4(ip)) => ip,
        _ => return Err("Adresse Internet introuvable.".into()),
    };
    if is_non_public(ext) {
        return Err("Votre connexion Internet est partagee (CGNAT) : vos amis ne peuvent pas vous rejoindre, mais vous pouvez rejoindre les leurs.".into());
    }
    let local_addr = SocketAddr::new(IpAddr::V4(local), port);
    // Si un autre PC de la maison utilise déjà le port, on en prend un voisin
    for p in port..port + 10 {
        if add_mapping(&gw, p, local_addr, name).is_ok() {
            return Ok((ext, p, gw, local_addr));
        }
    }
    Err("La box a refuse d'ouvrir le port (UPnP limite sur la box ?).".into())
}

/// Adresse publique vue depuis Internet (fonctionne sur la plupart des box, NAT « cone »).
fn public_ip_with_stun() -> Option<Ipv4Addr> {
    let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    sock.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    ["stun.l.google.com:19302", "stun1.l.google.com:19302", "stun2.l.google.com:19302"]
        .iter()
        .find_map(|server| stun_query(&sock, server))
        .filter(|ip| !is_non_public(*ip))
}

fn stun_query(sock: &UdpSocket, server: &str) -> Option<Ipv4Addr> {
    let addr = server.to_socket_addrs().ok()?.find(|a| a.is_ipv4())?;
    // Binding Request : type 0x0001, longueur 0, magic cookie, identifiant de transaction
    let mut req = [0u8; 20];
    req[1] = 0x01;
    req[4..8].copy_from_slice(&[0x21, 0x12, 0xA4, 0x42]);
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    for (i, b) in req[8..20].iter_mut().enumerate() {
        *b = (seed >> (i * 8 % 120)) as u8 ^ (i as u8).wrapping_mul(37);
    }
    sock.send_to(&req, addr).ok()?;
    let mut buf = [0u8; 256];
    let n = sock.recv(&mut buf).ok()?;
    parse_stun_response(&buf[..n])
}

/// Adresse IPv4 d'une réponse STUN (XOR-MAPPED-ADDRESS, ou MAPPED-ADDRESS).
fn parse_stun_response(buf: &[u8]) -> Option<Ipv4Addr> {
    if buf.len() < 20 || buf[0] != 0x01 || buf[1] != 0x01 {
        return None; // pas une Binding Response
    }
    let end = (20 + u16::from_be_bytes([buf[2], buf[3]]) as usize).min(buf.len());
    let mut i = 20;
    while i + 4 <= end {
        let kind = u16::from_be_bytes([buf[i], buf[i + 1]]);
        let len = u16::from_be_bytes([buf[i + 2], buf[i + 3]]) as usize;
        let start = i + 4;
        if len >= 8 && start + len <= end && buf[start + 1] == 0x01 {
            let ip = &buf[start + 4..start + 8];
            match kind {
                0x0020 => return Some(Ipv4Addr::new(ip[0] ^ 0x21, ip[1] ^ 0x12, ip[2] ^ 0xA4, ip[3] ^ 0x42)),
                0x0001 => return Some(Ipv4Addr::new(ip[0], ip[1], ip[2], ip[3])),
                _ => {}
            }
        }
        i = start + ((len + 3) & !3);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stun_responses_are_decoded() {
        // Binding Response avec XOR-MAPPED-ADDRESS = 82.64.155.7
        let mut r = vec![0x01, 0x01, 0x00, 0x0C, 0x21, 0x12, 0xA4, 0x42];
        r.extend([0u8; 12]);
        r.extend([0x00, 0x20, 0x00, 0x08, 0x00, 0x01, 0x00, 0x00, 82 ^ 0x21, 64 ^ 0x12, 155 ^ 0xA4, 7 ^ 0x42]);
        assert_eq!(parse_stun_response(&r), Some(Ipv4Addr::new(82, 64, 155, 7)));
        r[1] = 0x11; // erreur, pas une réponse
        assert_eq!(parse_stun_response(&r), None);
    }

    #[test]
    fn private_and_cgnat_addresses_are_not_public() {
        assert!(is_non_public(Ipv4Addr::new(192, 168, 1, 2)));
        assert!(is_non_public(Ipv4Addr::new(100, 70, 0, 1)));
        assert!(!is_non_public(Ipv4Addr::new(82, 64, 155, 7)));
    }
}
