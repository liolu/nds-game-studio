//! nds-nitro : lecture des formats Nintendo standard des jeux DS (SDK Nitro).
//!
//! - 3D : `.nsbmd` (modèles BMD0) et `.nsbtx` (textures BTX0). Les modules `nitro`, `nds`,
//!   `primitives`, `decompress`, `util` et `errors` sont repris d'apicula
//!   (<https://github.com/scurest/apicula>, licence 0BSD, voir LICENSE-apicula, commit 3d4e91e) ;
//!   `model3d` en tire des maillages et des images RGBA prêts pour un moteur.
//! - Son : `sadl`, archives `SDAT` (`sdat`).
//! - 2D : `.nclr` (palettes), `.ncgr` (tuiles), `.nscr` (écrans de tuiles), `.ncer` (cellules /
//!   sprites), voir `g2d`.
//! - Archives `.narc` (`narc`) et fichiers compressés LZ77 du BIOS (`decompress`).
//! - Les fichiers d'une rom, lus dans la rom : NitroFS (`nitrofs`), archives numérotées `rom.bin` (`romarc`).

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

pub mod adx;
pub mod g2d;
pub mod gx_mesh;
pub mod model3d;
pub mod narc;
pub mod nitrofs;
pub mod romarc;
pub mod sadl;
pub mod sdat;

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
    /// Animation de squelette (BCA0), jouée sur un modèle.
    ModelAnim,
    /// Animation de cellules (RNAN).
    CellAnim,
    /// Cellules et tuiles dans un seul fichier, et leur mise en page (JNCD, JNCL : Nintendogs).
    JCells,
    JLayout,
    /// Son (sadl, STRM, SWAV).
    Audio,
    /// Banque de sons (SWAR).
    AudioBank,
    /// Archive de son (SDAT) : ses fichiers sont listés à part.
    SoundArchive,
    /// Police (NFTR).
    Font,
    /// Messages (BMG).
    Messages,
    /// Texture d'un moteur maison (Rayman Raving Rabbids 2, Battle of Giants, Star Wars III...) : ses pixels
    /// (`LXET`, avec le format et la taille de la DS à la fin) et le fichier qui la relie à sa palette (`TXET`).
    Texel,
    TextureRef,
    /// Image plein format du même moteur (` PMB` : une image 8 bits compressée en LZ77).
    EngineBitmap,
    /// Maillage du même moteur (`GEOM` : commandes de la DS) et ce qui le relie à sa texture (`MESH`).
    EngineGeometry,
    EngineMesh,
    /// Pixels d'une texture sans en-tête (`.ntft`), avec sa palette `.ntfp`.
    RawTexture,
    /// Texture avec son en-tête et sa palette (`.bstga`, MySims).
    BsTexture,
    /// Image Windows (`.bmp`).
    Bitmap,
    /// Archive `.zip` : la liste de ses fichiers.
    ZipFile,
    /// Vidéo (Bink, VX, Mobiclip) : ses caractéristiques, elle n'est pas lue.
    Video,
    /// Planche de tuiles avec sa palette au début (Heroes of Mana).
    PaletteTiles,
    /// Fichier `.z` (Tom and Jerry) : image, écran ou texture une fois décompressé.
    ZImage,
    /// Animation `.anm` : un résumé, elle n'est pas jouée.
    AnmFile,
    /// Texte brut (`.txt`, `.ini`...).
    PlainText,
    /// Fichier compressé en LZ77 sans format connu dedans (`.lz`, `.cat`).
    Compressed,
    /// Mini-rom pour le téléchargement sans fil (`.srl`).
    NestedRom,
    /// Tuiles, palette, écran bruts, sans en-tête (`.nbfc`, `.nbfp`, `.nbfs`) : reconnus par leur extension.
    RawTiles,
    RawPalette,
    RawScreen,
    /// Script compilé (SB, DCPB) : montré en texte et en octets.
    Script,
    /// Autres fichiers Nitro 3D sans rien à montrer (BTP0, BTA0...).
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
            Kind::ModelAnim => "animation 3D",
            Kind::CellAnim => "animation 2D",
            Kind::JCells => "cellules J",
            Kind::JLayout => "mise en page",
            Kind::Audio => "son",
            Kind::AudioBank => "banque de sons",
            Kind::SoundArchive => "archive de son",
            Kind::Font => "police",
            Kind::PlainText => "texte",
            Kind::Texel | Kind::TextureRef | Kind::RawTexture | Kind::BsTexture => "texture",
            Kind::EngineGeometry | Kind::EngineMesh => "modele 3D",
            Kind::Bitmap | Kind::ZImage => "image",
            Kind::PaletteTiles => "tuiles",
            Kind::AnmFile => "animation 2D",
            Kind::ZipFile => "archive",
            Kind::Video => "video",
            Kind::EngineBitmap => "image",
            Kind::Compressed => "compresse",
            Kind::NestedRom => "mini rom",
            Kind::Messages => "messages",
            Kind::RawTiles => "tuiles",
            Kind::RawPalette => "palette",
            Kind::RawScreen => "ecran",
            Kind::Script => "script",
            Kind::Nitro3dOther => "3D (anim)",
            Kind::Nitro2dOther => "2D (anim)",
            Kind::Other => "",
        }
    }

    pub fn viewable(self) -> bool {
        matches!(self, Kind::Model | Kind::Textures | Kind::Palette | Kind::Tiles | Kind::Screen | Kind::Cells | Kind::ModelAnim | Kind::CellAnim | Kind::JCells | Kind::JLayout | Kind::Audio | Kind::AudioBank | Kind::Font | Kind::Messages | Kind::PaletteTiles | Kind::ZImage | Kind::AnmFile | Kind::EngineGeometry | Kind::EngineMesh | Kind::RawTexture | Kind::BsTexture | Kind::Bitmap | Kind::ZipFile | Kind::Video | Kind::Texel | Kind::TextureRef | Kind::EngineBitmap | Kind::PlainText | Kind::Compressed | Kind::NestedRom | Kind::RawTiles | Kind::RawPalette | Kind::RawScreen | Kind::Script)
    }
}

/// Sorte d'un fichier d'après ses premiers octets.
pub fn sniff(head: &[u8]) -> Kind {
    match head.get(0..4) {
        Some(b"BMD0") => Kind::Model,
        Some(b"BTX0") => Kind::Textures,
        Some(b"BCA0") => Kind::ModelAnim,
        Some(b"BTP0" | b"BTA0" | b"BMA0" | b"BVA0") => Kind::Nitro3dOther,
        Some(b"RLCN" | b"NCLR" | b"RPCN" | b"NCCL" | b"TTLP" | b"NCL\0") => Kind::Palette,
        Some(b"RGCN" | b"NCGR" | b"RCBN" | b"NCCG" | b"NCG\0") => Kind::Tiles,
        Some(b"RCSN" | b"NSCR" | b"NCSC" | b"NSC\0") => Kind::Screen,
        Some(b"RECN" | b"NCER") => Kind::Cells,
        Some(b"NARC") => Kind::Archive,
        Some(b"RNAN") => Kind::CellAnim,
        Some(b"JNCD") => Kind::JCells,
        Some(b"JNCL") => Kind::JLayout,
        Some(b"sadl" | b"STRM" | b"SWAV") => Kind::Audio,
        Some(b"SWAR") => Kind::AudioBank,
        Some(b"SDAT") => Kind::SoundArchive,
        Some(b"RTFN") => Kind::Font,
        Some(b"MESG" | b"GNAL") => Kind::Messages,
        Some(b"MOEG") => Kind::EngineGeometry,
        Some(b"HSEM") => Kind::EngineMesh,
        Some(b"BIKb" | b"BIKd" | b"BIKf" | b"BIKg" | b"BIKh" | b"BIKi" | b"VXDS" | b"MODS") => Kind::Video,
        Some(b"LXET") => Kind::Texel,
        Some(b"TXET") => Kind::TextureRef,
        Some(b" PMB") => Kind::EngineBitmap,
        Some(b"SB  " | b"DCPB") => Kind::Script,
        Some(b"RCMN" | b"RNMN") => Kind::Nitro2dOther,
        _ => Kind::Other,
    }
}

/// Le fichier, décompressé s'il est compressé en LZ77 (0x10 / 0x11) et que le résultat est un
/// fichier Nitro reconnu ; sinon tel quel.
pub fn unpack(bytes: Vec<u8>) -> Vec<u8> {
    if sniff(&bytes) == Kind::Other {
        if let Some(out) = decompress_any(&bytes) {
            if sniff(&out) != Kind::Other {
                return out;
            }
        }
    }
    bytes
}

/// Le premier octet d'un fichier compressé par le BIOS de la DS : LZ77 (0x10, 0x11), RLE (0x30), Huffman (0x24, 0x28),
/// ou simplement stocké avec le même en-tête (0).
pub fn looks_compressed(first: Option<&u8>) -> bool {
    matches!(first, Some(0x00 | 0x10 | 0x11 | 0x24 | 0x28 | 0x30))
}

/// Le contenu d'un fichier compressé par le BIOS, quel qu'il soit ; `None` si ce n'en est pas un.
pub fn decompress_any(bytes: &[u8]) -> Option<Vec<u8>> {
    const MAX_SIZE: usize = 64 << 20;
    if !looks_compressed(bytes.first()) || bytes.len() < 4 {
        return None;
    }
    let size = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize >> 8;
    if size == 0 || size > MAX_SIZE {
        return None;
    }
    match bytes[0] {
        // Stocké : la taille annoncée doit être celle du fichier (à l'alignement près), sinon c'est autre chose
        0x00 => (size + 4 <= bytes.len() && bytes.len() - size - 4 < 4).then(|| bytes[4..4 + size].to_vec()),
        0x10 | 0x11 => decompress::decompress(util::cur::Cur::new(bytes)).ok().map(|out| out.data),
        0x30 => romarc::rle(bytes, size),
        _ => romarc::huffman(bytes, size),
    }
}

/// Couleur DS BGR555 -> RGBA.
pub(crate) fn bgr555(c: u16) -> [u8; 4] {
    let ch = |v: u16| ((v & 31) as u32 * 255 / 31) as u8;
    [ch(c), ch(c >> 5), ch(c >> 10), 255]
}
