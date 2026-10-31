//! Essai de l'écriture VX sans FFmpeg : `vxroundtrip <fichier.vx> <sortie.vx> [son]`.
//! Relit la vidéo, la réécrit depuis ses propres images, compare. Avec `son`, le son est réécrit lui aussi au lieu d'être gardé.

use nds_video::{Video, vx};

fn frames_of(bytes: &[u8]) -> (Vec<Vec<u8>>, Vec<f32>) {
    let mut video = Video::open(bytes.to_vec()).unwrap();
    let (mut pcm, mut rgba, mut frames) = (Vec::new(), Vec::new(), Vec::new());
    while video.advance(&mut pcm) {
        video.rgba(&mut rgba);
        frames.push(rgba.clone());
    }
    assert_eq!(video.errors(), (0, None));
    (frames, pcm)
}

fn psnr(a: &[u8], b: &[u8]) -> f32 {
    let total: u64 = a.iter().zip(b).map(|(x, y)| (*x as i64 - *y as i64).unsigned_abs().pow(2)).sum();
    10.0 * (255f32 * 255.0 / (total as f32 / a.len() as f32).max(0.0001)).log10()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let original = std::fs::read(&args[1]).unwrap();
    let (frames, pcm) = frames_of(&original);
    let sound: Vec<i16> = pcm.iter().map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16).collect();
    let rewrite_sound = args.get(3).is_some_and(|a| a == "son");
    let start = std::time::Instant::now();
    let (bytes, written) = vx::encode_like(&original, frames.iter().map(|f| Ok(f.clone())), rewrite_sound.then_some(&sound[..])).unwrap();
    let ms = start.elapsed().as_secs_f32() * 1000.0 / frames.len().max(1) as f32;
    std::fs::write(&args[2], &bytes).unwrap();
    let mut back = Video::open(bytes.clone()).unwrap();
    let (mut new_pcm, mut rgba, mut index) = (Vec::new(), Vec::new(), 0);
    let (mut sum, mut worst) = (0f32, f32::MAX);
    while back.advance(&mut new_pcm) {
        back.rgba(&mut rgba);
        let quality = psnr(&rgba, &frames[index]);
        sum += quality;
        worst = worst.min(quality);
        index += 1;
    }
    let noise: f64 = pcm.iter().zip(&new_pcm).map(|(a, b)| ((a - b) as f64).powi(2)).sum();
    let power: f64 = pcm.iter().map(|a| (*a as f64).powi(2)).sum();
    println!(
        "{index}/{} images, erreurs {:?}, {} -> {} octets, niveau max {}, image {:.1} dB moy {:.1} dB pire, son {} -> {} ech {:.1} dB, {:.1} ms/image",
        frames.len(),
        back.errors(),
        original.len(),
        bytes.len(),
        written.worst_level,
        sum / index.max(1) as f32,
        worst,
        pcm.len(),
        new_pcm.len(),
        10.0 * (power / noise.max(1e-9)).log10(),
        ms
    );
}
