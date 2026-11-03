//! Essai sans fenêtre : `vcheck <fichier> [dossier] [n images]`.
//! Décode tout, compte les images illisibles, écrit le son en WAV et quelques images en PNG.

use std::time::Instant;

use nds_video::Video;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = &args[1];
    let dir = args.get(2);
    let shots: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(6);
    let bytes = std::fs::read(path).unwrap();
    let mut video = match Video::open(bytes) {
        Ok(v) => v,
        Err(e) => {
            println!("{path}: OUVERTURE {e}");
            return;
        }
    };
    let info = video.info().clone();
    let start = Instant::now();
    let (mut count, mut pcm, mut rgba) = (0u32, Vec::new(), Vec::new());
    let every = (info.frames as usize / shots.max(1)).max(1);
    let stem = std::path::Path::new(path).file_stem().unwrap().to_string_lossy().to_string();
    while video.advance(&mut pcm) {
        if let Some(dir) = dir {
            if count as usize % every == every / 2 {
                video.rgba(&mut rgba);
                let image = image::RgbaImage::from_raw(info.width, info.height, rgba.clone()).unwrap();
                image.save(format!("{dir}/{stem}_{count:05}.png")).unwrap();
            }
        }
        count += 1;
    }
    let (errors, last) = video.errors();
    let peak = pcm.iter().fold(0f32, |a, s| a.max(s.abs()));
    let seconds = pcm.len() as f32 / (info.channels.max(1) as f32 * info.sample_rate.max(1) as f32);
    let ms = start.elapsed().as_secs_f32() * 1000.0 / count.max(1) as f32;
    println!(
        "{stem}: {} {}x{} {}/{} images, {:.1}/s ({:.1} s), {:.2} ms/image, erreurs {errors} {last:?}, son {} Hz x{} {seconds:.1} s crete {peak:.2}",
        info.format, info.width, info.height, count, info.frames, info.fps, info.seconds(), ms, info.sample_rate, info.channels
    );
    if let (Some(dir), true) = (dir, info.channels > 0) {
        let data: Vec<u8> = pcm.iter().flat_map(|s| ((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes()).collect();
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&info.channels.to_le_bytes());
        wav.extend_from_slice(&info.sample_rate.to_le_bytes());
        wav.extend_from_slice(&(info.sample_rate * info.channels as u32 * 2).to_le_bytes());
        wav.extend_from_slice(&(info.channels * 2).to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
        wav.extend_from_slice(&data);
        std::fs::write(format!("{dir}/{stem}.wav"), wav).unwrap();
    }
}
