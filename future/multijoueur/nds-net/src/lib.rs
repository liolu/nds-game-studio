//! nds-net : multijoueur du studio (à venir, pas encore branché sur le moteur).
//!
//! Repris de SpaceSpore (`future/multijoueur/spacespore/net.rs`) et rendu indépendant du jeu :
//! - UDP direct, sans aucun service externe ;
//! - réseau local : `discover_lan` trouve les parties du même jeu par broadcast ;
//! - Internet : `InternetAccess` ouvre le port de la box (UPnP) ou trouve l'adresse publique
//!   (STUN) et donne un code d'invitation (`ABC-DEFG`) ;
//! - l'hôte relaie à tous l'état de chaque joueur : chaque jeu envoie ses propres données
//!   (`send_state`, octets libres : entrées DS en mode `inputs`, positions en mode `state`) ;
//! - `Hello` vérifie le protocole, la version du studio et le jeu ; sinon `Reject` avec une raison
//!   lisible ;
//! - chat confirmé par l'hôte.
//!
//! Utilisation : `NetSession::host` ou `NetSession::join`, puis `poll` à chaque image, qui renvoie
//! les événements (joueur arrivé, parti, état reçu, chat...). Voir `future/multijoueur.md`.

pub mod internet;
pub mod invite;

use std::collections::HashMap;
use std::io::{self, ErrorKind};
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};

use serde::{Deserialize, Serialize};

pub use internet::{Access, InternetAccess};
pub use invite::{decode_invite, encode_invite, parse_target};

/// Port du studio (SpaceSpore utilise 27777 : les deux peuvent tourner en même temps).
pub const NET_PORT: u16 = 27778;
const MAGIC: &str = "NDSGS";
const PROTOCOL: u32 = 1;
/// Joueurs au plus, hôte compris (comme le Wi-Fi local de la DS).
pub const MAX_PLAYERS: usize = 16;
pub const MAX_NAME_LEN: usize = 16;
pub const MAX_CHAT_LEN: usize = 120;
/// Taille maximale de l'état d'un joueur (un paquet UDP doit rester petit).
pub const MAX_STATE_LEN: usize = 1024;
const SEND_INTERVAL: f64 = 0.05;
const TIMEOUT: f64 = 6.0;
const JOIN_RETRY: f64 = 0.5;
const JOIN_TIMEOUT: f64 = 6.0;
const HOST_ID: u32 = 0;

/// Ce qui doit être identique chez tous les joueurs d'une partie.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameInfo {
    /// Version du studio (`ndsgs_common::VERSION`).
    pub studio: String,
    /// `id` du game.toml.
    pub game: String,
    /// Version du protocole propre au jeu (à augmenter quand son état change de forme).
    pub game_version: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub color: [f32; 3],
}

impl Profile {
    fn sanitized(mut self) -> Self {
        let clean: String = self.name.chars().filter(|c| !c.is_control()).take(MAX_NAME_LEN).collect();
        self.name = if clean.trim().is_empty() { "Joueur".into() } else { clean.trim().to_string() };
        self.color = self.color.map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 1.0 });
        self
    }
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "t")]
enum Msg {
    Discover { magic: String, game: String },
    HostInfo { magic: String, game: GameInfo, host: String, players: usize },
    Hello { magic: String, proto: u32, game: GameInfo, profile: Profile },
    Welcome { id: u32, players: Vec<(u32, Profile)> },
    Reject { reason: String },
    /// Client -> hôte : son état, et le chat pas encore confirmé (`seq` croissant).
    State { data: Vec<u8>, #[serde(default)] chat: Vec<(u32, String)> },
    /// Hôte -> clients : l'état de tous, les arrivées / départs et le chat.
    Snapshot {
        states: Vec<(u32, Vec<u8>)>,
        #[serde(default)] joined: Vec<(u32, Profile)>,
        #[serde(default)] left: Vec<u32>,
        #[serde(default)] chat: Vec<(u32, String)>,
        /// Dernier message de chat de ce client enregistré par l'hôte.
        #[serde(default)] ack: u32,
    },
    Bye,
}

fn send(socket: &UdpSocket, addr: SocketAddr, msg: &Msg) {
    if let Ok(bytes) = serde_json::to_vec(msg) {
        let _ = socket.send_to(&bytes, addr);
    }
}

/// Lit tous les paquets en attente sans bloquer.
fn recv_all(socket: &UdpSocket) -> Vec<(SocketAddr, Msg)> {
    let mut out = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    for _ in 0..512 {
        match socket.recv_from(&mut buf) {
            Ok((n, addr)) => {
                if let Ok(msg) = serde_json::from_slice::<Msg>(&buf[..n]) {
                    out.push((addr, msg));
                }
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
            // Windows renvoie ConnectionReset en UDP quand un pair a disparu : on ignore
            Err(e) if e.kind() == ErrorKind::ConnectionReset => continue,
            Err(_) => break,
        }
    }
    out
}

fn sanitize_chat(text: &str) -> Option<String> {
    let clean: String = text.chars().filter(|c| !c.is_control()).take(MAX_CHAT_LEN).collect();
    let clean = clean.trim();
    (!clean.is_empty()).then(|| clean.to_string())
}

/// Ce qui arrive pendant `poll`.
#[derive(Clone, Debug, PartialEq)]
pub enum NetEvent {
    /// Client : l'hôte nous a acceptés avec ce numéro de joueur.
    Connected { my_id: u32 },
    /// Client : refusé (raison lisible) ou hôte injoignable.
    Rejected(String),
    /// Client : plus de nouvelles de l'hôte.
    Disconnected,
    PlayerJoined { id: u32, profile: Profile },
    PlayerLeft { id: u32 },
    /// Dernier état envoyé par un joueur (le sien n'est pas renvoyé).
    State { id: u32, data: Vec<u8> },
    Chat { id: u32, text: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetMode {
    Hosting,
    Joining,
    Connected,
    Offline,
}

struct Client {
    id: u32,
    last_seen: f64,
    state: Vec<u8>,
    /// Dernier message de chat de ce client déjà relayé.
    chat_ack: u32,
}

/// Une partie en réseau, côté hôte ou côté client.
pub struct NetSession {
    socket: UdpSocket,
    game: GameInfo,
    profile: Profile,
    mode: NetMode,
    my_id: u32,
    host: Option<SocketAddr>,
    clients: HashMap<SocketAddr, Client>,
    next_id: u32,
    players: HashMap<u32, Profile>,
    my_state: Vec<u8>,
    /// Arrivées / départs / chat à annoncer au prochain envoi (hôte).
    pending_joined: Vec<(u32, Profile)>,
    pending_left: Vec<u32>,
    pending_chat: Vec<(u32, String)>,
    /// Client : mon chat pas encore confirmé par l'hôte, numéroté.
    chat_out: Vec<(u32, String)>,
    chat_seq: u32,
    started: f64,
    last_send: f64,
    last_hello: f64,
    last_recv: f64,
}

impl NetSession {
    /// Ouvre une partie sur `port` (`NET_PORT` en général). Les clients du réseau local la
    /// trouvent avec `discover_lan`.
    pub fn host(game: GameInfo, profile: Profile, port: u16) -> io::Result<Self> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port))?;
        let mut session = Self::new(socket, game, profile, NetMode::Hosting)?;
        session.players.insert(HOST_ID, session.profile.clone());
        Ok(session)
    }

    /// Rejoint la partie de `host` (adresse locale, ou `decode_invite` d'un code).
    pub fn join(game: GameInfo, profile: Profile, host: SocketAddr) -> io::Result<Self> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        let mut session = Self::new(socket, game, profile, NetMode::Joining)?;
        session.host = Some(host);
        session.last_hello = f64::NEG_INFINITY;
        Ok(session)
    }

    fn new(socket: UdpSocket, game: GameInfo, profile: Profile, mode: NetMode) -> io::Result<Self> {
        socket.set_nonblocking(true)?;
        socket.set_broadcast(true)?;
        Ok(Self {
            socket,
            game,
            profile: profile.sanitized(),
            mode,
            my_id: HOST_ID,
            host: None,
            clients: HashMap::new(),
            next_id: HOST_ID + 1,
            players: HashMap::new(),
            my_state: Vec::new(),
            pending_joined: Vec::new(),
            pending_left: Vec::new(),
            pending_chat: Vec::new(),
            chat_out: Vec::new(),
            chat_seq: 0,
            started: f64::NAN,
            last_send: f64::NEG_INFINITY,
            last_hello: f64::NEG_INFINITY,
            last_recv: 0.0,
        })
    }

    pub fn mode(&self) -> NetMode {
        self.mode
    }

    pub fn my_id(&self) -> u32 {
        self.my_id
    }

    pub fn is_host(&self) -> bool {
        self.mode == NetMode::Hosting
    }

    /// Joueurs de la partie (moi compris), par numéro.
    pub fn players(&self) -> &HashMap<u32, Profile> {
        &self.players
    }

    pub fn local_port(&self) -> u16 {
        self.socket.local_addr().map_or(0, |a| a.port())
    }

    /// Mon état, envoyé à chaque tick réseau (le jeu choisit son format).
    pub fn send_state(&mut self, data: Vec<u8>) {
        if data.len() <= MAX_STATE_LEN {
            self.my_state = data;
        }
    }

    pub fn say(&mut self, text: &str) {
        let Some(text) = sanitize_chat(text) else { return };
        if self.is_host() {
            self.pending_chat.push((HOST_ID, text));
        } else {
            self.chat_seq += 1;
            self.chat_out.push((self.chat_seq, text));
        }
    }

    /// À appeler à chaque image avec l'heure en secondes ; renvoie ce qui s'est passé.
    pub fn poll(&mut self, now: f64) -> Vec<NetEvent> {
        if self.started.is_nan() {
            self.started = now;
            self.last_recv = now;
        }
        let mut events = Vec::new();
        match self.mode {
            NetMode::Hosting => self.poll_host(now, &mut events),
            NetMode::Joining | NetMode::Connected => self.poll_client(now, &mut events),
            NetMode::Offline => {}
        }
        events
    }

    fn poll_host(&mut self, now: f64, events: &mut Vec<NetEvent>) {
        for (addr, msg) in recv_all(&self.socket) {
            match msg {
                Msg::Discover { magic, game } if magic == MAGIC && game == self.game.game => {
                    let info = Msg::HostInfo { magic: MAGIC.into(), game: self.game.clone(), host: self.profile.name.clone(), players: self.clients.len() + 1 };
                    send(&self.socket, addr, &info);
                }
                Msg::Hello { magic, proto, game, profile } if magic == MAGIC => {
                    if let Some(client) = self.clients.get_mut(&addr) {
                        // Hello répété (le Welcome s'est perdu)
                        client.last_seen = now;
                        let players = self.players.iter().map(|(id, p)| (*id, p.clone())).collect();
                        send(&self.socket, addr, &Msg::Welcome { id: client.id, players });
                        continue;
                    }
                    let refusal = if proto != PROTOCOL {
                        Some("Protocole reseau different : mettez le studio a jour.".to_string())
                    } else if game.game != self.game.game {
                        Some(format!("L'hote joue a un autre jeu ({}).", self.game.game))
                    } else if game.studio != self.game.studio || game.game_version != self.game.game_version {
                        Some(format!("Version differente : l'hote a le studio {} (vous : {}).", self.game.studio, game.studio))
                    } else if self.clients.len() + 1 >= MAX_PLAYERS {
                        Some(format!("Partie pleine ({MAX_PLAYERS} joueurs max)."))
                    } else {
                        None
                    };
                    if let Some(reason) = refusal {
                        send(&self.socket, addr, &Msg::Reject { reason });
                        continue;
                    }
                    let id = self.next_id;
                    self.next_id += 1;
                    let profile = profile.sanitized();
                    self.players.insert(id, profile.clone());
                    let players = self.players.iter().map(|(id, p)| (*id, p.clone())).collect();
                    send(&self.socket, addr, &Msg::Welcome { id, players });
                    self.clients.insert(addr, Client { id, last_seen: now, state: Vec::new(), chat_ack: 0 });
                    self.pending_joined.push((id, profile.clone()));
                    events.push(NetEvent::PlayerJoined { id, profile });
                }
                Msg::State { data, chat } => {
                    let Some(client) = self.clients.get_mut(&addr) else { continue };
                    client.last_seen = now;
                    if data.len() <= MAX_STATE_LEN && data != client.state {
                        client.state = data.clone();
                        events.push(NetEvent::State { id: client.id, data });
                    }
                    for (seq, text) in chat {
                        if seq > client.chat_ack {
                            client.chat_ack = seq;
                            if let Some(text) = sanitize_chat(&text) {
                                self.pending_chat.push((client.id, text));
                            }
                        }
                    }
                }
                Msg::Bye => {
                    if let Some(client) = self.clients.remove(&addr) {
                        self.player_left(client.id, events);
                    }
                }
                _ => {}
            }
        }
        let gone: Vec<SocketAddr> = self.clients.iter().filter(|(_, c)| now - c.last_seen > TIMEOUT).map(|(a, _)| *a).collect();
        for addr in gone {
            if let Some(client) = self.clients.remove(&addr) {
                self.player_left(client.id, events);
            }
        }
        // Le chat de l'hôte et celui relayé s'affichent aussi chez lui
        for (id, text) in &self.pending_chat {
            events.push(NetEvent::Chat { id: *id, text: text.clone() });
        }

        if now - self.last_send >= SEND_INTERVAL || !self.pending_joined.is_empty() || !self.pending_left.is_empty() || !self.pending_chat.is_empty() {
            self.last_send = now;
            let mut states: Vec<(u32, Vec<u8>)> = self.clients.values().map(|c| (c.id, c.state.clone())).collect();
            states.push((HOST_ID, self.my_state.clone()));
            for (addr, client) in &self.clients {
                let msg = Msg::Snapshot {
                    states: states.iter().filter(|(id, _)| *id != client.id).cloned().collect(),
                    joined: self.pending_joined.clone(),
                    left: self.pending_left.clone(),
                    chat: self.pending_chat.clone(),
                    ack: client.chat_ack,
                };
                send(&self.socket, *addr, &msg);
            }
            self.pending_joined.clear();
            self.pending_left.clear();
            self.pending_chat.clear();
        }
    }

    fn player_left(&mut self, id: u32, events: &mut Vec<NetEvent>) {
        self.players.remove(&id);
        self.pending_left.push(id);
        events.push(NetEvent::PlayerLeft { id });
    }

    fn poll_client(&mut self, now: f64, events: &mut Vec<NetEvent>) {
        let Some(host) = self.host else { return };
        for (addr, msg) in recv_all(&self.socket) {
            if addr != host {
                continue;
            }
            self.last_recv = now;
            match msg {
                Msg::Welcome { id, players } if self.mode == NetMode::Joining => {
                    self.mode = NetMode::Connected;
                    self.my_id = id;
                    events.push(NetEvent::Connected { my_id: id });
                    for (pid, profile) in players {
                        if pid != id {
                            events.push(NetEvent::PlayerJoined { id: pid, profile: profile.clone() });
                        }
                        self.players.insert(pid, profile);
                    }
                }
                Msg::Reject { reason } => {
                    self.mode = NetMode::Offline;
                    events.push(NetEvent::Rejected(reason));
                    return;
                }
                Msg::Snapshot { states, joined, left, chat, ack } if self.mode == NetMode::Connected => {
                    for (id, profile) in joined {
                        if id != self.my_id && self.players.insert(id, profile.clone()).is_none() {
                            events.push(NetEvent::PlayerJoined { id, profile });
                        }
                    }
                    for id in left {
                        if self.players.remove(&id).is_some() {
                            events.push(NetEvent::PlayerLeft { id });
                        }
                    }
                    for (id, data) in states {
                        if id != self.my_id {
                            events.push(NetEvent::State { id, data });
                        }
                    }
                    for (id, text) in chat {
                        events.push(NetEvent::Chat { id, text });
                    }
                    self.chat_out.retain(|(seq, _)| *seq > ack);
                }
                _ => {}
            }
        }
        match self.mode {
            NetMode::Joining => {
                if now - self.started > JOIN_TIMEOUT {
                    self.mode = NetMode::Offline;
                    events.push(NetEvent::Rejected("L'hote ne repond pas (code ou adresse faux, ou port ferme).".into()));
                } else if now - self.last_hello >= JOIN_RETRY {
                    self.last_hello = now;
                    let hello = Msg::Hello { magic: MAGIC.into(), proto: PROTOCOL, game: self.game.clone(), profile: self.profile.clone() };
                    send(&self.socket, host, &hello);
                }
            }
            NetMode::Connected => {
                if now - self.last_recv > TIMEOUT {
                    self.mode = NetMode::Offline;
                    events.push(NetEvent::Disconnected);
                } else if now - self.last_send >= SEND_INTERVAL {
                    self.last_send = now;
                    send(&self.socket, host, &Msg::State { data: self.my_state.clone(), chat: self.chat_out.clone() });
                }
            }
            _ => {}
        }
    }
}

impl Drop for NetSession {
    fn drop(&mut self) {
        if let (NetMode::Connected, Some(host)) = (self.mode, self.host) {
            send(&self.socket, host, &Msg::Bye);
        }
    }
}

/// Une partie trouvée sur le réseau local.
#[derive(Clone, Debug, PartialEq)]
pub struct LanGame {
    pub addr: SocketAddr,
    pub host: String,
    pub players: usize,
    pub game: GameInfo,
}

/// Cherche les parties de ce jeu sur le réseau local (et sur ce PC) pendant `wait`.
pub fn discover_lan(game: &str, port: u16, wait: std::time::Duration) -> io::Result<Vec<LanGame>> {
    let targets = [SocketAddr::new(Ipv4Addr::BROADCAST.into(), port), SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port)];
    discover_at(game, &targets, wait)
}

fn discover_at(game: &str, targets: &[SocketAddr], wait: std::time::Duration) -> io::Result<Vec<LanGame>> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    socket.set_broadcast(true)?;
    socket.set_nonblocking(true)?;
    for target in targets {
        send(&socket, *target, &Msg::Discover { magic: MAGIC.into(), game: game.into() });
    }
    let deadline = std::time::Instant::now() + wait;
    let mut found: Vec<LanGame> = Vec::new();
    while std::time::Instant::now() < deadline {
        for (addr, msg) in recv_all(&socket) {
            if let Msg::HostInfo { magic, game: info, host, players } = msg {
                if magic == MAGIC && info.game == game && !found.iter().any(|g| g.addr == addr) {
                    found.push(LanGame { addr, host, players, game: info });
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    // La partie la plus remplie d'abord (comme `pick_lan_host` de SpaceSpore)
    found.sort_by(|a, b| b.players.cmp(&a.players));
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn game(id: &str) -> GameInfo {
        GameInfo { studio: "0.1.0".into(), game: id.into(), game_version: 1 }
    }

    fn profile(name: &str) -> Profile {
        Profile { name: name.into(), color: [1.0, 0.5, 0.0] }
    }

    /// Fait tourner les sessions jusqu'à ce que `until` soit vrai (ou 3 s), en gardant les événements.
    fn run(sessions: &mut [&mut NetSession], log: &mut Vec<Vec<NetEvent>>, until: impl Fn(&[Vec<NetEvent>]) -> bool) {
        let start = std::time::Instant::now();
        log.resize(sessions.len(), Vec::new());
        while start.elapsed() < Duration::from_secs(3) {
            let now = start.elapsed().as_secs_f64();
            for (i, s) in sessions.iter_mut().enumerate() {
                log[i].extend(s.poll(now));
            }
            if until(log) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("delai depasse : {log:?}");
    }

    fn local(port: u16) -> SocketAddr {
        SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port)
    }

    #[test]
    fn two_players_join_exchange_state_and_chat() {
        let mut host = NetSession::host(game("platformer"), profile("Hote"), 0).unwrap();
        let port = host.local_port();
        let mut a = NetSession::join(game("platformer"), profile("Alice"), local(port)).unwrap();
        let mut b = NetSession::join(game("platformer"), profile("Bob"), local(port)).unwrap();
        let mut log = Vec::new();
        run(&mut [&mut host, &mut a, &mut b], &mut log, |l| {
            l[1].iter().any(|e| matches!(e, NetEvent::Connected { .. })) && l[2].iter().any(|e| matches!(e, NetEvent::Connected { .. }))
        });
        assert_eq!(host.players().len(), 3);
        let (ida, idb) = (a.my_id(), b.my_id());
        assert_ne!(ida, idb);

        // Chacun envoie son état : les autres le reçoivent par l'hôte
        host.send_state(vec![0]);
        a.send_state(vec![1, 2, 3]);
        b.send_state(vec![4]);
        a.say("salut");
        let mut log = Vec::new();
        run(&mut [&mut host, &mut a, &mut b], &mut log, |l| {
            l[2].contains(&NetEvent::State { id: ida, data: vec![1, 2, 3] })
                && l[1].contains(&NetEvent::State { id: HOST_ID, data: vec![0] })
                && l[0].contains(&NetEvent::State { id: idb, data: vec![4] })
                && l[2].contains(&NetEvent::Chat { id: ida, text: "salut".into() })
        });
        // Bob connaît Alice
        assert_eq!(b.players().get(&ida).map(|p| p.name.as_str()), Some("Alice"));

        // Alice part : l'hôte et Bob le voient
        drop(a);
        let mut log = Vec::new();
        run(&mut [&mut host, &mut b], &mut log, |l| l[0].contains(&NetEvent::PlayerLeft { id: ida }) && l[1].contains(&NetEvent::PlayerLeft { id: ida }));
    }

    #[test]
    fn other_game_or_version_is_rejected_with_a_reason() {
        let mut host = NetSession::host(game("bubbles"), profile("Hote"), 0).unwrap();
        let port = host.local_port();
        let mut other = NetSession::join(game("orbit"), profile("X"), local(port)).unwrap();
        let mut old = NetSession::join(GameInfo { studio: "0.0.9".into(), ..game("bubbles") }, profile("Y"), local(port)).unwrap();
        let mut log = Vec::new();
        run(&mut [&mut host, &mut other, &mut old], &mut log, |l| {
            l[1].iter().any(|e| matches!(e, NetEvent::Rejected(r) if r.contains("autre jeu")))
                && l[2].iter().any(|e| matches!(e, NetEvent::Rejected(r) if r.contains("Version")))
        });
        assert_eq!(host.players().len(), 1);
    }

    #[test]
    fn unreachable_host_gives_up() {
        // Port fermé : pas de réponse, abandon après JOIN_TIMEOUT (simulé avec une horloge avancée)
        let silent = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut s = NetSession::join(game("x"), profile("Z"), silent.local_addr().unwrap()).unwrap();
        assert!(s.poll(0.0).is_empty());
        assert!(matches!(s.poll(JOIN_TIMEOUT + 1.0).as_slice(), [NetEvent::Rejected(_)]));
        assert_eq!(s.mode(), NetMode::Offline);
    }

    #[test]
    fn lan_discovery_finds_games_of_the_same_id_only() {
        let mut host = NetSession::host(game("platformer"), profile("Salon"), 0).unwrap();
        let port = host.local_port();
        let target = [local(port)];
        let handle = std::thread::spawn(move || {
            (discover_at("platformer", &target, Duration::from_millis(400)).unwrap(), discover_at("bubbles", &target, Duration::from_millis(400)).unwrap())
        });
        let start = std::time::Instant::now();
        while !handle.is_finished() {
            host.poll(start.elapsed().as_secs_f64());
            std::thread::sleep(Duration::from_millis(5));
        }
        let (same, other) = handle.join().unwrap();
        assert_eq!(same.len(), 1);
        assert_eq!(same[0].host, "Salon");
        assert_eq!(same[0].players, 1);
        assert!(other.is_empty());
    }
}
