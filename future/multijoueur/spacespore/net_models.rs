//! Modèles des joueurs sur le réseau (E7, règle 7 de `roadmaps/fait/ROADMAP-0.12-editeur.md`).
//!
//! - Chaque état de joueur porte son `Looks` : ses modèles (`ModelKey` : un modèle par défaut se
//!   désigne par son nom, un fichier par son **empreinte**), l'état de son vaisseau, son marcheur
//!   (astre, pose, animation) et son amarrage dans un hangar.
//! - Un modèle inconnu est demandé par empreinte ; il passe par l'hôte (qui le garde), en morceaux de
//!   16 Kio redemandés tant qu'il en manque, une seule fois : ensuite il est dans
//!   `saves/cache_modeles/`. 10 Mo au plus (Q4), empreinte vérifiée à l'arrivée.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;

use bevy::math::{Quat, Vec3};
use serde::{Deserialize, Serialize};

use crate::editeur::format::{Model, MAX_FILE_BYTES};
use crate::models::ModelKey;
use crate::ui::TargetKind;

/// Taille d'un morceau de fichier (avant l'écriture en hexadécimal).
pub const PART: usize = 16 * 1024;
/// Morceaux au plus pour un fichier de 10 Mo.
pub const MAX_PARTS: u32 = (MAX_FILE_BYTES / PART + 1) as u32;
/// Fichiers attendus en même temps au plus.
const MAX_INCOMING: usize = 8;
/// Morceaux envoyés par image au plus.
pub const PARTS_PER_FRAME: usize = 12;
/// Morceaux redemandés par paquet au plus, et tous les combien.
const ASK_PARTS: usize = 48;
const ASK_EVERY: f64 = 0.5;
/// Un fichier qui n'arrive pas est abandonné (et redemandé plus tard).
const GIVE_UP: f64 = 120.0;

/// Le marcheur d'un joueur : astre (1 = planète, 2 = lune), pose dans le repère de l'astre,
/// animation, taille d'un voxel du terrain.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct WalkState {
    pub kind: u8,
    pub a: u32,
    #[serde(default)]
    pub b: u32,
    pub pos: [f32; 3],
    pub rot: [f32; 4],
    pub anim: String,
    pub v: f32,
}

impl WalkState {
    pub fn new(body: TargetKind, local: bevy::prelude::Transform, anim: &str) -> Option<Self> {
        let (kind, a, b) = match body {
            TargetKind::Planet(id) => (1, id as u32, 0),
            TargetKind::Moon(p, m) => (2, p as u32, m as u32),
            _ => return None,
        };
        Some(Self { kind, a, b, pos: local.translation.to_array(), rot: local.rotation.to_array(), anim: anim.into(), v: local.scale.x })
    }

    pub fn body(&self) -> Option<TargetKind> {
        match self.kind {
            1 => Some(TargetKind::Planet(self.a as usize)),
            2 => Some(TargetKind::Moon(self.a as usize, self.b as usize)),
            _ => None,
        }
    }

    pub fn local(&self) -> bevy::prelude::Transform {
        bevy::prelude::Transform { translation: Vec3::from_array(self.pos), rotation: Quat::from_array(self.rot).normalize(), scale: Vec3::splat(self.v) }
    }

    fn valid(&self) -> bool {
        self.pos.iter().chain(self.rot.iter()).all(|x| x.is_finite()) && self.v.is_finite() && self.v > 0.0 && self.anim.len() <= 32
    }
}

/// Amarrage d'un vaisseau dans le hangar `hangar` du vaisseau du joueur `carrier` : entrée (0),
/// amarré (1), sortie (2), instant de la séquence.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct DockState {
    pub carrier: u32,
    pub hangar: u8,
    pub phase: u8,
    pub t: f32,
}

/// L'allure d'un joueur : ses modèles et ce qu'ils font.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Looks {
    /// Modèles (absents : ceux par défaut).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ship: Option<ModelKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chr: Option<ModelKey>,
    /// État du vaisseau (« vol », « pose »...).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ss: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub walk: Option<WalkState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dock: Option<DockState>,
    /// Rentrée atmosphérique (0.13 P9) : chaleur (0..255) et vitesse en dixièmes... de Mach x 20 :
    /// les autres le voient comme une étoile filante, et entendent son bang.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub heat: u8,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub mach: u8,
}

fn is_zero(x: &u8) -> bool {
    *x == 0
}

impl Looks {
    pub fn is_default(&self) -> bool {
        *self == Looks::default()
    }

    /// Ce qui vient d'un autre joueur : borné et vérifié.
    pub fn sanitized(mut self) -> Self {
        let name_ok = |k: &Option<ModelKey>| match k {
            Some(ModelKey::Default(s)) => s.len() <= 48,
            _ => true,
        };
        if !name_ok(&self.ship) {
            self.ship = None;
        }
        if !name_ok(&self.chr) {
            self.chr = None;
        }
        self.ss.truncate(16);
        if self.walk.as_ref().is_some_and(|w| !w.valid()) {
            self.walk = None;
        }
        if self.dock.is_some_and(|d| !d.t.is_finite()) {
            self.dock = None;
        }
        self
    }

    pub fn ship_key(&self) -> ModelKey {
        self.ship.clone().unwrap_or_else(ModelKey::default_ship)
    }

    pub fn character_key(&self) -> ModelKey {
        self.chr.clone().unwrap_or_else(ModelKey::default_character)
    }
}

/// Octets -> hexadécimal (dans un paquet JSON).
pub fn to_hex(b: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push(H[(x >> 4) as usize] as char);
        s.push(H[(x & 15) as usize] as char);
    }
    s
}

pub fn from_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || s.len() > PART * 2 {
        return None;
    }
    let digit = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    s.as_bytes().chunks(2).map(|p| Some(digit(p[0])? << 4 | digit(p[1])?)).collect()
}

/// Nombre de morceaux d'un fichier, et le morceau `i` (hexadécimal).
pub fn part_of(bytes: &[u8], i: u32) -> Option<(u32, String)> {
    let total = bytes.len().div_ceil(PART).max(1) as u32;
    let start = i as usize * PART;
    (i < total).then(|| (total, to_hex(&bytes[start..(start + PART).min(bytes.len())])))
}

struct Incoming {
    total: u32,
    parts: HashMap<u32, Vec<u8>>,
    last_ask: f64,
    started: f64,
}

/// Fichiers de modèles en route (attendus, et à envoyer).
#[derive(Default)]
pub struct Transfers {
    incoming: HashMap<u64, Incoming>,
    queue: VecDeque<(SocketAddr, u64, u32)>,
}

impl Transfers {
    /// Ce fichier nous manque : on le demandera.
    pub fn want(&mut self, fp: u64, now: f64) {
        if self.incoming.len() < MAX_INCOMING {
            self.incoming.entry(fp).or_insert(Incoming { total: 0, parts: HashMap::new(), last_ask: -1.0, started: now });
        }
    }

    /// Les demandes à envoyer maintenant : (empreinte, morceaux qui manquent).
    pub fn asks(&mut self, now: f64) -> Vec<(u64, Vec<u32>)> {
        self.incoming.retain(|_, inc| now - inc.started < GIVE_UP);
        let mut out = Vec::new();
        for (fp, inc) in self.incoming.iter_mut() {
            if now - inc.last_ask < ASK_EVERY {
                continue;
            }
            inc.last_ask = now;
            let missing: Vec<u32> = if inc.total == 0 { vec![0] } else { (0..inc.total).filter(|i| !inc.parts.contains_key(i)).take(ASK_PARTS).collect() };
            out.push((*fp, missing));
        }
        out
    }

    /// Un morceau reçu ; le fichier complet (empreinte vérifiée) quand c'est le dernier.
    pub fn receive(&mut self, fp: u64, index: u32, total: u32, data: &str) -> Option<Vec<u8>> {
        let inc = self.incoming.get_mut(&fp)?;
        if total == 0 || total > MAX_PARTS || index >= total || (inc.total != 0 && inc.total != total) {
            return None;
        }
        inc.total = total;
        let bytes = from_hex(data)?;
        inc.parts.insert(index, bytes);
        if inc.parts.len() as u32 != total {
            return None;
        }
        let inc = self.incoming.remove(&fp)?;
        let mut all = Vec::new();
        for i in 0..total {
            all.extend_from_slice(inc.parts.get(&i)?);
        }
        (Model::fingerprint(&all) == fp && all.len() <= MAX_FILE_BYTES).then_some(all)
    }

    /// Envoyer ces morceaux de `fp` à `to`.
    pub fn serve(&mut self, to: SocketAddr, fp: u64, parts: &[u32]) {
        for i in parts.iter().take(ASK_PARTS) {
            if *i < MAX_PARTS && !self.queue.contains(&(to, fp, *i)) {
                self.queue.push_back((to, fp, *i));
            }
        }
    }

    /// Les prochains morceaux à envoyer.
    pub fn next_parts(&mut self, n: usize) -> Vec<(SocketAddr, u64, u32)> {
        let n = n.min(self.queue.len());
        self.queue.drain(..n).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_crosses_the_network_in_parts() {
        let lib = crate::editeur::motion::Library::load(None).0;
        let bytes = crate::editeur::defaults::build("vaisseau:chasseur", &lib).unwrap().to_bytes().unwrap();
        let fp = Model::fingerprint(&bytes);
        let addr: SocketAddr = "127.0.0.1:9".parse().unwrap();
        let (mut a, mut b) = (Transfers::default(), Transfers::default());
        b.want(fp, 0.0);
        let mut got = None;
        for round in 0..200 {
            let now = round as f64;
            for (f, parts) in b.asks(now) {
                a.serve(addr, f, &parts);
            }
            for (_, f, i) in a.next_parts(PARTS_PER_FRAME) {
                // Un morceau sur trois se perd en route
                if (i + round) % 3 == 0 {
                    continue;
                }
                let (total, data) = part_of(&bytes, i).unwrap();
                if let Some(all) = b.receive(f, i, total, &data) {
                    got = Some(all);
                }
            }
            if got.is_some() {
                break;
            }
        }
        assert_eq!(got.as_deref(), Some(bytes.as_slice()));
        // Un morceau faux : le fichier est refusé
        let mut c = Transfers::default();
        c.want(fp, 0.0);
        let total = part_of(&bytes, 0).unwrap().0;
        let mut ok = true;
        for i in 0..total {
            let (_, mut data) = part_of(&bytes, i).unwrap();
            if i == 0 {
                data.replace_range(0..2, "ff");
            }
            ok &= c.receive(fp, i, total, &data).is_none();
        }
        assert!(ok);
        assert_eq!(from_hex(&to_hex(&[0, 1, 254, 255])), Some(vec![0, 1, 254, 255]));
    }

    #[test]
    fn looks_survive_json_and_bad_values_are_dropped() {
        let l = Looks {
            ship: Some(ModelKey::Print(42)),
            chr: Some(ModelKey::Default("perso:centaure".into())),
            ss: "combat".into(),
            walk: WalkState::new(TargetKind::Moon(3, 1), bevy::prelude::Transform::from_xyz(1.0, 2.0, 3.0).with_scale(Vec3::splat(4.0)), "courir"),
            dock: Some(DockState { carrier: 2, hangar: 0, phase: 1, t: 3.0 }),
            heat: 200,
            mach: 60,
        };
        let back: Looks = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
        assert_eq!(back, l);
        assert!(back.walk.as_ref().and_then(|w| w.body()) == Some(TargetKind::Moon(3, 1)));
        let bad = Looks { walk: Some(WalkState { v: f32::NAN, ..l.walk.clone().unwrap() }), ..l };
        assert!(bad.sanitized().walk.is_none());
        assert!(serde_json::to_string(&Looks::default()).unwrap().len() < 4);
    }
}
