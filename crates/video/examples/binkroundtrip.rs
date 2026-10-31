//! Essai de l'écriture Bink sans FFmpeg : `binkroundtrip <fichier.bik> <sortie.bik> [son]`.
//! Relit la vidéo, la réécrit depuis ses propres images, compare. Avec `son`, le son est réécrit lui aussi au lieu d'être gardé.
//! Avec `modif` : couleurs inversées, un carré de bruit qui se déplace et un dégradé qui défile, pour forcer tous les modes.

use nds_video::{Video, bink};

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

/// Une image bien différente de l'original : couleurs inversées, bruit, dégradé.
fn modified(frame: &[u8], index: usize, width: usize) -> Vec<u8> {
    let mut out = frame.to_vec();
    let height = frame.len() / 4 / width;
    let mut seed = 12345u32.wrapping_add(index as u32);
    for y in 0..height {
        for x in 0..width {
            let px = &mut out[(y * width + x) * 4..][..3];
            for v in px.iter_mut() {
                *v = 255 - *v;
            }
            let (bx, by) = (index * 3 % width, index * 2 % height);
            if x.wrapping_sub(bx) < 64 && y.wrapping_sub(by) < 48 {
                seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                px[0] = (seed >> 16) as u8;
                px[1] = (seed >> 24) as u8;
                px[2] = (x * 3 + index * 5) as u8;
            }
            if y < 16 {
                px[1] = ((x + index * 4) % 256) as u8;
            }
        }
    }
    out
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
    let rewrite_sound = args.iter().skip(3).any(|a| a == "son");
    let width = Video::open(original.clone()).unwrap().info().width as usize;
    let frames: Vec<Vec<u8>> = if args.iter().skip(3).any(|a| a == "modif") { frames.iter().enumerate().map(|(i, f)| modified(f, i, width)).collect() } else { frames };
    let start = std::time::Instant::now();
    let (bytes, written) = bink::encode_like(&original, frames.iter().map(|f| Ok(f.clone())), rewrite_sound.then_some(&sound[..])).unwrap();
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
