//! Jouer une vidéo de jeu DS (VX, MODS, Bink) dans une image Bevy, avec son son.
//!
//! Un jeu crée un [`VideoPlayer`], affiche [`VideoPlayer::image`] où il veut, appelle
//! [`VideoPlayer::play`] puis [`VideoPlayer::update`] à chaque image.
//! Le décodage a lieu dans un fil à part ; l'image suit le temps écoulé.

use std::collections::VecDeque;
use std::num::NonZero;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bevy::asset::RenderAssetUsages;
use bevy::audio::{AddAudioSource, Decodable, Sample, Source};
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
pub use nds_video::{Info, Video};

/// Images décodées d'avance.
const AHEAD: usize = 12;

pub struct VideoPlugin;

impl Plugin for VideoPlugin {
    fn build(&self, app: &mut App) {
        app.add_audio_source::<VideoAudio>();
    }
}

#[derive(Default)]
struct AudioQueue {
    samples: Mutex<VecDeque<f32>>,
    /// Plus rien ne viendra : le son s'arrête une fois la file vide.
    finished: AtomicBool,
}

/// Le son d'une vidéo, au fur et à mesure du décodage.
#[derive(Asset, TypePath)]
pub struct VideoAudio {
    queue: Arc<AudioQueue>,
    rate: u32,
    channels: u16,
}

pub struct VideoAudioDecoder {
    queue: Arc<AudioQueue>,
    rate: u32,
    channels: u16,
}

impl Iterator for VideoAudioDecoder {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        match self.queue.samples.lock().unwrap().pop_front() {
            Some(sample) => Some(sample as Sample),
            None if self.queue.finished.load(Ordering::Acquire) => None,
            // Le décodage a pris du retard : un instant de silence plutôt que la fin du son
            None => Some(0.0),
        }
    }
}

impl Source for VideoAudioDecoder {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> NonZero<u16> {
        NonZero::new(self.channels.max(1)).unwrap()
    }

    fn sample_rate(&self) -> NonZero<u32> {
        NonZero::new(self.rate.max(1)).unwrap()
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

impl Decodable for VideoAudio {
    type Decoder = VideoAudioDecoder;

    fn decoder(&self) -> VideoAudioDecoder {
        VideoAudioDecoder { queue: self.queue.clone(), rate: self.rate, channels: self.channels }
    }
}

struct Running {
    frames: Mutex<Receiver<Vec<u8>>>,
    stop: Arc<AtomicBool>,
    audio: Option<Entity>,
    queue: Arc<AudioQueue>,
    clock: f32,
    /// N° de la prochaine image reçue.
    next: u64,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.queue.finished.store(true, Ordering::Release);
    }
}

/// Le décodage : s'arrête à la fin de la vidéo ou quand plus personne ne lit les images.
fn decode_thread(mut video: Video, sender: SyncSender<Vec<u8>>, queue: Arc<AudioQueue>, stop: Arc<AtomicBool>) {
    let mut pcm = Vec::new();
    while !stop.load(Ordering::Acquire) && video.advance(&mut pcm) {
        queue.samples.lock().unwrap().extend(pcm.drain(..));
        let mut rgba = Vec::new();
        video.rgba(&mut rgba);
        if sender.send(rgba).is_err() {
            break;
        }
    }
    queue.finished.store(true, Ordering::Release);
}

pub struct VideoPlayer {
    bytes: Arc<[u8]>,
    info: Info,
    image: Handle<Image>,
    running: Option<Running>,
}

impl VideoPlayer {
    /// Ouvre la vidéo et montre sa première image.
    pub fn new(bytes: Arc<[u8]>, images: &mut Assets<Image>) -> Result<VideoPlayer, String> {
        let mut video = Video::open(bytes.clone())?;
        let info = video.info().clone();
        let mut rgba = Vec::new();
        video.advance(&mut Vec::new());
        video.rgba(&mut rgba);
        let mut image = Image::new(Extent3d { width: info.width, height: info.height, depth_or_array_layers: 1 }, TextureDimension::D2, rgba, TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::default());
        image.sampler = ImageSampler::nearest();
        Ok(VideoPlayer { bytes, info, image: images.add(image), running: None })
    }

    pub fn info(&self) -> &Info {
        &self.info
    }

    /// L'image où la vidéo est dessinée.
    pub fn image(&self) -> Handle<Image> {
        self.image.clone()
    }

    pub fn is_playing(&self) -> bool {
        self.running.is_some()
    }

    /// Reprend au début, avec le son s'il y en a un.
    pub fn play(&mut self, commands: &mut Commands, sources: &mut Assets<VideoAudio>) {
        self.stop(commands);
        let Ok(video) = Video::open(self.bytes.clone()) else { return };
        let queue = Arc::new(AudioQueue::default());
        let stop = Arc::new(AtomicBool::new(false));
        let (sender, frames) = sync_channel(AHEAD);
        let audio = (self.info.channels > 0).then(|| {
            let source = sources.add(VideoAudio { queue: queue.clone(), rate: self.info.sample_rate, channels: self.info.channels });
            commands.spawn((AudioPlayer::<VideoAudio>(source), PlaybackSettings::DESPAWN)).id()
        });
        let (thread_queue, thread_stop) = (queue.clone(), stop.clone());
        std::thread::spawn(move || decode_thread(video, sender, thread_queue, thread_stop));
        self.running = Some(Running { frames: Mutex::new(frames), stop, audio, queue, clock: 0.0, next: 0 });
    }

    /// Arrête la lecture et son son. L'image reste à l'écran.
    pub fn stop(&mut self, commands: &mut Commands) {
        if let Some(audio) = self.running.take().and_then(|r| r.audio) {
            commands.entity(audio).try_despawn();
        }
    }

    /// Fait avancer la vidéo de `delta` secondes. `false` quand elle ne joue pas (ou plus).
    pub fn update(&mut self, delta: f32, images: &mut Assets<Image>) -> bool {
        let Some(running) = &mut self.running else { return false };
        running.clock += delta;
        let wanted = (running.clock * self.info.fps) as u64;
        let mut newest = None;
        let mut ended = false;
        while running.next <= wanted {
            match running.frames.get_mut().unwrap().try_recv() {
                Ok(rgba) => {
                    newest = Some(rgba);
                    running.next += 1;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    ended = true;
                    break;
                }
            }
        }
        if let Some(rgba) = newest {
            if let Some(mut image) = images.get_mut(&self.image) {
                image.data = Some(rgba);
            }
        }
        if ended {
            // Le son finit seul, sa file vidée
            if let Some(mut running) = self.running.take() {
                running.audio = None;
            }
            return false;
        }
        true
    }
}
