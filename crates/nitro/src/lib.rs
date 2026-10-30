//! nds-nitro : lecture des formats Nintendo standard des jeux DS (SDK Nitro).
//!
//! - 3D : `.nsbmd` (modèles BMD0) et `.nsbtx` (textures BTX0). Les modules `nitro`, `nds`,
//!   `primitives`, `decompress`, `util` et `errors` sont repris d'apicula
//!   (<https://github.com/scurest/apicula>, licence 0BSD, voir LICENSE-apicula, commit 3d4e91e) ;
//!   `model3d` en tire des maillages et des images RGBA prêts pour un moteur.
//! - 2D : `.nclr` (palettes), `.ncgr` (tuiles), `.nscr` (écrans de tuiles), `.ncer` (cellules /
//!   sprites), voir `g2d`.
//! - Archives `.narc` (`narc`) et fichiers compressés LZ77 du BIOS (`decompress`).

#![allow(mismatched_lifetime_syntaxes)]

#[macro_use]
extern crate log;

#[macro_use]
pub mod errors;
#[macro_use]
pub mod util;
pub mod decompress;
pub mod nds;
pub mod nitro;
pub mod primitives;

pub mod g2d;
pub mod model3d;
pub mod narc;

pub use errors::Result;

/// Image RGBA 8 bits, lignes de haut en bas.
#[derive(Clone)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Sorte de fichier reconnue par sa signature (les extensions mentent souvent).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Model,
    Textures,
    Palette,
    Tiles,
    Screen,
    Cells,
    Archive,
    /// Animations ou autres fichiers Nitro 3D sans rien à montrer (BCA0, BTP0, BTA0...).
    Nitro3dOther,
    /// Autre fichier Nitro 2D (NANR, NMCR...).
    Nitro2dOther,
    Other,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Model => "modele 3D",
            Kind::Textures => "textures 3D",
            Kind::Palette => "palette",
            Kind::Tiles => "tuiles",
            Kind::Screen => "ecran",
            Kind::Cells => "cellules",
            Kind::Archive => "archive",
            Kind::Nitro3dOther => "3D (anim)",
            Kind::Nitro2dOther => "2D (anim)",
            Kind::Other => "",
        }
    }

    pub fn viewable(self) -> bool {
        matches!(self, Kind::Model | Kind::Textures | Kind::Palette | Kind::Tiles | Kind::Screen | Kind::Cells)
    }
}

/// Sorte d'un fichier d'après ses premiers octets.
pub fn sniff(head: &[u8]) -> Kind {
    match head.get(0..4) {
        Some(b"BMD0") => Kind::Model,
        Some(b"BTX0") => Kind::Textures,
        Some(b"BCA0" | b"BTP0" | b"BTA0" | b"BMA0" | b"BVA0") => Kind::Nitro3dOther,
        Some(b"RLCN" | b"NCLR" | b"RPCN") => Kind::Palette,
        Some(b"RGCN" | b"NCGR" | b"RCBN") => Kind::Tiles,
        Some(b"RCSN" | b"NSCR") => Kind::Screen,
        Some(b"RECN" | b"NCER") => Kind::Cells,
        Some(b"NARC") => Kind::Archive,
        Some(b"RNAN" | b"RCMN" | b"RNMN" | b"RTFN") => Kind::Nitro2dOther,
        _ => Kind::Other,
    }
}

/// Le fichier, décompressé s'il est compressé en LZ77 (0x10 / 0x11) et que le résultat est un
/// fichier Nitro reconnu ; sinon tel quel.
pub fn unpack(bytes: Vec<u8>) -> Vec<u8> {
    if sniff(&bytes) == Kind::Other && matches!(bytes.first(), Some(0x10 | 0x11)) {
        if let Ok(out) = decompress::decompress(util::cur::Cur::new(&bytes)) {
            if sniff(&out.data) != Kind::Other {
                return out.data;
            }
        }
    }
    bytes
}

/// Couleur DS BGR555 -> RGBA.
pub(crate) fn bgr555(c: u16) -> [u8; 4] {
    let ch = |v: u16| ((v & 31) as u32 * 255 / 31) as u8;
    [ch(c), ch(c >> 5), ch(c >> 10), 255]
}
