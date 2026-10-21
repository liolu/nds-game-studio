// ─────────────────────────────────────────────────────────────────────────
//  Multijoueur
//
//  Réseau UDP direct, sans aucun service externe (pas de Steam, Hamachi…).
//  Rien n'est ouvert tant que le joueur n'a pas ouvert le panneau
//  Multijoueur (F2). Ensuite tout est automatique :
//   - le jeu ouvre une partie (port UDP NET_PORT) ;
//   - les jeux d'un même réseau local se trouvent par broadcast et se
//     regroupent tout seuls dans une seule partie ;
//   - pour Internet, la box est ouverte automatiquement (UPnP) et l'hôte
//     reçoit un petit code d'invitation (son adresse encodée) à donner
//     à ses amis.
//
//  La carte est générée localement chez chaque joueur (même code, même
//  graine). Seuls s'échangent : position/orientation du vaisseau, pseudo et
//  couleur de contour, plus le système stellaire où l'on se trouve et son
//  horloge d'univers. L'hôte relaie l'état de tout le monde.
//
//  Échanges limités au nécessaire :
//   - à chaque tick, seulement ce qui bouge (position, système, horloge,
//     coque / tirs en combat) et l'empreinte du profil du joueur ;
//   - le profil (pseudo, couleur, étoiles revendiquées, diplomatie, guilde)
//     n'est transmis que lorsqu'il change, à ceux qui n'ont pas son empreinte ;
//   - les fiches de guilde ne sont transmises que lorsqu'elles changent ou
//     qu'un joueur annonce une révision plus récente que celle qu'on a ;
//   - profils et fiches reçus sont gardés sur disque (`net_cache.json`) :
//     à la prochaine rencontre, rien n'est redemandé si rien n'a changé.
//
//  Orbites : chacun garde sa propre horloge. Elle n'est alignée que lorsque
//  deux joueurs sont dans le même système : celui qui arrive prend l'horloge
//  de celui qui y est depuis le plus longtemps, pour que les planètes soient
//  au même endroit chez les deux. Dans des systèmes différents, rien n'est
//  synchronisé (inutile : on ne voit pas les planètes de l'autre).
// ─────────────────────────────────────────────────────────────────────────

use bevy::math::DVec3;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::planet::SpawnedSystems;
use crate::settings::GameSettings;
use crate::ship::{outline_material, LocalOutline, Ship};
use crate::CameraController;

pub const NET_PORT: u16 = 27777;
pub const MAX_NAME_LEN: usize = 16;
pub const MAX_TAG_LEN: usize = 5;
/// Étoiles revendiquées au plus par joueur.
pub const MAX_CLAIMS: usize = 5;
const PROTOCOL: u32 = 39;
/// Modifications de voxels gardées en attente au plus (protection contre un flot).
const VOXEL_EDITS_MAX: usize = 512;
pub const MAX_CHAT_LEN: usize = 120;
/// Messages gardés à l'écran / dans l'historique de l'hôte.
const CHAT_HISTORY: usize = 50;
/// Messages de chat au plus par paquet (taille des paquets UDP).
const CHAT_PER_PACKET: usize = 8;
const CHAT_OUTBOX_MAX: usize = 20;
const GUILD_INBOX_MAX: usize = 64;
/// Délai entre deux demandes de profils / fiches manquants (secondes).
const WANT_INTERVAL: f64 = if cfg!(test) { 0.05 } else { 0.5 };
/// Le code d'invitation est rappelé aux clients à cet intervalle (secondes).
const CODE_INTERVAL: f64 = 2.0;
const PROFILES_PER_PACKET: usize = 4;
const MAGIC: &str = "SPACESPORE";
const GAME_VERSION: &str = spacespore_common::VERSION;
const MAX_PLAYERS: usize = 16;
const SEND_INTERVAL: f64 = 0.05;
const TIMEOUT: f64 = 6.0;
const JOIN_RETRY: f64 = 0.5;
const JOIN_TIMEOUT: f64 = 6.0;
const DISCOVER_INTERVAL: f64 = 1.5;
const LAN_EXPIRY: f64 = 5.0;
const HOST_RETRY: f64 = 3.0;
const AVOID_DURATION: f64 = 30.0;
const HOST_ID: u32 = 0;
/// Écart d'horloge toléré entre deux joueurs d'un même système (secondes).

pub struct NetPlugin;

impl Plugin for NetPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<crate::world_clock::WorldClock>()
            .init_resource::<Net>()
            .add_event::<NetCommand>()
            .add_systems(
                Update,
                (
                    handle_net_commands,
                    net_update,
                    sync_remote_ships,
                    update_remote_labels,
                    update_local_outline,
                )
                    .chain(),
            )
            .add_systems(Last, cleanup_on_exit);
    }
}

// ─────────────────────────────────────────────────────────────────────────
//  Protocole
// ─────────────────────────────────────────────────────────────────────────

/// Ce qui change sans arrêt : envoyé à chaque tick. Tout le reste est dans
/// le `Profile`, désigné ici par son empreinte et envoyé seulement à ceux
/// qui ne l'ont pas encore.
#[derive(Serialize, Deserialize, Clone, Debug)]
struct PlayerState {
    id: u32,
    /// Empreinte du profil actuel du joueur (voir `Profile::fingerprint`).
    ph: u64,
    /// Position ABSOLUE (l'origine flottante de chaque joueur est différente).
    pos: [f64; 3],
    rot: [f32; 4],
    /// Système stellaire où se trouve le joueur (None = espace profond).
    sys: Option<u32>,
    /// Depuis combien de secondes il est dans ce système.
    stay: f64,
    /// Son horloge du monde (`WorldClock`) et sa vitesse : celles de l'hôte font foi.
    clock: f64,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    speed: f64,
    /// Coque du vaisseau (absente du paquet quand elle est intacte).
    #[serde(default = "full_hp", skip_serializing_if = "is_full_hp")]
    hp: u8,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    hits: Vec<(u32, u32)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    siege: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    taken: Vec<u32>,
    /// Modèles du joueur et ce qu'ils font (E7).
    #[serde(default, skip_serializing_if = "crate::net_models::Looks::is_default")]
    look: crate::net_models::Looks,
}

fn is_full_hp(hp: &u8) -> bool {
    *hp >= MAX_HP
}

/// Ce qui change rarement chez un joueur. N'est transmis que lorsqu'il
/// change, et gardé sur disque par ceux qui l'ont reçu : à la prochaine
/// rencontre, l'empreinte suffit.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Profile {
    /// Identifiant permanent du joueur.
    pub pid: u64,
    pub name: String,
    pub color: [f32; 3],
    /// Systèmes stellaires revendiqués (MAX_CLAIMS au plus).
    #[serde(default)]
    pub claims: Vec<u32>,
    /// Factions déclarées alliées / ennemies (voir `diplomacy::faction_key`).
    #[serde(default)]
    pub allies: Vec<String>,
    #[serde(default)]
    pub enemies: Vec<String>,
    /// Guilde dont il se dit membre et révision de la fiche qu'il en a (0 = aucune).
    #[serde(default)]
    pub gid: u64,
    #[serde(default)]
    pub grev: u64,
    /// Guilde qu'il demande à rejoindre (0 = aucune).
    #[serde(default)]
    pub req: u64,
    /// Fiches de guildes qu'il a quittées ou dissoutes : (guilde, révision).
    #[serde(default)]
    pub archive: Vec<(u64, u64)>,
}

impl Profile {
    fn sanitized(mut self) -> Self {
        let keys = |v: &mut Vec<String>| {
            v.truncate(MAX_RELATIONS);
            for k in v.iter_mut() {
                *k = k.chars().filter(|c| !c.is_control()).take(MAX_NAME_LEN + 2).collect();
            }
        };
        self.name = sanitize_name(&self.name);
        self.color = sanitize_color(self.color);
        self.claims.truncate(MAX_CLAIMS);
        keys(&mut self.allies);
        keys(&mut self.enemies);
        self.archive.truncate(8);
        self
    }

    /// Empreinte du contenu : deux profils identiques ont la même, partout.
    pub fn fingerprint(&self) -> u64 {
        let json = serde_json::to_string(self).unwrap_or_default();
        json.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
    }
}

/// Profils connus, par empreinte (les plus anciens sont oubliés).
#[derive(Default)]
pub struct ProfileCache {
    map: HashMap<u64, Profile>,
    order: VecDeque<u64>,
}

impl ProfileCache {
    const MAX: usize = 64;

    pub fn get(&self, fingerprint: u64) -> Option<&Profile> {
        self.map.get(&fingerprint)
    }

    /// Ajoute un profil ; renvoie vrai s'il était inconnu.
    pub fn insert(&mut self, profile: Profile) -> bool {
        let profile = profile.sanitized();
        let key = profile.fingerprint();
        if self.map.insert(key, profile).is_some() {
            return false;
        }
        self.order.push_back(key);
        while self.order.len() > Self::MAX {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
        true
    }

    pub fn all(&self) -> Vec<Profile> {
        self.order.iter().filter_map(|k| self.map.get(k).cloned()).collect()
    }
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "t")]
enum Msg {
    Discover { magic: String },
    HostInfo { magic: String, sid: u64, players: usize, game: String, world: u64 },
    Hello { magic: String, proto: u32, game: String, world: u64, name: String, color: [f32; 3] },
    Welcome { id: u32 },
    Reject { reason: String },
    /// `chat` : messages pas encore confirmés par l'hôte ; `seen` : dernier
    /// message de l'hôte reçu.
    /// `profile` : mon profil, tant que l'hôte ne l'a pas ; `want` : profils
    /// qui me manquent ; `gwant` : fiches de guilde qui me manquent.
    State {
        state: PlayerState,
        #[serde(default, skip_serializing_if = "Vec::is_empty")] chat: Vec<ChatOut>,
        #[serde(default)] seen: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")] profile: Option<Profile>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")] want: Vec<u64>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")] gwant: Vec<u64>,
    },
    /// `chat` : messages que ce client n'a pas encore reçus ; `ack` : dernier
    /// message du client enregistré par l'hôte ; `code` : code d'invitation
    /// (seulement de temps en temps) ; `pack` : empreinte du profil que l'hôte
    /// a de ce client ; `profiles` : profils demandés ; `gwant` : fiches de
    /// guilde que l'hôte demande à ce client.
    Snapshot {
        #[serde(default, skip_serializing_if = "Option::is_none")] code: Option<String>,
        players: Vec<PlayerState>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")] chat: Vec<ChatLine>,
        #[serde(default)] ack: u32,
        #[serde(default)] pack: u64,
        #[serde(default, skip_serializing_if = "Vec::is_empty")] profiles: Vec<Profile>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")] gwant: Vec<u64>,
    },
    /// Fiche de guilde, envoyée quand elle change ou quand elle est demandée.
    Guild { guild: crate::guild::GuildRecord },
    /// Cellules voxel modifiées (cratère d'impact, minage) : l'hôte les relaie à tous.
    Voxels { edits: Vec<crate::voxel::VoxelEdit> },
    /// Modèle demandé par empreinte (règle 7) : les morceaux qui manquent (`[0]` au début).
    ModelWant { fp: u64, parts: Vec<u32> },
    /// Un morceau d'un fichier de modèle (hexadécimal).
    ModelPart { fp: u64, index: u32, total: u32, data: String },
    /// Un joueur partage un modèle (E8) : chacun le reçoit par son empreinte, dans sa bibliothèque.
    Share { fp: u64, name: String, from: String },
    Bye,
}

pub const MAX_HP: u8 = 100;
/// Relations déclarées au plus (alliés, et autant d'ennemis).
pub const MAX_RELATIONS: usize = 16;

fn full_hp() -> u8 { MAX_HP }

/// Combat, sièges et diplomatie connus pour un joueur : le combat vient de
/// son état (chaque tick), le reste de son profil.
#[derive(Clone, Debug)]
pub struct PlayerStatus {
    /// Coque du vaisseau (0 = détruit, en attente de réapparition).
    pub hp: u8,
    /// Tirs réussis sur chaque joueur (identifiant, total depuis le début de la partie).
    pub hits: Vec<(u32, u32)>,
    /// Étoile en cours de siège.
    pub siege: Option<u32>,
    /// Étoiles prises par siège tout récemment.
    pub taken: Vec<u32>,
    /// Factions déclarées alliées / ennemies (voir `diplomacy::faction_key`).
    pub allies: Vec<String>,
    pub enemies: Vec<String>,
    /// Identifiant permanent du joueur.
    pub pid: u64,
    /// Guilde dont il se dit membre, révision de la fiche qu'il en a, et
    /// guilde qu'il demande à rejoindre (0 = aucune).
    pub gid: u64,
    pub grev: u64,
    pub req: u64,
    /// Fiches de guildes quittées ou dissoutes qu'il garde : (guilde, révision).
    pub archive: Vec<(u64, u64)>,
}

impl Default for PlayerStatus {
    fn default() -> Self {
        Self {
            hp: MAX_HP, hits: Vec::new(), siege: None, taken: Vec::new(), allies: Vec::new(), enemies: Vec::new(),
            pid: 0, gid: 0, grev: 0, req: 0, archive: Vec::new(),
        }
    }
}

/// Combat et sièges du joueur local.
pub struct LocalCombat {
    pub hp: u8,
    pub hits: Vec<(u32, u32)>,
    pub siege: Option<u32>,
    /// Étoiles prises par siège (système, heure de fin de l'annonce).
    pub taken: Vec<(u32, f64)>,
    /// Identifiant permanent du joueur, et guilde qu'il demande à rejoindre (0 = aucune).
    pub pid: u64,
    pub req: u64,
}

impl Default for LocalCombat {
    fn default() -> Self {
        Self { hp: MAX_HP, hits: Vec::new(), siege: None, taken: Vec::new(), pid: 0, req: 0 }
    }
}

/// Message écrit par un client, renvoyé jusqu'à confirmation de l'hôte (UDP).
#[derive(Serialize, Deserialize, Clone, Debug)]
struct ChatOut {
    cseq: u32,
    text: String,
    /// Message réservé à la guilde de l'auteur.
    #[serde(default)]
    guild: bool,
}

/// Message du chat numéroté par l'hôte, diffusé à tous (ou à la guilde `tag`
/// seulement si `guild`).
#[derive(Serialize, Deserialize, Clone, Debug)]
struct ChatLine {
    seq: u64,
    name: String,
    #[serde(default)]
    tag: String,
    color: [f32; 3],
    text: String,
    #[serde(default)]
    guild: bool,
}

impl ChatLine {
    fn visible_to(&self, tag: &str) -> bool {
        !self.guild || (!tag.is_empty() && self.tag == tag)
    }
}

/// Message affiché à l'écran.
pub struct ChatEntry {
    pub name: String,
    pub tag: String,
    pub color: [f32; 3],
    pub text: String,
    /// Message de guilde.
    pub guild: bool,
    /// Message du jeu (pas d'un joueur).
    pub system: bool,
    /// Heure de réception (`Time::elapsed_secs_f64`).
    pub time: f64,
}

pub fn sanitize_chat(text: &str) -> Option<String> {
    let clean: String = text.chars().filter(|c| !c.is_control()).take(MAX_CHAT_LEN).collect();
    let clean = clean.trim();
    (!clean.is_empty()).then(|| clean.to_string())
}

/// Tag de guilde : lettres et chiffres en majuscules, 5 au plus.
pub fn sanitize_tag(tag: &str) -> String {
    tag.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_uppercase())
        .take(MAX_TAG_LEN)
        .collect()
}

/// « [TAG] Pseudo », ou juste le pseudo sans guilde.
pub fn display_name(tag: &str, name: &str) -> String {
    if tag.is_empty() { name.to_string() } else { format!("[{tag}] {name}") }
}

fn push_chat(chat: &mut ChatState, entry: ChatEntry) {
    chat.lines.push_back(entry);
    while chat.lines.len() > CHAT_HISTORY {
        chat.lines.pop_front();
    }
    chat.total += 1;
}

fn system_chat(chat: &mut ChatState, text: &str, now: f64) {
    push_chat(chat, ChatEntry {
        name: String::new(), tag: String::new(), color: [1.0; 3], text: text.into(),
        guild: false, system: true, time: now,
    });
}

fn line_entry(line: ChatLine, now: f64) -> ChatEntry {
    ChatEntry {
        name: line.name, tag: line.tag, color: line.color, text: line.text,
        guild: line.guild, system: false, time: now,
    }
}

/// Enregistre un message côté hôte : numéroté, gardé pour les clients, et
/// affiché chez l'hôte s'il y a droit (`my_tag` = guilde de l'hôte).
fn host_chat(chat: &mut ChatState, line: ChatLine, my_tag: &str, now: f64) {
    chat.host_seq += 1;
    let line = ChatLine { seq: chat.host_seq, ..line };
    chat.host_log.push_back(line.clone());
    while chat.host_log.len() > CHAT_HISTORY {
        chat.host_log.pop_front();
    }
    if line.visible_to(my_tag) {
        push_chat(chat, line_entry(line, now));
    }
}

/// Tout l'état du chat (affichage, envoi côté client, historique côté hôte).
#[derive(Default)]
pub struct ChatState {
    /// Messages affichés, du plus ancien au plus récent.
    pub lines: VecDeque<ChatEntry>,
    /// Nombre total de messages reçus (pour savoir quand rafraîchir l'écran).
    pub total: u64,
    outbox: Vec<ChatOut>,
    next_cseq: u32,
    /// Client : dernier message de l'hôte reçu.
    seen: u64,
    /// Hôte : historique numéroté.
    host_log: VecDeque<ChatLine>,
    host_seq: u64,
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
            // Windows renvoie ConnectionReset en UDP quand un pair a disparu : on ignore.
            Err(e) if e.kind() == ErrorKind::ConnectionReset => continue,
            Err(_) => break,
        }
    }
    out
}

fn sanitize_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME_LEN)
        .collect();
    let clean = clean.trim();
    if clean.is_empty() { "Pilote".into() } else { clean.to_string() }
}

fn sanitize_color(c: [f32; 3]) -> [f32; 3] {
    c.map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 1.0 })
}

fn valid_vec(v: &[f32]) -> bool {
    v.iter().all(|x| x.is_finite() && x.abs() < 1.0e12)
}

/// Empreinte de la carte générée. N'utilise que des valeurs calculées sans
/// trigonométrie (identiques au bit près sur Windows, Linux et macOS).
pub fn world_fingerprint(settings: &GameSettings) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    };
    // Notre galaxie et les extérieures (générées au départ partout) ; les galaxies lointaines, par
    // leur nombre et leur place (leurs systèmes en découlent)
    eat(&(settings.systems.len() as u64).to_le_bytes());
    eat(&(settings.galaxies.len() as u64).to_le_bytes());
    for g in &settings.galaxies {
        eat(&g.abs_center.x.to_bits().to_le_bytes());
        eat(&g.abs_center.z.to_bits().to_le_bytes());
    }
    for sys in settings.systems.dense() {
        eat(sys.name.as_bytes());
        eat(&(sys.stars.len() as u64).to_le_bytes());
        for st in &sys.stars {
            // Rayon arrondi à 10 unités (`StarPhysics::render_radius`) : identique partout
            eat(&[st.class as u8]);
            eat(&st.radius.to_bits().to_le_bytes());
            eat(&st.orbit_distance.to_bits().to_le_bytes());
        }
        // Sans remplir le cache des planètes : on parcourt tous les systèmes
        let planets = sys.planets_uncached();
        eat(&(planets.len() as u64).to_le_bytes());
        for p in planets.iter() {
            // Valeurs affichées arrondies à la génération (`planetgen::system`) : identiques partout
            eat(&[p.kind as u8]);
            eat(&p.seed.to_le_bytes());
            eat(&p.radius.to_bits().to_le_bytes());
            eat(&p.orbit_distance.to_bits().to_le_bytes());
            eat(&(p.moons.len() as u64).to_le_bytes());
        }
    }
    h
}

/// IP locale utilisée pour sortir sur le réseau (aucun paquet n'est envoyé).
fn local_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(8, 8, 8, 8), 80)).ok()?;
    let ip = socket.local_addr().ok()?.ip();
    if ip.is_unspecified() { None } else { Some(ip) }
}

// ─────────────────────────────────────────────────────────────────────────
//  Code d'invitation : adresse Internet de l'hôte encodée en base 32
//  (alphabet de Crockford, sans I, L, O, U pour éviter les confusions).
//  Port standard : 7 caractères « ABC-DEFG » ; autre port : 10 caractères.
// ─────────────────────────────────────────────────────────────────────────

const CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

pub fn encode_invite(ip: Ipv4Addr, port: u16) -> String {
    let (value, len) = if port == NET_PORT {
        (u32::from(ip) as u64, 7)
    } else {
        (((u32::from(ip) as u64) << 16) | port as u64, 10)
    };
    let mut chars: Vec<u8> = (0..len)
        .map(|i| CODE_ALPHABET[((value >> (5 * (len - 1 - i))) & 31) as usize])
        .collect();
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

/// Code d'invitation, ou à défaut une adresse IP / un nom de machine.
fn parse_target(input: &str) -> Option<SocketAddr> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    if let Some(addr) = decode_invite(input) {
        return Some(addr);
    }
    if let Ok(addr) = input.parse::<SocketAddr>() {
        return Some(addr);
    }
    if let Ok(ip) = input.trim_matches(|c| c == '[' || c == ']').parse::<IpAddr>() {
        return Some(SocketAddr::new(ip, NET_PORT));
    }
    if !input.contains('.') && !input.contains(':') {
        return None; // ni code valide, ni adresse : faute de frappe probable
    }
    let resolved: Vec<SocketAddr> = if input.contains(':') {
        input.to_socket_addrs().ok()?.collect()
    } else {
        (input, NET_PORT).to_socket_addrs().ok()?.collect()
    };
    resolved.iter().find(|a| a.is_ipv4()).or_else(|| resolved.first()).copied()
}

// ─────────────────────────────────────────────────────────────────────────
//  Ouverture automatique du port sur la box (UPnP), en arrière-plan
// ─────────────────────────────────────────────────────────────────────────

enum Upnp {
    Pending,
    Ready { ip: Ipv4Addr, port: u16, gateway: igd_next::Gateway },
    #[allow(dead_code)] // message d'échec gardé pour le diagnostic
    Failed(String),
}

fn is_non_public(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || (o[0] == 100 && (64..128).contains(&o[1])) // CGNAT 100.64.0.0/10
}

fn add_mapping(gw: &igd_next::Gateway, port: u16, local: SocketAddr) -> Result<(), igd_next::AddPortError> {
    use igd_next::{AddPortError, PortMappingProtocol};
    match gw.add_port(PortMappingProtocol::UDP, port, local, 3600, "SpaceSpore") {
        Err(AddPortError::OnlyPermanentLeasesSupported) => {
            gw.add_port(PortMappingProtocol::UDP, port, local, 0, "SpaceSpore")
        }
        r => r,
    }
}

fn start_upnp(state: Arc<Mutex<Upnp>>) {
    std::thread::spawn(move || {
        let result = (|| -> Result<(Ipv4Addr, u16, igd_next::Gateway, SocketAddr), String> {
            let local = match local_ip() {
                Some(IpAddr::V4(ip)) => ip,
                _ => return Err("Pas de connexion reseau.".into()),
            };
            // Recherche de la box depuis la bonne carte réseau (important sous
            // Windows avec VPN / cartes virtuelles), puis sur toutes à défaut.
            let search = |bind: SocketAddr| {
                igd_next::search_gateway(igd_next::SearchOptions {
                    bind_addr: bind,
                    timeout: Some(Duration::from_secs(8)),
                    ..Default::default()
                })
            };
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
            let local_addr = SocketAddr::new(IpAddr::V4(local), NET_PORT);
            // Si un autre PC de la maison utilise déjà le port, on en prend un voisin.
            for port in NET_PORT..NET_PORT + 10 {
                if add_mapping(&gw, port, local_addr).is_ok() {
                    return Ok((ext, port, gw, local_addr));
                }
            }
            Err("La box a refuse d'ouvrir le port (UPnP limite sur la box ?).".into())
        })();

        match result {
            Ok((ip, port, gateway, local_addr)) => {
                if let Ok(mut s) = state.lock() {
                    *s = Upnp::Ready { ip, port, gateway: gateway.clone() };
                }
                // Renouvelle l'ouverture tant que le jeu tourne
                loop {
                    std::thread::sleep(Duration::from_secs(30 * 60));
                    let _ = add_mapping(&gateway, port, local_addr);
                }
            }
            Err(reason) => {
                if let Ok(mut s) = state.lock() {
                    *s = Upnp::Failed(reason);
                }
            }
        }
    });
}

// ─────────────────────────────────────────────────────────────────────────
//  STUN : découverte de l'IP publique (fallback quand UPnP échoue)
//
//  Envoie un Binding Request à un serveur STUN public (Google). La réponse
//  contient l'adresse IP:port vue depuis Internet. On utilise l'IP obtenue
//  + le port du jeu (NET_PORT) pour générer le code d'invitation.
//  Fonctionne sur la majorité des box (Full Cone / Restricted Cone NAT).
// ─────────────────────────────────────────────────────────────────────────

enum StunResult {
    Pending,
    Ready(Ipv4Addr),
    Failed,
}

fn start_stun(state: Arc<Mutex<StunResult>>) {
    std::thread::spawn(move || {
        let result = (|| -> Option<Ipv4Addr> {
            let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
            sock.set_read_timeout(Some(Duration::from_secs(3))).ok()?;

            let stun_servers = [
                "stun.l.google.com:19302",
                "stun1.l.google.com:19302",
                "stun2.l.google.com:19302",
            ];

            for server in &stun_servers {
                if let Some(ip) = stun_query(&sock, server) {
                    return Some(ip);
                }
            }
            None
        })();

        if let Ok(mut s) = state.lock() {
            match result {
                Some(ip) if !is_non_public(ip) => *s = StunResult::Ready(ip),
                _ => *s = StunResult::Failed,
            }
        }
    });
}

fn stun_query(sock: &UdpSocket, server: &str) -> Option<Ipv4Addr> {
    let addr = server.to_socket_addrs().ok()?.find(|a| a.is_ipv4())?;

    // STUN Binding Request: type 0x0001, length 0, magic cookie, random txn id
    let mut req = [0u8; 20];
    req[0] = 0x00; req[1] = 0x01; // Binding Request
    // req[2..4] = 0 (length)
    req[4] = 0x21; req[5] = 0x12; req[6] = 0xA4; req[7] = 0x42; // Magic Cookie
    for b in &mut req[8..20] { *b = rand::random(); }

    sock.send_to(&req, addr).ok()?;

    let mut buf = [0u8; 256];
    let n = sock.recv(&mut buf).ok()?;
    if n < 20 { return None; }

    // Vérifier que c'est un Binding Response (0x0101)
    if buf[0] != 0x01 || buf[1] != 0x01 { return None; }

    // Parser les attributs pour trouver XOR-MAPPED-ADDRESS (0x0020) ou MAPPED-ADDRESS (0x0001)
    let msg_len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
    let end = (20 + msg_len).min(n);
    let mut i = 20;
    while i + 4 <= end {
        let attr_type = u16::from_be_bytes([buf[i], buf[i + 1]]);
        let attr_len = u16::from_be_bytes([buf[i + 2], buf[i + 3]]) as usize;
        let attr_start = i + 4;

        if attr_type == 0x0020 && attr_len >= 8 && attr_start + attr_len <= end {
            // XOR-MAPPED-ADDRESS: family at +1, port at +2..4, ip at +4..8
            if buf[attr_start + 1] == 0x01 { // IPv4
                let ip_bytes = [
                    buf[attr_start + 4] ^ 0x21,
                    buf[attr_start + 5] ^ 0x12,
                    buf[attr_start + 6] ^ 0xA4,
                    buf[attr_start + 7] ^ 0x42,
                ];
                return Some(Ipv4Addr::from(ip_bytes));
            }
        }

        if attr_type == 0x0001 && attr_len >= 8 && attr_start + attr_len <= end {
            // MAPPED-ADDRESS (non-XOR)
            if buf[attr_start + 1] == 0x01 {
                let ip_bytes = [buf[attr_start + 4], buf[attr_start + 5], buf[attr_start + 6], buf[attr_start + 7]];
                return Some(Ipv4Addr::from(ip_bytes));
            }
        }

        i = attr_start + ((attr_len + 3) & !3); // padding to 4-byte boundary
    }
    None
}

// ─────────────────────────────────────────────────────────────────────────
//  État réseau
// ─────────────────────────────────────────────────────────────────────────

struct ClientSlot {
    id: u32,
    last_seen: f64,
    /// Dernier message de chat de ce client déjà enregistré.
    last_cseq: u32,
    /// Dernier message de l'hôte que ce client a reçu.
    chat_seen: u64,
    /// Empreinte du profil reçu de ce client (0 = pas encore reçu).
    pack: u64,
    /// Profils que ce client a demandés.
    send_profiles: Vec<u64>,
    /// Le code d'invitation lui a déjà été envoyé.
    sent_code: bool,
}

enum Session {
    Offline,
    Hosting {
        socket: UdpSocket,
        clients: HashMap<SocketAddr, ClientSlot>,
        next_id: u32,
    },
    Joining {
        socket: UdpSocket,
        host: SocketAddr,
        started: f64,
        last_hello: f64,
        manual: bool,
    },
    Connected {
        socket: UdpSocket,
        host: SocketAddr,
        my_id: u32,
        last_recv: f64,
        manual: bool,
        host_code: Option<String>,
        /// Empreinte du profil que l'hôte a de moi (0 = aucun).
        host_pack: u64,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NetMode {
    Offline,
    Hosting,
    Joining,
    Connected,
}

/// État du code d'invitation affiché au joueur.
pub enum Invite {
    Pending,
    Ready(String),
    Unavailable(String),
}

/// Un autre joueur, tel qu'affiché localement.
pub struct Peer {
    pub name: String,
    /// Tag et identifiant de sa guilde, confirmés par la fiche de la guilde
    /// (vide / 0 = sans guilde). Tenus à jour par `guild::guild_sync`.
    pub tag: String,
    pub gid: u64,
    /// Empreinte du profil appliqué à ce joueur.
    ph: u64,
    /// Systèmes stellaires revendiqués par ce joueur.
    pub claims: Vec<u32>,
    /// Combat, sièges et diplomatie annoncés par ce joueur.
    pub status: PlayerStatus,
    pub color: [f32; 3],
    /// Position absolue.
    abs: DVec3,
    rot: Quat,
    vel: Vec3,
    last_update: f64,
    sys: Option<u32>,
    stay: f64,
    clock: f64,
    speed: f64,
    /// Ses modèles et ce qu'ils font (E7).
    pub look: crate::net_models::Looks,
}

fn one() -> f64 {
    1.0
}

fn is_one(x: &f64) -> bool {
    *x == 1.0
}

impl Peer {
    /// Dernière position connue du vaisseau, dans le repère monde (origine flottante).
    pub fn pos(&self) -> Vec3 {
        (self.abs - crate::settings::origin()).as_vec3()
    }

    /// État actuel estimé (durée et horloge avancées depuis la réception).
    fn state_now(&self, id: u32, now: f64) -> PlayerState {
        let age = (now - self.last_update).max(0.0);
        PlayerState {
            id,
            ph: self.ph,
            pos: self.abs.to_array(),
            rot: self.rot.to_array(),
            sys: self.sys,
            stay: self.stay + age,
            clock: self.clock + age * self.speed,
            speed: self.speed,
            hp: self.status.hp,
            hits: self.status.hits.clone(),
            siege: self.status.siege,
            taken: self.status.taken.clone(),
            look: self.look.clone(),
        }
    }
}

/// Partie trouvée sur le réseau local.
#[derive(Clone)]
struct LanHost {
    addr: SocketAddr,
    sid: u64,
    players: usize,
    game: String,
    world: u64,
    seen: f64,
}

#[derive(Resource)]
pub struct Net {
    session: Session,
    /// Dernier message à afficher (erreur de connexion, départ de l'hôte…).
    pub notice: String,
    pub notice_is_error: bool,
    pub peers: HashMap<u32, Peer>,
    /// Identifiant aléatoire de cette instance du jeu.
    sid: u64,
    world: Option<u64>,
    lan: Vec<LanHost>,
    avoid: HashMap<SocketAddr, f64>,
    discover_socket: Option<UdpSocket>,
    last_discover: f64,
    next_host_try: f64,
    last_send: f64,
    upnp: Arc<Mutex<Upnp>>,
    upnp_started: bool,
    /// Le multijoueur a été activé (panneau ouvert au moins une fois).
    enabled: bool,
    stun: Arc<Mutex<StunResult>>,
    stun_started: bool,
    my_sys: Option<u32>,
    sys_since: f64,
    pub chat: ChatState,
    /// Combat et sièges du joueur local (annoncés aux autres).
    pub local: LocalCombat,
    /// Change à chaque changement de partie : les identifiants des joueurs
    /// ne sont valables que pour une même valeur.
    pub epoch: u64,
    /// Fiches de guilde à annoncer / reçues (traitées par `guild::guild_sync`).
    pub guild_outbox: Vec<crate::guild::GuildRecord>,
    pub guild_inbox: Vec<crate::guild::GuildRecord>,
    /// Cellules voxel modifiées ici, à envoyer ; reçues d'ailleurs, à appliquer (`meteors.rs`).
    pub voxel_outbox: Vec<crate::voxel::VoxelEdit>,
    pub voxel_inbox: Vec<crate::voxel::VoxelEdit>,
    /// (hôte) Modifications reçues d'un client, à renvoyer à tous.
    voxel_relay: Vec<crate::voxel::VoxelEdit>,
    /// Fichiers de modèles en route (E7).
    transfers: crate::net_models::Transfers,
    /// Mon amarrage dans le hangar d'un autre joueur (E7).
    pub dock: Option<crate::net_models::DockState>,
    /// Modèles que je partage (empreinte, nom), à envoyer (E8).
    pub share_out: Vec<(u64, String)>,
    /// Modèles partagés par les autres, en route : (empreinte, nom, auteur, depuis).
    shares_in: Vec<(u64, String, String, f64)>,
    /// Hôte : qui a partagé quel fichier (pour le lui demander).
    share_owner: HashMap<u64, u32>,
    /// Fiches de guilde qui me manquent (une version plus récente est annoncée).
    pub guild_want: Vec<u64>,
    /// Fiches de guilde qu'un autre joueur me demande.
    pub guild_asked: Vec<u64>,
    /// Profils connus (les miens, ceux reçus, ceux relus du disque).
    pub profiles: ProfileCache,
    profile_want: Vec<u64>,
    /// Un profil a été reçu : le cache sur disque est à réécrire.
    pub cache_dirty: bool,
    last_want: f64,
    last_code: f64,
}

impl Default for Net {
    fn default() -> Self {
        Self {
            session: Session::Offline,
            notice: String::new(),
            notice_is_error: false,
            peers: HashMap::new(),
            sid: rand::random::<u64>(),
            world: None,
            lan: Vec::new(),
            avoid: HashMap::new(),
            discover_socket: None,
            last_discover: f64::NEG_INFINITY,
            next_host_try: 0.0,
            last_send: f64::NEG_INFINITY,
            upnp: Arc::new(Mutex::new(Upnp::Pending)),
            upnp_started: false,
            enabled: false,
            stun: Arc::new(Mutex::new(StunResult::Pending)),
            stun_started: false,
            my_sys: None,
            sys_since: 0.0,
            chat: ChatState::default(),
            local: LocalCombat::default(),
            epoch: 0,
            guild_outbox: Vec::new(),
            guild_inbox: Vec::new(),
            voxel_outbox: Vec::new(),
            voxel_inbox: Vec::new(),
            voxel_relay: Vec::new(),
            transfers: Default::default(),
            dock: None,
            share_out: Vec::new(),
            shares_in: Vec::new(),
            share_owner: HashMap::new(),
            guild_want: Vec::new(),
            guild_asked: Vec::new(),
            profiles: ProfileCache::default(),
            profile_want: Vec::new(),
            cache_dirty: false,
            last_want: f64::NEG_INFINITY,
            last_code: f64::NEG_INFINITY,
        }
    }
}

impl Net {
    /// Seul l'hôte (ou un joueur seul) règle la vitesse du temps.
    pub fn may_set_clock(&self) -> bool {
        matches!(self.session, Session::Offline | Session::Hosting { .. })
    }

    pub fn mode(&self) -> NetMode {
        match self.session {
            Session::Offline => NetMode::Offline,
            Session::Hosting { .. } => NetMode::Hosting,
            Session::Joining { .. } => NetMode::Joining,
            Session::Connected { .. } => NetMode::Connected,
        }
    }

    /// Nombre de joueurs dans la partie, soi compris.
    pub fn player_count(&self) -> usize {
        self.peers.len() + 1
    }

    /// Vrai si on a rejoint un ami avec son code (pas le regroupement automatique).
    pub fn joined_by_code(&self) -> bool {
        matches!(
            self.session,
            Session::Joining { manual: true, .. } | Session::Connected { manual: true, .. }
        )
    }

    fn public_address(&self) -> Option<(Ipv4Addr, u16)> {
        if let Ok(Upnp::Ready { ip, port, .. }) = self.upnp.lock().as_deref() {
            return Some((*ip, *port));
        }
        if let Ok(StunResult::Ready(ip)) = self.stun.lock().as_deref() {
            return Some((*ip, NET_PORT));
        }
        None
    }

    fn public_pending(&self) -> bool {
        matches!(self.upnp.lock().as_deref(), Ok(Upnp::Pending))
            || matches!(self.stun.lock().as_deref(), Ok(StunResult::Pending))
    }

    /// Code à donner à un ami pour qu'il rejoigne la partie en cours.
    pub fn invite(&self) -> Invite {
        match &self.session {
            Session::Connected { host_code: Some(code), .. } => Invite::Ready(code.clone()),
            Session::Connected { .. } => {
                Invite::Unavailable("La box de l'hote n'accepte pas les joueurs venant d'Internet.".into())
            }
            Session::Hosting { .. } => {
                if let Some((ip, port)) = self.public_address() {
                    Invite::Ready(encode_invite(ip, port))
                } else if self.public_pending() {
                    Invite::Pending
                } else {
                    Invite::Unavailable("Impossible de determiner votre adresse Internet.".into())
                }
            }
            _ => Invite::Pending,
        }
    }

    fn own_code(&self) -> Option<String> {
        self.public_address().map(|(ip, port)| encode_invite(ip, port))
    }

    /// Identifiant du joueur local (0 = hôte ou hors ligne).
    /// Tests (développement) : un faux joueur à la position `pos` (monde), avec cette allure.
    pub fn test_peer(&mut self, id: u32, name: &str, pos: Vec3, rot: Quat, look: crate::net_models::Looks) {
        self.peers.insert(id, Peer {
            name: name.into(), tag: String::new(), gid: 0, ph: 0, claims: Vec::new(),
            status: PlayerStatus::default(), color: [1.0, 0.6, 0.2],
            abs: crate::settings::to_abs(pos), rot, vel: Vec3::ZERO, last_update: f64::MAX, sys: None, stay: 0.0, clock: 0.0, speed: 1.0,
            look,
        });
    }

    pub fn my_id(&self) -> u32 {
        match self.session {
            Session::Connected { my_id, .. } => my_id,
            _ => HOST_ID,
        }
    }

    /// Décalage du point de stationnement du vaisseau, pour que deux joueurs
    /// posés sur le même astre ne se superposent pas.
    pub fn hover_offset(&self, zoom_distance: f32) -> Vec3 {
        let id = self.my_id();
        if id == HOST_ID {
            return Vec3::ZERO;
        }
        let angle = id as f32 * 2.399_963; // angle d'or : bonne répartition
        Vec3::new(angle.cos(), 0.0, angle.sin()) * zoom_distance.max(1.0) * 0.06
    }

    /// Active le multijoueur (ouverture du panneau). Relance aussi
    /// l'ouverture automatique de la box si elle avait échoué.
    pub fn enable(&mut self) {
        self.enabled = true;
        if let Ok(mut state) = self.upnp.lock() {
            if matches!(*state, Upnp::Failed(_)) {
                *state = Upnp::Pending;
                self.upnp_started = false;
            }
        }
    }

    /// Le multijoueur est actif (panneau ouvert au moins une fois).
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Met un message de chat en file d'envoi. « /g message » : guilde seulement.
    fn queue_chat(&mut self, text: &str, my_tag: &str, now: f64) {
        let trimmed = text.trim_start();
        let (guild, body) = match trimmed.strip_prefix("/g") {
            Some(rest) if rest.is_empty() || rest.starts_with(' ') => (true, rest),
            _ => (false, trimmed),
        };
        let Some(text) = sanitize_chat(body) else { return };
        if guild && my_tag.is_empty() {
            system_chat(&mut self.chat, "Vous n'avez pas de guilde : choisissez un tag dans le panneau Multijoueur (F2).", now);
            return;
        }
        if self.chat.outbox.len() >= CHAT_OUTBOX_MAX {
            return;
        }
        self.chat.next_cseq += 1;
        let cseq = self.chat.next_cseq;
        self.chat.outbox.push(ChatOut { cseq, text, guild });
    }

    /// Affiche un message du jeu dans le chat.
    pub fn notify(&mut self, text: &str, now: f64) {
        if std::env::var_os("SPACESPORE_TEST_CMD").is_some() {
            info!("NOTIFY {text}");
        }
        system_chat(&mut self.chat, text, now);
    }

    fn set_notice(&mut self, text: impl Into<String>, error: bool) {
        self.notice = text.into();
        self.notice_is_error = error;
    }

    fn leave(&mut self) {
        match &self.session {
            Session::Hosting { socket, clients, .. } => {
                for addr in clients.keys() {
                    send(socket, *addr, &Msg::Bye);
                }
            }
            Session::Connected { socket, host, .. } | Session::Joining { socket, host, .. } => {
                send(socket, *host, &Msg::Bye);
            }
            Session::Offline => {}
        }
        self.session = Session::Offline;
        self.peers.clear();
        self.epoch += 1;
    }

    fn try_host(&mut self, now: f64) {
        match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, NET_PORT)) {
            Ok(socket) => {
                let _ = socket.set_nonblocking(true);
                self.session = Session::Hosting { socket, clients: HashMap::new(), next_id: 1 };
            }
            // Port déjà pris (ex. deuxième jeu sur ce PC) : on rejoindra via la recherche locale.
            Err(_) => self.next_host_try = now + HOST_RETRY,
        }
    }

    fn start_join(&mut self, host: SocketAddr, manual: bool, now: f64) -> bool {
        self.leave();
        let bind: SocketAddr = if host.is_ipv4() {
            (Ipv4Addr::UNSPECIFIED, 0).into()
        } else {
            (std::net::Ipv6Addr::UNSPECIFIED, 0).into()
        };
        match UdpSocket::bind(bind) {
            Ok(socket) => {
                let _ = socket.set_nonblocking(true);
                self.session = Session::Joining {
                    socket,
                    host,
                    started: now,
                    last_hello: f64::NEG_INFINITY,
                    manual,
                };
                true
            }
            Err(_) => false,
        }
    }
}

/// Choisit la partie locale à rejoindre automatiquement.
/// Hors ligne : la plus peuplée. En hébergeant seul : seulement une partie
/// déjà peuplée, ou à égalité celle de plus petit identifiant (les deux jeux
/// font le même choix, donc ils finissent toujours regroupés).
fn pick_lan_host(lan: &[LanHost], my_sid: u64, hosting_alone: bool, world: u64, avoid: &HashMap<SocketAddr, f64>, now: f64) -> Option<SocketAddr> {
    lan.iter()
        .filter(|h| h.sid != my_sid && h.game == GAME_VERSION && h.world == world && h.players < MAX_PLAYERS)
        .filter(|h| avoid.get(&h.addr).map_or(true, |until| now >= *until))
        .filter(|h| !hosting_alone || h.players > 1 || h.sid < my_sid)
        .max_by(|a, b| a.players.cmp(&b.players).then(b.sid.cmp(&a.sid)))
        .map(|h| h.addr)
}

/// Met à jour un joueur d'après son état. Son profil (pseudo, couleur,
/// étoiles, diplomatie…) est repris du cache quand son empreinte change ;
/// un joueur dont on n'a encore aucun profil n'est pas affiché.
fn update_peer(peers: &mut HashMap<u32, Peer>, profiles: &ProfileCache, st: PlayerState, now: f64) {
    if !st.pos.iter().all(|x| x.is_finite() && x.abs() < 1.0e13) || !valid_vec(&st.rot) {
        return;
    }
    let pos = DVec3::from_array(st.pos);
    let q = Quat::from_array(st.rot);
    let rot = if q.length_squared() > 1.0e-6 { q.normalize() } else { Quat::IDENTITY };
    if !st.clock.is_finite() || !st.stay.is_finite() || !st.speed.is_finite() {
        return;
    }
    let profile = profiles.get(st.ph);
    if !peers.contains_key(&st.id) {
        if profile.is_none() {
            return;
        }
        peers.insert(st.id, Peer {
            name: String::new(), tag: String::new(), gid: 0, ph: 0, claims: Vec::new(),
            status: PlayerStatus::default(), color: [1.0; 3],
            abs: pos, rot, vel: Vec3::ZERO, last_update: now, sys: st.sys, stay: 0.0, clock: 0.0, speed: 1.0,
            look: Default::default(),
        });
    }
    let Some(p) = peers.get_mut(&st.id) else { return };

    let dt = (now - p.last_update) as f32;
    if dt > 0.001 {
        p.vel = (pos - p.abs).as_vec3() / dt;
    }
    p.abs = pos;
    p.rot = rot;
    p.last_update = now;
    p.sys = st.sys;
    p.stay = st.stay.max(0.0);
    p.clock = st.clock;
    p.speed = st.speed;
    p.status.hp = st.hp.min(MAX_HP);
    p.status.hits = st.hits.into_iter().take(MAX_PLAYERS).collect();
    p.status.siege = st.siege;
    p.status.taken = st.taken.into_iter().take(MAX_CLAIMS).collect();
    p.look = st.look.sanitized();

    if let Some(profile) = profile.filter(|_| p.ph != st.ph) {
        p.ph = st.ph;
        p.name = profile.name.clone();
        p.color = profile.color;
        p.claims = profile.claims.clone();
        p.status.allies = profile.allies.clone();
        p.status.enemies = profile.enemies.clone();
        p.status.pid = profile.pid;
        p.status.gid = profile.gid;
        p.status.grev = profile.grev;
        p.status.req = profile.req;
        p.status.archive = profile.archive.clone();
    }
}

// ─────────────────────────────────────────────────────────────────────────
//  Commandes (depuis l'interface)
// ─────────────────────────────────────────────────────────────────────────

#[derive(Event, Clone)]
pub enum NetCommand {
    /// Rejoindre un ami avec son code d'invitation.
    JoinCode(String),
    /// Quitter la partie de l'ami et revenir au mode automatique.
    Leave,
    /// Envoyer un message dans le chat.
    Chat(String),
}

fn handle_net_commands(
    mut events: EventReader<NetCommand>,
    mut net: ResMut<Net>,
    settings: Res<GameSettings>,
    time: Res<Time>,
) {
    for ev in events.read() {
        let now = time.elapsed_secs_f64();
        match ev {
            NetCommand::JoinCode(code) => {
                net.enable();
                let Some(host) = parse_target(code) else {
                    net.set_notice(format!("Code invalide : \"{}\"", code.trim()), true);
                    continue;
                };
                if net.start_join(host, true, now) {
                    net.set_notice("Connexion a votre ami...", false);
                } else {
                    net.set_notice("Erreur reseau.", true);
                }
            }
            NetCommand::Leave => {
                net.leave();
                net.next_host_try = now;
                net.set_notice("", false);
            }
            NetCommand::Chat(text) => net.queue_chat(text, &sanitize_tag(&settings.clan_tag), now),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
//  Boucle réseau
// ─────────────────────────────────────────────────────────────────────────

pub(crate) fn net_update(
    time: Res<Time>,
    settings: Res<GameSettings>,
    ship_q: Query<&GlobalTransform, With<Ship>>,
    spawned: Option<Res<SpawnedSystems>>,
    mut clock: ResMut<crate::world_clock::WorldClock>,
    mut net: ResMut<Net>,
    mut models: ResMut<crate::models::GameModels>,
    surface: Res<crate::surface::Surface>,
) {
    if !net.enabled {
        return; // aucun port ouvert avant l'ouverture du panneau Multijoueur
    }
    let now = time.elapsed_secs_f64();
    let (pos, rot) = ship_q
        .get_single()
        .map(|gt| {
            let (_, r, t) = gt.to_scale_rotation_translation();
            (t, r)
        })
        .unwrap_or((Vec3::ZERO, Quat::IDENTITY));
    let my_name = sanitize_name(&settings.player_name);
    let my_color = sanitize_color(settings.aura_color);
    let my_tag = sanitize_tag(&settings.clan_tag);
    let (allies, enemies) = crate::diplomacy::declared_lists(&settings);

    let net = &mut *net;
    let world = *net.world.get_or_insert_with(|| world_fingerprint(&settings));

    // Système stellaire actuel (celui chargé autour du vaisseau)
    let sys = spawned.and_then(|s| s.0.iter().next().map(|&i| i as u32));
    if sys != net.my_sys {
        net.my_sys = sys;
        net.sys_since = now;
    }
    // Mon profil : n'est transmis que si son empreinte est inconnue en face
    let my_profile = Profile {
        pid: net.local.pid,
        name: my_name.clone(),
        color: my_color,
        claims: settings.claims.clone(),
        allies,
        enemies,
        gid: crate::diplomacy::my_gid(&settings),
        grev: settings.guild.as_ref().map_or(0, |g| g.rev),
        req: net.local.req,
        archive: settings.guild_archive.iter().map(|g| (g.id, g.rev)).collect(),
    }
    .sanitized();
    let my_ph = my_profile.fingerprint();
    if net.profiles.get(my_ph).is_none() {
        net.profiles.insert(my_profile.clone());
    }
    let me = PlayerState {
        id: net.my_id(),
        ph: my_ph,
        hp: net.local.hp,
        hits: net.local.hits.clone(),
        siege: net.local.siege,
        taken: net.local.taken.iter().map(|t| t.0).collect(),
        pos: crate::settings::to_abs(pos).to_array(),
        rot: rot.to_array(),
        sys,
        stay: now - net.sys_since,
        clock: clock.secs,
        speed: clock.speed,
        look: crate::net_models::Looks {
            ship: Some(models.ship.clone()).filter(|k| *k != crate::models::ModelKey::default_ship()),
            chr: Some(models.character.clone()).filter(|k| *k != crate::models::ModelKey::default_character()),
            ss: surface.ship_state().to_string(),
            walk: surface.walker_state().and_then(|(b, t, a)| crate::net_models::WalkState::new(b, t, a)),
            dock: net.dock,
            heat: (surface.flight().heat * 255.0) as u8,
            mach: (surface.flight().mach * 20.0).clamp(0.0, 255.0) as u8,
        },
    };
    // Modèles des autres joueurs qui nous manquent : à demander (règle 7)
    for fp in models.missing_prints() {
        net.transfers.want(fp, now);
    }
    // Modèles partagés (E8) : arrivés, ils vont dans la bibliothèque (`saves/modeles/partages/`)
    let mut arrived = Vec::new();
    net.shares_in.retain(|(fp, name, from, since)| {
        if let Some(bytes) = models.file(*fp) {
            arrived.push((bytes, name.clone(), from.clone()));
            return false;
        }
        now - since < 300.0
    });
    for (fp, ..) in net.shares_in.clone() {
        net.transfers.want(fp, now);
    }
    for (bytes, name, from) in arrived {
        let dir = crate::editeur::library_dir().join("partages");
        let r = crate::editeur::format::Model::from_bytes(&bytes).and_then(|mut m| {
            m.name = format!("{name} (de {from})");
            crate::editeur::save_model(&dir, &m)
        });
        match r {
            Ok(_) => net.notify(&format!("{from} a partage \"{name}\" : il est dans votre bibliotheque (editeur)."), now),
            Err(e) => net.notify(&format!("Modele partage illisible : {e}"), now),
        }
    }
    if !net.upnp_started {
        net.upnp_started = true;
        start_upnp(net.upnp.clone());
    }
    if !net.stun_started {
        net.stun_started = true;
        start_stun(net.stun.clone());
    }

    // ── Mode automatique : toujours dans une partie ─────────────────────
    if net.mode() == NetMode::Offline && now >= net.next_host_try {
        net.try_host(now);
    }
    if let Some(target) = discovery(net, now, world) {
        net.start_join(target, false, now);
    }

    let tick = now - net.last_send >= SEND_INTERVAL;
    if tick {
        net.last_send = now;
    }
    let wants_due = tick && now - net.last_want >= WANT_INTERVAL;
    if wants_due {
        net.last_want = now;
    }

    // Hôte (ou seul) : ses propres messages sont enregistrés directement.
    // Client : ils partent avec son état jusqu'à confirmation de l'hôte.
    if matches!(net.mode(), NetMode::Hosting | NetMode::Offline) {
        for out in std::mem::take(&mut net.chat.outbox) {
            let line = ChatLine {
                seq: 0, name: my_name.clone(), tag: my_tag.clone(), color: my_color,
                text: out.text, guild: out.guild,
            };
            host_chat(&mut net.chat, line, &my_tag, now);
        }
    }

    let own_code = net.own_code();
    let sid = net.sid;
    // (nouvelle session, message, erreur ?, adresse à éviter)
    let mut next: Option<(Session, String, bool, Option<SocketAddr>)> = None;

    match &mut net.session {
        Session::Offline => {}

        Session::Hosting { socket, clients, next_id } => {
            for (addr, msg) in recv_all(socket) {
                match msg {
                    Msg::Discover { magic } if magic == MAGIC => {
                        send(socket, addr, &Msg::HostInfo {
                            magic: MAGIC.into(),
                            sid,
                            players: clients.len() + 1,
                            game: GAME_VERSION.into(),
                            world,
                        });
                    }
                    Msg::Hello { magic, proto, game, world: their_world, .. } if magic == MAGIC => {
                        if let Some(slot) = clients.get(&addr) {
                            send(socket, addr, &Msg::Welcome { id: slot.id });
                            continue;
                        }
                        let reject = if proto != PROTOCOL || game != GAME_VERSION {
                            Some(format!(
                                "Versions differentes : votre ami a la v{GAME_VERSION}, vous avez la v{game}. Mettez le jeu a jour."
                            ))
                        } else if their_world != world {
                            Some("Votre carte est differente de celle de votre ami (modifiee dans les options ?).".into())
                        } else if clients.len() + 1 >= MAX_PLAYERS {
                            Some(format!("Partie pleine ({MAX_PLAYERS} joueurs max)."))
                        } else {
                            None
                        };
                        match reject {
                            Some(reason) => send(socket, addr, &Msg::Reject { reason }),
                            None => {
                                let id = *next_id;
                                *next_id += 1;
                                clients.insert(addr, ClientSlot { id, last_seen: now, last_cseq: 0, chat_seen: 0, pack: 0, send_profiles: Vec::new(), sent_code: false });
                                send(socket, addr, &Msg::Welcome { id });
                            }
                        }
                    }
                    Msg::State { state, mut chat, seen, profile, want, gwant } => {
                        if let Some(slot) = clients.get_mut(&addr) {
                            slot.last_seen = now;
                            slot.chat_seen = slot.chat_seen.max(seen.min(net.chat.host_seq));
                            // Profil du client, et ce qu'il lui manque
                            if let Some(profile) = profile {
                                net.cache_dirty |= net.profiles.insert(profile);
                            }
                            slot.pack = if net.profiles.get(state.ph).is_some() { state.ph } else { 0 };
                            for h in want.into_iter().take(MAX_PLAYERS) {
                                if !slot.send_profiles.contains(&h) {
                                    slot.send_profiles.push(h);
                                }
                            }
                            for g in gwant.into_iter().take(MAX_PLAYERS) {
                                if !net.guild_asked.contains(&g) {
                                    net.guild_asked.push(g);
                                }
                            }
                            // Pseudo et couleur viennent du profil : sans lui, le chat attend
                            let author = net.profiles.get(state.ph).cloned();
                            chat.sort_by_key(|c| c.cseq);
                            for c in chat {
                                let Some(author) = &author else { break };
                                if c.cseq <= slot.last_cseq {
                                    continue; // déjà reçu (renvoi UDP)
                                }
                                slot.last_cseq = c.cseq;
                                // Guilde confirmée par sa fiche, pas celle que le joueur annonce
                                let tag = net.peers.get(&slot.id).map(|p| p.tag.clone()).unwrap_or_default();
                                if c.guild && tag.is_empty() {
                                    continue; // message de guilde sans guilde
                                }
                                if let Some(text) = sanitize_chat(&c.text) {
                                    let line = ChatLine {
                                        seq: 0, name: author.name.clone(), tag,
                                        color: author.color, text, guild: c.guild,
                                    };
                                    host_chat(&mut net.chat, line, &my_tag, now);
                                }
                            }
                            update_peer(&mut net.peers, &net.profiles, PlayerState { id: slot.id, ..state }, now);
                        }
                    }
                    Msg::Guild { guild } => {
                        if clients.contains_key(&addr) && net.guild_inbox.len() < GUILD_INBOX_MAX {
                            net.guild_inbox.push(guild);
                        }
                    }
                    // Un modèle demandé : je l'ai (je l'envoie) ou je le demande à son joueur
                    Msg::ModelWant { fp, parts } => {
                        if clients.contains_key(&addr) {
                            if models.file(fp).is_some() {
                                net.transfers.serve(addr, fp, &parts);
                            } else {
                                net.transfers.want(fp, now);
                            }
                        }
                    }
                    Msg::ModelPart { fp, index, total, data } => {
                        if clients.contains_key(&addr) {
                            if let Some(bytes) = net.transfers.receive(fp, index, total, &data) {
                                models.add_file(bytes);
                            }
                        }
                    }
                    // Un client partage un modèle : je le note, le prends aussi, et préviens les autres
                    Msg::Share { fp, name, from } => {
                        if let Some(slot) = clients.get(&addr) {
                            let (name, from) = (sanitize_chat(&name).unwrap_or_default(), sanitize_name(&from));
                            net.share_owner.insert(fp, slot.id);
                            if net.shares_in.len() < 16 {
                                net.shares_in.push((fp, name.clone(), from.clone(), now));
                            }
                            let msg = Msg::Share { fp, name, from };
                            for (a, _) in clients.iter().filter(|(a, _)| **a != addr) {
                                send(socket, *a, &msg);
                            }
                        }
                    }
                    // Un client a modifié des voxels : on les applique et on les relaie à tous
                    Msg::Voxels { edits } => {
                        if clients.contains_key(&addr) {
                            for e in edits.into_iter().take(VOXEL_EDITS_MAX) {
                                if net.voxel_inbox.len() < VOXEL_EDITS_MAX {
                                    net.voxel_inbox.push(e.clone());
                                    net.voxel_relay.push(e);
                                }
                            }
                        }
                    }
                    Msg::Bye => {
                        if let Some(slot) = clients.remove(&addr) {
                            net.peers.remove(&slot.id);
                        }
                    }
                    _ => {}
                }
            }

            clients.retain(|_, slot| {
                let alive = now - slot.last_seen < TIMEOUT;
                if !alive {
                    net.peers.remove(&slot.id);
                }
                alive
            });

            // Modèles : je demande ceux qui me manquent à leur joueur, j'envoie ceux demandés
            for (fp, parts) in net.transfers.asks(now) {
                let key = crate::models::ModelKey::Print(fp);
                let shared = net.share_owner.get(&fp).copied();
                let owner = clients.iter().find(|(_, s)| Some(s.id) == shared || net.peers.get(&s.id).is_some_and(|p| p.look.ship.as_ref() == Some(&key) || p.look.chr.as_ref() == Some(&key)));
                if let Some((addr, _)) = owner {
                    send(socket, *addr, &Msg::ModelWant { fp, parts });
                }
            }
            for (to, fp, index) in net.transfers.next_parts(crate::net_models::PARTS_PER_FRAME) {
                if let Some((total, data)) = models.file(fp).and_then(|b| crate::net_models::part_of(&b, index)) {
                    send(socket, to, &Msg::ModelPart { fp, index, total, data });
                }
            }
            // Mes partages : à tous les clients
            for (fp, name) in std::mem::take(&mut net.share_out) {
                let msg = Msg::Share { fp, name, from: my_name.clone() };
                for addr in clients.keys() {
                    send(socket, *addr, &msg);
                }
            }
            // Voxels modifiés par moi ou par un client : à tous les clients
            if tick && (!net.voxel_outbox.is_empty() || !net.voxel_relay.is_empty()) {
                let mut edits: Vec<crate::voxel::VoxelEdit> = net.voxel_outbox.drain(..).collect();
                edits.extend(net.voxel_relay.drain(..));
                for chunk in edits.chunks(8) {
                    let msg = Msg::Voxels { edits: chunk.to_vec() };
                    for addr in clients.keys() {
                        send(socket, *addr, &msg);
                    }
                }
            }
            // Fiches de guilde modifiées par moi ou demandées par un client
            if tick {
                for guild in net.guild_outbox.drain(..) {
                    let msg = Msg::Guild { guild };
                    for addr in clients.keys() {
                        send(socket, *addr, &msg);
                    }
                }
            }

            if tick && !clients.is_empty() {
                let mut players = vec![me.clone()];
                for slot in clients.values() {
                    if let Some(p) = net.peers.get(&slot.id) {
                        players.push(p.state_now(slot.id, now));
                    }
                }
                let remind_code = now - net.last_code >= CODE_INTERVAL;
                if remind_code {
                    net.last_code = now;
                }
                for (addr, slot) in clients.iter_mut() {
                    let peer = net.peers.get(&slot.id);
                    // Profils demandés par ce client, quelques-uns par paquet
                    let count = slot.send_profiles.len().min(PROFILES_PER_PACKET);
                    let profiles: Vec<Profile> = slot.send_profiles.drain(..count)
                        .filter_map(|h| net.profiles.get(h).cloned())
                        .collect();
                    // Fiches de guilde qui me manquent et que ce client annonce avoir
                    let gwant: Vec<u64> = if wants_due {
                        net.guild_want.iter().copied()
                            .filter(|g| peer.is_some_and(|p| {
                                p.status.gid == *g || p.status.archive.iter().any(|a| a.0 == *g)
                            }))
                            .collect()
                    } else {
                        Vec::new()
                    };
                    // Le code d'invitation ne change presque jamais : rappel de temps en temps
                    let code = (remind_code || !slot.sent_code).then(|| own_code.clone().unwrap_or_default());
                    slot.sent_code = true;
                    // Seulement les messages que ce client n'a pas encore
                    // (et pas ceux des autres guildes)
                    let their_tag = peer.map_or("", |p| p.tag.as_str());
                    let mut chat: Vec<ChatLine> = net.chat.host_log.iter()
                        .rev()
                        .filter(|l| l.seq > slot.chat_seen && l.visible_to(their_tag))
                        .take(CHAT_PER_PACKET)
                        .cloned()
                        .collect();
                    chat.reverse();
                    send(socket, *addr, &Msg::Snapshot {
                        code,
                        players: players.clone(),
                        chat,
                        ack: slot.last_cseq,
                        pack: slot.pack,
                        profiles,
                        gwant,
                    });
                }
            }
        }

        Session::Joining { socket, host, started, last_hello, manual } => {
            if now - *last_hello >= JOIN_RETRY {
                *last_hello = now;
                send(socket, *host, &Msg::Hello {
                    magic: MAGIC.into(),
                    proto: PROTOCOL,
                    game: GAME_VERSION.into(),
                    world,
                    name: my_name.clone(),
                    color: my_color,
                });
            }
            for (addr, msg) in recv_all(socket) {
                if addr != *host {
                    continue;
                }
                match msg {
                    Msg::Welcome { id } => {
                        if let Ok(socket) = socket.try_clone() {
                            let text = if *manual { "Vous avez rejoint votre ami !" } else { "" };
                            next = Some((
                                Session::Connected {
                                    socket,
                                    host: *host,
                                    my_id: id,
                                    last_recv: now,
                                    manual: *manual,
                                    host_code: None,
                                    host_pack: 0,
                                },
                                text.into(),
                                false,
                                None,
                            ));
                        }
                        break;
                    }
                    Msg::Reject { reason } => {
                        let text = if *manual { format!("Refuse : {reason}") } else { String::new() };
                        next = Some((Session::Offline, text, *manual, Some(*host)));
                        break;
                    }
                    _ => {}
                }
            }
            if next.is_none() && now - *started > JOIN_TIMEOUT {
                let text = if *manual {
                    "Aucune reponse de votre ami. Verifiez le code et que son jeu est bien lance.".into()
                } else {
                    String::new()
                };
                next = Some((Session::Offline, text, *manual, Some(*host)));
            }
        }

        Session::Connected { socket, host, my_id, last_recv, host_code, host_pack, .. } => {
            for (addr, msg) in recv_all(socket) {
                if addr != *host {
                    continue;
                }
                match msg {
                    Msg::Snapshot { code, players, chat, ack, pack, profiles, gwant } => {
                        *last_recv = now;
                        *host_pack = pack;
                        for profile in profiles.into_iter().take(PROFILES_PER_PACKET) {
                            net.cache_dirty |= net.profiles.insert(profile);
                        }
                        for g in gwant.into_iter().take(MAX_PLAYERS) {
                            if !net.guild_asked.contains(&g) {
                                net.guild_asked.push(g);
                            }
                        }
                        net.chat.outbox.retain(|c| c.cseq > ack);
                        for line in chat {
                            if line.seq <= net.chat.seen {
                                continue;
                            }
                            net.chat.seen = line.seq;
                            if !line.visible_to(&my_tag) {
                                continue;
                            }
                            if let Some(text) = sanitize_chat(&line.text) {
                                let line = ChatLine {
                                    name: sanitize_name(&line.name),
                                    tag: sanitize_tag(&line.tag),
                                    color: sanitize_color(line.color),
                                    text,
                                    ..line
                                };
                                push_chat(&mut net.chat, line_entry(line, now));
                            }
                        }
                        // Code d'invitation : absent du paquet = inchangé
                        if let Some(code) = code {
                            *host_code = Some(code).filter(|c| decode_invite(c).is_some());
                        }
                        let ids: Vec<u32> = players.iter().map(|p| p.id).filter(|id| id != my_id).collect();
                        net.peers.retain(|id, _| ids.contains(id));
                        net.profile_want.clear();
                        for st in players.into_iter().take(MAX_PLAYERS) {
                            if st.id == *my_id {
                                continue;
                            }
                            // Profil inconnu : à demander à l'hôte
                            if net.profiles.get(st.ph).is_none() && !net.profile_want.contains(&st.ph) {
                                net.profile_want.push(st.ph);
                            }
                            update_peer(&mut net.peers, &net.profiles, st, now);
                        }
                    }
                    Msg::Welcome { .. } => *last_recv = now,
                    Msg::Guild { guild } => {
                        if net.guild_inbox.len() < GUILD_INBOX_MAX {
                            net.guild_inbox.push(guild);
                        }
                    }
                    Msg::Voxels { edits } => {
                        for e in edits.into_iter().take(VOXEL_EDITS_MAX) {
                            if net.voxel_inbox.len() < VOXEL_EDITS_MAX {
                                net.voxel_inbox.push(e);
                            }
                        }
                    }
                    // L'hôte demande un de mes modèles, ou m'en envoie un morceau
                    Msg::ModelWant { fp, parts } => {
                        if models.file(fp).is_some() {
                            net.transfers.serve(*host, fp, &parts);
                        }
                    }
                    Msg::ModelPart { fp, index, total, data } => {
                        if let Some(bytes) = net.transfers.receive(fp, index, total, &data) {
                            models.add_file(bytes);
                        }
                    }
                    Msg::Share { fp, name, from } => {
                        if net.shares_in.len() < 16 && !net.shares_in.iter().any(|s| s.0 == fp) {
                            net.shares_in.push((fp, sanitize_chat(&name).unwrap_or_default(), sanitize_name(&from), now));
                        }
                    }
                    Msg::Bye => {
                        next = Some((Session::Offline, "L'hote a quitte la partie.".into(), false, None));
                        break;
                    }
                    Msg::Reject { reason } => {
                        next = Some((Session::Offline, format!("Deconnecte : {reason}"), true, None));
                        break;
                    }
                    _ => {}
                }
            }
            if next.is_none() {
                for (fp, name) in std::mem::take(&mut net.share_out) {
                    send(socket, *host, &Msg::Share { fp, name, from: my_name.clone() });
                }
                for (fp, parts) in net.transfers.asks(now) {
                    send(socket, *host, &Msg::ModelWant { fp, parts });
                }
                for (to, fp, index) in net.transfers.next_parts(crate::net_models::PARTS_PER_FRAME) {
                    if let Some((total, data)) = models.file(fp).and_then(|b| crate::net_models::part_of(&b, index)) {
                        send(socket, to, &Msg::ModelPart { fp, index, total, data });
                    }
                }
                if now - *last_recv > TIMEOUT {
                    next = Some((Session::Offline, "Connexion perdue avec l'hote.".into(), true, None));
                } else if tick {
                    let chat = net.chat.outbox.iter().take(CHAT_PER_PACKET).cloned().collect();
                    // Mon profil part tant que l'hôte ne l'a pas confirmé ; les demandes, de temps en temps
                    let profile = (*host_pack != my_ph).then(|| my_profile.clone());
                    let want = if wants_due { net.profile_want.clone() } else { Vec::new() };
                    let gwant = if wants_due { net.guild_want.clone() } else { Vec::new() };
                    send(socket, *host, &Msg::State { state: me.clone(), chat, seen: net.chat.seen, profile, want, gwant });
                    for guild in net.guild_outbox.drain(..) {
                        send(socket, *host, &Msg::Guild { guild });
                    }
                    let edits: Vec<crate::voxel::VoxelEdit> = net.voxel_outbox.drain(..).collect();
                    for chunk in edits.chunks(8) {
                        send(socket, *host, &Msg::Voxels { edits: chunk.to_vec() });
                    }
                }
            }
        }
    }

    if let Some((session, text, error, avoid)) = next {
        let offline = matches!(session, Session::Offline);
        if matches!(session, Session::Connected { .. }) {
            // Nouvel hôte : sa numérotation des messages repart de zéro
            net.chat.seen = 0;
        }
        net.session = session;
        net.epoch += 1;
        if offline {
            net.peers.clear();
            net.next_host_try = now; // on rouvre aussitôt sa propre partie
        }
        if let Some(addr) = avoid {
            net.avoid.insert(addr, now + AVOID_DURATION);
        }
        if !text.is_empty() || !error {
            net.set_notice(text, error);
        }
    }

    follow_host_clock(net, &mut clock, now);
}

/// Règle 9 : l'horloge du monde est celle de l'hôte (orbites, rotation, saisons identiques chez
/// tous). L'hôte et le joueur seul gardent la leur.
fn follow_host_clock(net: &Net, clock: &mut crate::world_clock::WorldClock, now: f64) {
    if net.mode() != NetMode::Connected {
        return;
    }
    if let Some(host) = net.peers.get(&HOST_ID) {
        let st = host.state_now(HOST_ID, now);
        clock.follow(st.clock, st.speed);
    }
}

/// Recherche des parties sur le réseau local par broadcast UDP.
/// Renvoie la partie à rejoindre automatiquement, s'il y en a une.
fn discovery(net: &mut Net, now: f64, world: u64) -> Option<SocketAddr> {
    let active = match &net.session {
        Session::Offline => true,
        Session::Hosting { clients, .. } => clients.is_empty(),
        _ => false,
    };
    if !active {
        net.discover_socket = None;
        net.lan.clear();
        return None;
    }
    if net.discover_socket.is_none() {
        if let Ok(socket) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) {
            let _ = socket.set_nonblocking(true);
            let _ = socket.set_broadcast(true);
            net.discover_socket = Some(socket);
            net.last_discover = f64::NEG_INFINITY;
        }
    }
    let socket = net.discover_socket.as_ref()?;

    if now - net.last_discover >= DISCOVER_INTERVAL {
        net.last_discover = now;
        let probe = Msg::Discover { magic: MAGIC.into() };
        send(socket, (Ipv4Addr::BROADCAST, NET_PORT).into(), &probe);
        // Un autre jeu lancé sur ce même PC
        send(socket, (Ipv4Addr::LOCALHOST, NET_PORT).into(), &probe);
    }

    for (addr, msg) in recv_all(socket) {
        if let Msg::HostInfo { magic, sid, players, game, world } = msg {
            if magic != MAGIC || sid == net.sid {
                continue;
            }
            // Un même jeu répond parfois via 127.0.0.1 et via son IP : on garde une entrée par jeu.
            if let Some(h) = net.lan.iter_mut().find(|h| h.sid == sid) {
                if !addr.ip().is_loopback() || h.addr.ip().is_loopback() {
                    h.addr = addr;
                }
                h.players = players;
                h.game = game;
                h.world = world;
                h.seen = now;
            } else {
                net.lan.push(LanHost { addr, sid, players, game, world, seen: now });
            }
        }
    }
    net.lan.retain(|h| now - h.seen < LAN_EXPIRY);
    net.avoid.retain(|_, until| now < *until);

    let hosting_alone = net.mode() == NetMode::Hosting;
    pick_lan_host(&net.lan, net.sid, hosting_alone, world, &net.avoid, now)
}

/// À la fermeture du jeu : prévient les autres joueurs et referme le port de la box.
fn cleanup_on_exit(mut exit: EventReader<AppExit>, mut net: ResMut<Net>) {
    if exit.read().next().is_none() {
        return;
    }
    net.leave();
    if let Ok(Upnp::Ready { port, gateway, .. }) = net.upnp.lock().as_deref() {
        let _ = gateway.remove_port(igd_next::PortMappingProtocol::UDP, *port);
    }
}

// ─────────────────────────────────────────────────────────────────────────
//  Affichage des autres joueurs
// ─────────────────────────────────────────────────────────────────────────

#[derive(Component)]
pub(crate) struct RemoteShip {
    pub(crate) id: u32,
    color: [f32; 3],
    outline: Handle<StandardMaterial>,
    /// Son modèle (E7).
    rig: Entity,
}

#[derive(Component)]
struct RemoteLabel(u32);

#[derive(Component)]
struct RemoteLabelText;

fn label_color(c: [f32; 3]) -> Color {
    Color::srgb(0.35 + c[0] * 0.65, 0.35 + c[1] * 0.65, 0.35 + c[2] * 0.65)
}

#[allow(clippy::too_many_arguments)]
fn sync_remote_ships(
    mut commands: Commands,
    time: Res<Time>,
    net: Res<Net>,
    models: Res<crate::models::GameModels>,
    surface: Res<crate::surface::Surface>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    cam_q: Query<&GlobalTransform, With<CameraController>>,
    mut ships: Query<(Entity, &mut RemoteShip, &mut Transform, &mut Visibility)>,
    mut rigs: Query<&mut crate::models::Rig>,
    labels: Query<(Entity, &RemoteLabel)>,
) {
    let now = time.elapsed_secs_f64();
    let dt = time.delta_secs();
    let cam_pos = cam_q.get_single().map(|gt| gt.translation()).unwrap_or_default();

    // Disparus
    for (e, rs, _, _) in &ships {
        if !net.peers.contains_key(&rs.id) {
            if let Some(ec) = commands.get_entity(e) { ec.despawn_recursive(); }
        }
    }
    for (e, label) in &labels {
        if !net.peers.contains_key(&label.0) {
            if let Some(ec) = commands.get_entity(e) { ec.despawn_recursive(); }
        }
    }

    // Mise à jour des existants
    let mut present = Vec::new();
    for (_, mut rs, mut tf, mut vis) in &mut ships {
        let Some(peer) = net.peers.get(&rs.id) else { continue };
        present.push(rs.id);

        // Prédiction courte (≤ 0,25 s) + lissage exponentiel
        let age = (now - peer.last_update).clamp(0.0, 0.25) as f32;
        let predicted = peer.pos() + peer.vel * age;
        let gap = tf.translation.distance(predicted);
        if gap > 200_000.0 || tf.translation == Vec3::ZERO {
            tf.translation = predicted;
        } else {
            let k = 1.0 - (-15.0 * dt).exp();
            tf.translation = tf.translation.lerp(predicted, k);
        }
        tf.rotation = tf.rotation.slerp(peer.rot, (1.0 - (-12.0 * dt).exp()).clamp(0.0, 1.0));
        // Même taille apparente que notre propre vaisseau ; près de nous sur un astre : sa vraie
        // taille (4 voxels = 1 bloc)
        let key = peer.look.ship_key();
        let icon = (cam_pos.distance(tf.translation) * 0.008).max(0.05);
        let real = surface.voxel().zip(models.peek(&key)).map(|(v, l)| l.size().max_element() / 4.0 * v / crate::models::icon_length(l));
        tf.scale = Vec3::splat(match real {
            Some(r) if surface.center().is_some_and(|c| c.distance(tf.translation) < 1.0e6) => r,
            _ => icon,
        });
        if let Ok(mut rig) = rigs.get_mut(rs.rig) {
            if rig.key != key {
                rig.key = key;
            }
            // Un vaisseau entre ou sort de ses hangars : ses portes s'ouvrent
            if let Some((seq, t)) = crate::dock::carrier_sequence(&net, rs.id) {
                rig.play(seq);
                rig.t = t;
            } else {
                let state = if peer.status.hp == 0 { "detruit" } else if peer.look.ss.is_empty() { "vol" } else { peer.look.ss.as_str() };
                rig.play(&format!("{}{state}", crate::editeur::motion::STATE_PREFIX));
            }
        }
        // Vaisseau détruit : invisible jusqu'à sa réapparition
        let want = if peer.status.hp == 0 { Visibility::Hidden } else { Visibility::Inherited };
        if *vis != want {
            *vis = want;
        }

        if rs.color != peer.color {
            rs.color = peer.color;
            if let Some(m) = materials.get_mut(&rs.outline) { *m = outline_material(peer.color); }
        }
    }

    // Nouveaux joueurs
    for (id, peer) in &net.peers {
        if present.contains(id) {
            continue;
        }
        let outline = materials.add(outline_material(peer.color));
        let rig = commands
            .spawn((
                crate::models::Rig::new(peer.look.ship_key(), crate::models::Fit::Ship, "etat:vol"),
                Transform::IDENTITY,
                Visibility::default(),
            ))
            .id();
        commands
            .spawn((
                Transform::from_translation(peer.pos()).with_rotation(peer.rot),
                Visibility::default(),
                RemoteShip { id: *id, color: peer.color, outline: outline.clone(), rig },
            ))
            .add_child(rig);

        commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Px(240.0),
                    justify_content: JustifyContent::Center,
                    ..default()
                },
                Visibility::Hidden,
                GlobalZIndex(-1),
                RemoteLabel(*id),
            ))
            .with_child((
                Node {
                    padding: UiRect::axes(Val::Px(6.0), Val::Px(2.0)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.45)),
                BorderRadius::all(Val::Px(4.0)),
                Text::new(display_name(&peer.tag, &peer.name)),
                TextFont { font_size: 15.0, ..default() },
                TextColor(label_color(peer.color)),
                RemoteLabelText,
            ));
    }
}

/// Place le pseudo de chaque joueur au-dessus de son vaisseau, à l'écran.
fn update_remote_labels(
    net: Res<Net>,
    settings: Res<GameSettings>,
    cam_q: Query<(&Camera, &GlobalTransform), With<CameraController>>,
    ships: Query<(&RemoteShip, &GlobalTransform)>,
    mut labels: Query<(&RemoteLabel, &mut Node, &mut Visibility, &Children)>,
    mut texts: Query<(&mut Text, &mut TextColor), With<RemoteLabelText>>,
    viewport: Res<crate::graphics::ViewportScale>,
) {
    let Ok((camera, cam_gt)) = cam_q.get_single() else { return };
    for (label, mut node, mut vis, children) in &mut labels {
        let Some(peer) = net.peers.get(&label.0) else { continue };
        let ship_pos = ships
            .iter()
            .find(|(rs, _)| rs.id == label.0)
            .map(|(_, gt)| gt.translation());
        let screen = ship_pos
            .and_then(|p| camera.world_to_viewport(cam_gt, p).ok())
            .map(|p| viewport.to_window(p));
        match screen {
            Some(sp) => {
                node.left = Val::Px(sp.x - 120.0);
                node.top = Val::Px(sp.y - 48.0);
                *vis = Visibility::Inherited;
            }
            None => *vis = Visibility::Hidden,
        }
        for child in children.iter() {
            if let Ok((mut text, mut color)) = texts.get_mut(*child) {
                // Allié en vert, ennemi en rouge, neutre à sa couleur ; coque si entamée
                let relation = crate::diplomacy::relation_with(peer, &settings);
                let mut label = display_name(&peer.tag, &peer.name);
                match relation {
                    crate::diplomacy::Relation::Ally => label.push_str(" (allie)"),
                    crate::diplomacy::Relation::Enemy => label.push_str(" (ennemi)"),
                    crate::diplomacy::Relation::Neutral => {}
                }
                match peer.status.hp {
                    0 => label.push_str("\nDETRUIT"),
                    hp if hp < MAX_HP => label.push_str(&format!("\nCoque {hp}/{MAX_HP}")),
                    _ => {}
                }
                if text.0 != label {
                    text.0 = label;
                }
                let c = match relation {
                    crate::diplomacy::Relation::Neutral => label_color(peer.color),
                    other => other.color(),
                };
                if color.0 != c {
                    color.0 = c;
                }
            }
        }
    }
}

fn update_local_outline(
    settings: Res<GameSettings>,
    outline_q: Query<&LocalOutline>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut last: Local<Option<[f32; 3]>>,
) {
    if *last == Some(settings.aura_color) {
        return;
    }
    let Ok(outline) = outline_q.get_single() else { return };
    *last = Some(settings.aura_color);
    if let Some(m) = materials.get_mut(&outline.0) { *m = outline_material(settings.aura_color); }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app(name: &str, color: [f32; 3], pos: Vec3) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Assets<StandardMaterial>>()
            // Fourni par GraphicsPlugin dans le jeu (pas d'échelle de rendu ici)
            .insert_resource(crate::graphics::ViewportScale(1.0))
            .insert_resource(GameSettings {
                player_name: name.into(),
                aura_color: color,
                ..GameSettings::default()
            })
            .init_resource::<crate::net_ui::NetPanel>()
            .init_resource::<crate::models::GameModels>()
            .init_resource::<crate::surface::Surface>()
            .add_plugins((NetPlugin, crate::guild::GuildPlugin));
        // Pas de recherche UPnP pendant les tests
        let mut net = app.world_mut().resource_mut::<Net>();
        net.upnp_started = true;
        net.enabled = true;
        app.world_mut().spawn((Transform::from_translation(pos), GlobalTransform::from_translation(pos), Ship));
        app
    }

    #[test]
    fn invite_codes_roundtrip() {
        let ip = Ipv4Addr::new(86, 245, 12, 201);
        let code = encode_invite(ip, NET_PORT);
        assert_eq!(code.len(), 8, "{code}");
        assert_eq!(decode_invite(&code), Some(SocketAddr::new(ip.into(), NET_PORT)));
        assert_eq!(decode_invite(&code.to_lowercase().replace('-', " ")), Some(SocketAddr::new(ip.into(), NET_PORT)));
        let code = encode_invite(ip, 27779);
        assert_eq!(code.len(), 11, "{code}");
        assert_eq!(decode_invite(&code), Some(SocketAddr::new(ip.into(), 27779)));
        assert_eq!(decode_invite("ABC"), None);
        assert_eq!(parse_target("192.168.1.20"), Some(SocketAddr::from(([192, 168, 1, 20], NET_PORT))));
        assert_eq!(parse_target("n'importe quoi"), None);
    }

    #[test]
    fn two_lone_hosts_pick_the_same_winner() {
        let a = LanHost { addr: ([10, 0, 0, 1], NET_PORT).into(), sid: 5, players: 1, game: GAME_VERSION.into(), world: 1, seen: 0.0 };
        let b = LanHost { addr: ([10, 0, 0, 2], NET_PORT).into(), sid: 9, players: 1, game: GAME_VERSION.into(), world: 1, seen: 0.0 };
        let avoid = HashMap::new();
        // b voit a (sid plus petit) : b rejoint a. a voit b : a reste hôte.
        assert_eq!(pick_lan_host(&[a.clone()], 9, true, 1, &avoid, 0.0), Some(a.addr));
        assert_eq!(pick_lan_host(&[b.clone()], 5, true, 1, &avoid, 0.0), None);
        // Une partie déjà peuplée attire toujours
        let big = LanHost { players: 3, sid: 99, ..b.clone() };
        assert_eq!(pick_lan_host(&[big.clone()], 5, true, 1, &avoid, 0.0), Some(big.addr));
        // Carte différente : ignorée
        assert_eq!(pick_lan_host(&[big], 5, true, 2, &avoid, 0.0), None);
    }

    fn state(id: u32, sys: Option<u32>, stay: f64, clock: f64) -> PlayerState {
        let ph = Profile { name: "x".into(), ..Profile::default() }.sanitized().fingerprint();
        PlayerState { id, ph, pos: [0.0; 3], rot: [0.0, 0.0, 0.0, 1.0], sys, stay, clock, speed: 1.0, hp: MAX_HP, hits: Vec::new(), siege: None, taken: Vec::new(), look: Default::default() }
    }

    #[test]
    fn steady_state_packets_carry_no_profile() {
        let profile = Profile {
            pid: 42,
            name: "UnLongPseudo1234".into(),
            color: [0.2, 0.9, 1.0],
            claims: vec![1, 2, 3, 4, 5],
            allies: vec!["g:abcdef".into(), "p:Quelquun".into()],
            enemies: vec!["g:123456".into()],
            gid: 77,
            grev: 12,
            req: 0,
            archive: vec![(5, 3)],
        }
        .sanitized();
        let st = PlayerState {
            ph: profile.fingerprint(),
            pos: [123456.7, -2345.6, 98765.4],
            ..state(3, Some(7), 12.5, 5000.25)
        };
        let size = |profile: Option<Profile>| {
            let msg = Msg::State { state: st.clone(), chat: Vec::new(), seen: 9, profile, want: Vec::new(), gwant: Vec::new() };
            serde_json::to_string(&msg).unwrap()
        };
        let steady = size(None);
        let with_profile = size(Some(profile.clone()));
        // Le paquet de chaque tick ne contient ni pseudo, ni étoiles, ni relations, ni champs vides
        for absent in ["UnLongPseudo", "claims", "allies", "profile", "want", "chat", "hits", "hp"] {
            assert!(!steady.contains(absent), "{absent} dans {steady}");
        }
        assert!(steady.len() < 200, "{} octets : {steady}", steady.len());
        assert!(with_profile.len() > steady.len() + 150);

        // Le même profil a la même empreinte après un aller-retour réseau ou disque,
        // et la moindre modification la change
        let back: Profile = serde_json::from_str(&serde_json::to_string(&profile).unwrap()).unwrap();
        assert_eq!(back.sanitized().fingerprint(), profile.fingerprint());
        let changed = Profile { claims: vec![1, 2, 3, 4], ..profile.clone() };
        assert_ne!(changed.fingerprint(), profile.fingerprint());

        // Un profil déjà connu n'est pas compté comme nouveau (pas de réécriture du cache)
        let mut cache = ProfileCache::default();
        assert!(cache.insert(profile.clone()));
        assert!(!cache.insert(profile.clone()));
        assert_eq!(cache.get(profile.fingerprint()), Some(&profile));
    }

    #[test]
    fn clients_take_the_host_clock() {
        let mut net = Net::default();
        net.profiles.insert(Profile { name: "x".into(), ..Profile::default() });
        let host = PlayerState { speed: 10.0, ..state(HOST_ID, Some(7), 100.0, 5000.0) };
        update_peer(&mut net.peers, &net.profiles, host, 0.0);
        let mut clock = crate::world_clock::WorldClock::default();
        // Seul (ou hôte) : on garde son horloge
        follow_host_clock(&net, &mut clock, 1.0);
        assert_eq!(clock.secs, 0.0);
        // Connecté : l'horloge de l'hôte, avancée de 1 s × sa vitesse
        net.session = Session::Connected {
            socket: UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap(),
            host: "127.0.0.1:1".parse().unwrap(),
            my_id: 3,
            last_recv: 0.0,
            manual: false,
            host_code: None,
            host_pack: 0,
        };
        follow_host_clock(&net, &mut clock, 1.0);
        assert_eq!((clock.secs, clock.speed), (5010.0, 10.0));
        assert!(!net.may_set_clock());
    }

    #[test]
    fn second_game_on_same_pc_joins_automatically() {
        let mut host = test_app("Hote", [1.0, 0.0, 0.0], Vec3::new(100.0, 0.0, 0.0));
        host.update();
        assert_eq!(host.world().resource::<Net>().mode(), NetMode::Hosting);

        // Le port est pris : ce jeu trouve l'autre via la recherche locale et le rejoint seul.
        let mut client = test_app("Client", [0.0, 1.0, 0.0], Vec3::new(0.0, 50.0, 0.0));
        let start = std::time::Instant::now();
        loop {
            host.update();
            client.update();
            let ok_client = client.world().resource::<Net>().peers.values().any(|p| p.name == "Hote");
            let ok_host = host.world().resource::<Net>().peers.values().any(|p| p.name == "Client");
            if ok_client && ok_host {
                break;
            }
            assert!(start.elapsed().as_secs() < 8, "timeout");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let cnet = client.world().resource::<Net>();
        assert_eq!(cnet.mode(), NetMode::Connected);
        assert_eq!(cnet.player_count(), 2);
        let hote = cnet.peers.values().find(|p| p.name == "Hote").unwrap();
        assert_eq!(hote.color, [1.0, 0.0, 0.0]);
        assert!(hote.pos().distance(Vec3::new(100.0, 0.0, 0.0)) < 0.01);

        // Voxels (cratère d'impact) : le client les envoie, l'hôte les reçoit et les renvoie à tous
        let edit = crate::voxel::VoxelEdit { body: "p:1:2".into(), block: "2.10.20.0".into(), cells: vec![(5, 0), (6, 17)] };
        client.world_mut().resource_mut::<Net>().voxel_outbox.push(edit.clone());
        let start = std::time::Instant::now();
        while host.world().resource::<Net>().voxel_inbox.is_empty() || client.world().resource::<Net>().voxel_inbox.is_empty() {
            host.update();
            client.update();
            assert!(start.elapsed().as_secs() < 8, "voxels timeout");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(host.world().resource::<Net>().voxel_inbox[0], edit);
        assert_eq!(client.world().resource::<Net>().voxel_inbox[0], edit);

        // Modèle du client (E7, règle 7) : l'hôte le voit par son empreinte, le demande et le reçoit
        let lib = crate::editeur::motion::Library::load(None).0;
        let mut m = crate::editeur::defaults::build("vaisseau:corvette", &lib).unwrap();
        m.name = format!("Corvette {:?}", std::time::SystemTime::now());
        let bytes = m.to_bytes().unwrap();
        let fp = {
            let mut models = client.world_mut().resource_mut::<crate::models::GameModels>();
            let fp = models.add_file(bytes.clone()).unwrap();
            models.ship = crate::models::ModelKey::Print(fp);
            fp
        };
        let key = crate::models::ModelKey::Print(fp);
        let start = std::time::Instant::now();
        loop {
            host.update();
            client.update();
            let seen = host.world().resource::<Net>().peers.values().any(|p| p.look.ship.as_ref() == Some(&key));
            let mut models = host.world_mut().resource_mut::<crate::models::GameModels>();
            if seen {
                models.get(&key);
            }
            if models.files.get(&fp).is_some_and(|b| **b == bytes) {
                break;
            }
            assert!(start.elapsed().as_secs() < 15, "modele timeout");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        // Partage (E8) : le client partage un modèle, il arrive dans la bibliothèque de l'hôte
        let mut shared = crate::editeur::defaults::build("perso:lamia", &lib).unwrap();
        shared.name = format!("Naga {}", fp % 100_000);
        let sfp = client.world_mut().resource_mut::<crate::models::GameModels>().add_file(shared.to_bytes().unwrap()).unwrap();
        client.world_mut().resource_mut::<Net>().share_out.push((sfp, shared.name.clone()));
        let want = format!("{} (de Client)", shared.name);
        let dir = crate::editeur::library_dir().join("partages");
        let start = std::time::Instant::now();
        while !crate::editeur::list_models(&dir).iter().any(|e| e.name == want) {
            host.update();
            client.update();
            assert!(start.elapsed().as_secs() < 15, "partage timeout");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        // Chat : chacun écrit, les deux voient les deux messages une seule fois
        host.world_mut().send_event(NetCommand::Chat("salut".into()));
        client.world_mut().send_event(NetCommand::Chat("  coucou  ".into()));
        let texts = |app: &App| -> Vec<String> {
            app.world().resource::<Net>().chat.lines.iter().filter(|l| !l.system).map(|l| format!("{}: {}", l.name, l.text)).collect()
        };
        let start = std::time::Instant::now();
        while texts(&host).len() < 2 || texts(&client).len() < 2 {
            host.update();
            client.update();
            assert!(start.elapsed().as_secs() < 8, "chat timeout");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        for _ in 0..20 {
            host.update();
            client.update();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        for app in [&host, &client] {
            let mut t = texts(app);
            t.sort();
            assert_eq!(t, vec!["Client: coucou".to_string(), "Hote: salut".to_string()]);
        }
        assert!(client.world().resource::<Net>().chat.outbox.is_empty());

        let pump = |host: &mut App, client: &mut App, frames: usize| {
            for _ in 0..frames * 2 {
                host.update();
                client.update();
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        use crate::diplomacy::{faction_key, relation_with, Relation};
        use crate::guild::{self, Guilds, Role};
        // Agit sur la guilde d'un jeu comme le ferait son panneau Guilde
        fn act<R>(app: &mut App, f: impl FnOnce(&mut GameSettings, &mut Guilds, &mut Net) -> R) -> R {
            app.world_mut().resource_scope(|world, mut settings: Mut<GameSettings>| {
                world.resource_scope(|world, mut guilds: Mut<Guilds>| {
                    f(&mut settings, &mut guilds, &mut world.resource_mut::<Net>())
                })
            })
        }
        let peer_tag = |app: &App| app.world().resource::<Net>().peers.values().next().unwrap().tag.clone();
        let last = |app: &App| app.world().resource::<Net>().chat.lines.back().map(|l| (l.text.clone(), l.guild, l.system));
        let relation = |app: &App| {
            let world = app.world();
            let peer = world.resource::<Net>().peers.values().next().unwrap();
            relation_with(peer, world.resource::<GameSettings>())
        };
        let my_role = |app: &App| guild::my_role(app.world().resource::<GameSettings>());

        // Guildes : l'hôte crée [ABC] et en devient le Chef ; le client la découvre
        assert!(act(&mut host, |s, g, n| guild::create(s, g, n, 0.0, "Les Spores", "abc", 3, [1.0, 0.0, 0.0])));
        assert_eq!(my_role(&host), Some(Role::Chief));
        pump(&mut host, &mut client, 30);
        assert_eq!(peer_tag(&client), "ABC");
        let gid = host.world().resource::<GameSettings>().guild.as_ref().unwrap().id;
        assert_eq!(client.world().resource::<Guilds>().active(gid).unwrap().name, "Les Spores");
        assert_eq!(relation(&client), Relation::Neutral);

        // Chat de guilde : invisible pour le client, qui n'a pas de guilde
        host.world_mut().send_event(NetCommand::Chat("/g secret".into()));
        client.world_mut().send_event(NetCommand::Chat("/g perdu".into()));
        pump(&mut host, &mut client, 30);
        assert_eq!(last(&host), Some(("secret".into(), true, false)));
        assert!(client.world().resource::<Net>().chat.lines.iter().all(|l| l.text != "secret"));
        assert!(last(&client).unwrap().2);
        assert!(host.world().resource::<Net>().chat.lines.iter().all(|l| l.text != "perdu"));

        // Un joueur ne peut pas s'attribuer une guilde : sans fiche qui le confirme, pas de tag
        client.world_mut().resource_mut::<GameSettings>().clan_tag = "ABC".into();
        pump(&mut host, &mut client, 20);
        assert_eq!(peer_tag(&host), "");

        // Le client demande à rejoindre ; le Chef accepte : il devient Membre
        let rev = client.world().resource::<Guilds>().active(gid).unwrap().rev;
        client.world_mut().resource_mut::<Guilds>().request = Some((gid, rev));
        pump(&mut host, &mut client, 20);
        let (client_pid, req) = {
            let p = host.world().resource::<Net>().peers.values().next().unwrap();
            (p.status.pid, p.status.req)
        };
        assert_eq!(req, gid);
        assert!(act(&mut host, |s, g, n| guild::edit(s, g, n, 0.0, |rec, me| {
            rec.add_member(me, client_pid, "Client")?;
            Ok(None)
        })));
        pump(&mut host, &mut client, 30);
        assert_eq!(my_role(&client), Some(Role::Member));
        assert_eq!(client.world().resource::<Guilds>().request, None);
        assert_eq!(peer_tag(&host), "ABC");
        assert_eq!((relation(&host), relation(&client)), (Relation::Ally, Relation::Ally));

        // Une fois tout le monde à jour, plus rien d'autre que les positions ne circule :
        // profil confirmé par l'hôte, aucun profil ni fiche de guilde en attente ou demandé
        for app in [&host, &client] {
            let net = app.world().resource::<Net>();
            assert!(net.profile_want.is_empty() && net.guild_want.is_empty());
            assert!(net.guild_outbox.is_empty() && net.guild_asked.is_empty());
        }
        match &client.world().resource::<Net>().session {
            Session::Connected { host_pack, .. } => assert_ne!(*host_pack, 0),
            _ => panic!("client non connecte"),
        }

        // Il reçoit maintenant les messages de guilde
        host.world_mut().send_event(NetCommand::Chat("/g bienvenue".into()));
        pump(&mut host, &mut client, 30);
        {
            let cnet = client.world().resource::<Net>();
            let l = cnet.chat.lines.back().unwrap();
            assert_eq!((l.text.as_str(), l.tag.as_str(), l.guild), ("bienvenue", "ABC", true));
            assert_eq!(display_name(&l.tag, &l.name), "[ABC] Hote");
        }

        // Un Membre ne gère rien ; promu Officier par le Chef, il ne change toujours aucun rôle
        let host_pid = host.world().resource::<GameSettings>().player_id;
        assert!(!act(&mut client, |s, g, n| guild::edit(s, g, n, 0.0, |rec, me| {
            rec.kick(me, host_pid)?;
            Ok(None)
        })));
        assert!(act(&mut host, |s, g, n| guild::edit(s, g, n, 0.0, |rec, me| {
            rec.set_role(me, client_pid, Role::Officer)?;
            Ok(None)
        })));
        pump(&mut host, &mut client, 30);
        assert_eq!(my_role(&client), Some(Role::Officer));
        assert!(!act(&mut client, |s, g, n| guild::edit(s, g, n, 0.0, |rec, me| {
            rec.set_role(me, host_pid, Role::Member)?;
            Ok(None)
        })));

        // Étoiles revendiquées : annoncées aux autres joueurs, 5 au plus
        host.world_mut().resource_mut::<GameSettings>().claims = vec![3, 8];
        client.world_mut().resource_mut::<GameSettings>().claims = (10..20).collect();
        pump(&mut host, &mut client, 20);
        let peer_claims = |app: &App| app.world().resource::<Net>().peers.values().next().unwrap().claims.clone();
        assert_eq!(peer_claims(&client), vec![3, 8]);
        assert_eq!(peer_claims(&host), vec![10, 11, 12, 13, 14]);

        // Le client quitte la guilde : l'hôte l'apprend, ils redeviennent neutres
        act(&mut client, |s, g, n| guild::leave(s, g, n, 0.0));
        assert_eq!(my_role(&client), None);
        pump(&mut host, &mut client, 30);
        assert_eq!(host.world().resource::<GameSettings>().guild.as_ref().unwrap().members.len(), 1);
        assert_eq!(peer_tag(&host), "");
        assert_eq!((relation(&host), relation(&client)), (Relation::Neutral, Relation::Neutral));

        // Alliance : demandée par la guilde, elle ne vaut que si le joueur l'accepte ; la guerre se déclare seul
        let guild_relation = |host: &mut App, r: Relation| {
            assert!(act(host, |s, g, n| guild::edit(s, g, n, 0.0, |rec, me| {
                rec.set_relation(me, &faction_key(0, "Client"), "Client", r)?;
                Ok(None)
            })));
        };
        let personal = |client: &mut App, r: Relation| {
            let mut s = client.world_mut().resource_mut::<GameSettings>();
            let key = faction_key(gid, "Hote");
            s.allies.retain(|k| *k != key);
            s.enemies.retain(|k| *k != key);
            match r {
                Relation::Ally => s.allies.push(key),
                Relation::Enemy => s.enemies.push(key),
                Relation::Neutral => {}
            }
        };
        guild_relation(&mut host, Relation::Ally);
        pump(&mut host, &mut client, 20);
        // Demande d'alliance de la guilde seule : pas encore alliés
        assert_eq!((relation(&host), relation(&client)), (Relation::Neutral, Relation::Neutral));
        // Le joueur accepte : alliés des deux côtés
        personal(&mut client, Relation::Ally);
        pump(&mut host, &mut client, 20);
        assert_eq!((relation(&host), relation(&client)), (Relation::Ally, Relation::Ally));
        // Le client déclare la guerre : ennemis des deux côtés
        personal(&mut client, Relation::Enemy);
        pump(&mut host, &mut client, 20);
        assert_eq!((relation(&host), relation(&client)), (Relation::Enemy, Relation::Enemy));

        // Combat : la coque, les tirs et les sièges sont annoncés
        {
            let mut net = host.world_mut().resource_mut::<Net>();
            let target = *net.peers.keys().next().unwrap();
            net.local.hp = 40;
            net.local.hits = vec![(target, 3)];
            net.local.siege = Some(12);
        }
        pump(&mut host, &mut client, 20);
        {
            let cnet = client.world().resource::<Net>();
            let status = &cnet.peers.values().next().unwrap().status;
            assert_eq!((status.hp, status.hits.clone(), status.siege), (40, vec![(cnet.my_id(), 3)], Some(12)));
        }

        // Dissolution par le Chef : la guilde disparaît aussi chez le client
        assert!(act(&mut host, |s, g, n| guild::edit(s, g, n, 0.0, |rec, me| {
            rec.dissolve(me)?;
            Ok(None)
        })));
        assert_eq!(my_role(&host), None);
        pump(&mut host, &mut client, 30);
        assert!(client.world().resource::<Guilds>().active(gid).is_none());

        // Deux jeux sur la même sauvegarde : le client (identifiant réseau plus élevé) prend
        // une identité neuve, l'hôte garde la sienne
        let host_pid = host.world().resource::<GameSettings>().player_id;
        assert!(act(&mut host, |s, g, n| guild::create(s, g, n, 0.0, "Doublon", "dbl", 1, [0.0, 1.0, 0.0])));
        {
            let mut s = client.world_mut().resource_mut::<GameSettings>();
            s.player_id = host_pid;
            s.player_name = "Hote".into();
        }
        pump(&mut host, &mut client, 40);
        {
            let hs = host.world().resource::<GameSettings>();
            let cs = client.world().resource::<GameSettings>();
            assert_eq!(hs.player_id, host_pid);
            assert!(!hs.temp_identity && hs.guild.is_some());
            assert_ne!(cs.player_id, host_pid);
            assert!(cs.temp_identity && cs.guild.is_none());
            assert_ne!(cs.player_name, "Hote");
        }

        // L'hôte ferme : le client revient à sa propre partie (ou hors ligne si le port est encore pris)
        host.world_mut().resource_mut::<Net>().leave();
        drop(host);
        for _ in 0..20 {
            client.update();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let cnet = client.world().resource::<Net>();
        assert!(cnet.peers.is_empty());
        assert_ne!(cnet.mode(), NetMode::Connected);
    }
}
