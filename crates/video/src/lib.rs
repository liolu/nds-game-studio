//! Les vidéos des jeux DS : VX (Actimagine), MODS (Mobiclip) et Bink 1.
//!
//! Portage en Rust des décodeurs de FFmpeg et de mobipeg, sous LGPL 2.1 ou ultérieure :
//! ce crate garde cette licence (voir COPYING), à part du reste du studio.

pub mod bink;
mod bits;
pub mod mobiclip;
mod tables;
pub mod vx;

use std::sync::Arc;

pub(crate) use bits::R;

pub(crate) fn le16(bytes: &[u8], at: usize) -> u16 {
    bytes.get(at..at + 2).map_or(0, |s| u16::from_le_bytes([s[0], s[1]]))
}

pub(crate) fn le32(bytes: &[u8], at: usize) -> u32 {
    bytes.get(at..at + 4).map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Ce que dit l'en-tête d'une vidéo.
#[derive(Clone, Debug)]
pub struct Info {
    pub format: &'static str,
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub fps: f32,
    /// 0 : pas de son lisible.
    pub sample_rate: u32,
    pub channels: u16,
}

impl Info {
    pub fn seconds(&self) -> f32 {
        self.frames as f32 / self.fps.max(0.01)
    }
}

pub(crate) trait Stream: Send {
    fn rewind(&mut self);
    /// L'image suivante, et son son ajouté à `pcm`. `None` à la fin.
    fn next(&mut self, pcm: &mut Vec<f32>) -> Option<R>;
    fn rgba(&self, out: &mut Vec<u8>);
    /// Les trois plans à la suite, tels que décodés.
    fn yuv(&self, out: &mut Vec<u8>);
}

/// Une vidéo qu'on lit de bout en bout, image par image.
pub struct Video {
    info: Info,
    stream: Box<dyn Stream>,
    errors: u32,
    last_error: Option<&'static str>,
}

impl Video {
    /// `None` si ce n'est pas une vidéo qu'on sait lire ; l'erreur dit pourquoi sinon.
    pub fn open(bytes: impl Into<Arc<[u8]>>) -> Result<Video, String> {
        let bytes: Arc<[u8]> = bytes.into();
        let (stream, info): (Box<dyn Stream>, Info) = match bytes.get(..4) {
            Some(b"VXDS") => {
                let (s, i) = vx::Vx::open(bytes)?;
                (Box::new(s), i)
            }
            Some(b"MODS") => {
                let (s, i) = mobiclip::Mods::open(bytes)?;
                (Box::new(s), i)
            }
            Some([b'B', b'I', b'K', _]) => {
                let (s, i) = bink::Bink::open(bytes)?;
                (Box::new(s), i)
            }
            _ => return Err("pas une video connue".into()),
        };
        Ok(Video { info, stream, errors: 0, last_error: None })
    }

    pub fn info(&self) -> &Info {
        &self.info
    }

    /// Le nombre d'images illisibles depuis le début, et la dernière raison.
    pub fn errors(&self) -> (u32, Option<&'static str>) {
        (self.errors, self.last_error)
    }

    pub fn rewind(&mut self) {
        self.stream.rewind();
    }

    /// Passe à l'image suivante. Son son (entrelacé, -1 à 1) est ajouté à `pcm`.
    /// `false` à la fin de la vidéo. Une image illisible garde la précédente.
    pub fn advance(&mut self, pcm: &mut Vec<f32>) -> bool {
        match self.stream.next(pcm) {
            None => false,
            Some(Ok(())) => true,
            Some(Err(e)) => {
                self.errors += 1;
                self.last_error = Some(e);
                true
            }
        }
    }

    /// L'image en cours, en RGBA (`largeur * hauteur * 4` octets).
    pub fn rgba(&self, out: &mut Vec<u8>) {
        self.stream.rgba(out);
    }

    /// L'image en cours en YUV 4:2:0, pour la comparer à un autre décodeur.
    pub fn yuv(&self, out: &mut Vec<u8>) {
        self.stream.yuv(out);
    }
}
