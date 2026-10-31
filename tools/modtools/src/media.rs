//! FFmpeg, livré à côté de l'outil : MP3, MP4 et OGV, et lecture de tout son à réimporter. Sans lui, PNG, OBJ, WAV et TXT
//! marchent quand même.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use nds_video::Video;

use crate::sound;

fn ffmpeg() -> Option<&'static PathBuf> {
    static FOUND: OnceLock<Option<PathBuf>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            let name = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
            let beside = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.join(name)));
            let chosen = std::env::var_os("NDSGS_FFMPEG").map(PathBuf::from);
            let on_path = std::env::var_os("PATH").into_iter().flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).map(|dir| dir.join(name));
            chosen.into_iter().chain(beside).chain(on_path).find(|p| p.is_file())
        })
        .as_ref()
}

pub fn available() -> bool {
    ffmpeg().is_some()
}

fn command() -> Option<Command> {
    let mut command = Command::new(ffmpeg()?);
    command.arg("-hide_banner").arg("-loglevel").arg("error").arg("-y");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    Some(command)
}

fn has_encoder(name: &str) -> bool {
    static LIST: OnceLock<String> = OnceLock::new();
    let list = LIST.get_or_init(|| {
        let Some(mut command) = command() else { return String::new() };
        command.arg("-encoders").stdin(Stdio::null()).stderr(Stdio::null()).output().map_or(String::new(), |o| String::from_utf8_lossy(&o.stdout).into_owned())
    });
    list.split_whitespace().any(|word| word == name)
}

/// Écrit `out` en MP3 depuis un WAV en mémoire.
pub fn wav_to_mp3(wav: &[u8], out: &Path) -> Result<(), String> {
    let mut command = command().ok_or("FFmpeg absent")?;
    command.args(["-f", "wav", "-i", "-", "-c:a", "libmp3lame", "-q:a", "2"]).arg(out);
    pipe(command, wav)
}

fn pipe(mut command: Command, input: &[u8]) -> Result<(), String> {
    let mut child = command.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    let mut stdin = child.stdin.take().ok_or("FFmpeg sans entree")?;
    let bytes = input.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&bytes);
    });
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    let _ = writer.join();
    if output.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&output.stderr).trim().to_string()) }
}

/// Le son d'un fichier, à cette fréquence et avec ces voies. Un WAV se lit sans FFmpeg ; avec FFmpeg, la conversion est meilleure.
pub fn read_sound(path: &Path, rate: u32, channels: u16) -> Result<Vec<i16>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let wav = sound::read_wav(&bytes);
    if let Some((r, c, samples)) = &wav {
        if *r == rate && *c == channels {
            return Ok(samples.clone());
        }
    }
    if let Some(mut command) = command() {
        let args = ["-f".to_string(), "s16le".to_string(), "-ar".to_string(), rate.to_string(), "-ac".to_string(), channels.to_string(), "-".to_string()];
        let output = command.arg("-i").arg(path).args(args).stdin(Stdio::null()).output().map_err(|e| e.to_string())?;
        if output.status.success() {
            return Ok(output.stdout.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect());
        }
    }
    match wav {
        Some((r, c, samples)) => Ok(sound::convert(&samples, r, c, rate, channels)),
        None => Err("ce format de son a besoin de FFmpeg : exporter en WAV, ou installer FFmpeg".into()),
    }
}

/// Lit toute une vidéo et l'écrit en MP4 (Blender, lecteurs...) et en OGV si FFmpeg sait le faire (Godot ne lit que celui-là).
pub fn video_to_files(bytes: Vec<u8>, base: &Path) -> Result<Vec<PathBuf>, String> {
    let mut video = Video::open(bytes)?;
    let info = video.info().clone();
    let (width, height) = (info.width, info.height);
    let codec = ["libx264", "libopenh264"].into_iter().find(|c| has_encoder(c)).ok_or("FFmpeg sans encodeur H.264")?;
    // Le son n'est connu qu'en lisant la vidéo : d'abord les images, le son ensuite
    let silent = base.with_extension("tmp.mp4");
    let mut images = command().ok_or("FFmpeg absent")?;
    let size = format!("{width}x{height}");
    let fps = format!("{}", info.fps.max(1.0));
    images.args(["-f", "rawvideo", "-pix_fmt", "rgba", "-s", &size, "-r", &fps, "-i", "-", "-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2", "-pix_fmt", "yuv420p", "-c:v", codec]).arg(&silent);
    let mut child = images.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| e.to_string())?;
    let mut stdin = child.stdin.take().ok_or("FFmpeg sans entree")?;
    let (mut frame, mut pcm) = (Vec::new(), Vec::new());
    let mut failed = None;
    while video.advance(&mut pcm) {
        video.rgba(&mut frame);
        if let Err(e) = stdin.write_all(&frame) {
            failed = Some(e.to_string());
            break;
        }
    }
    drop(stdin);
    let status = child.wait().map_err(|e| e.to_string())?;
    if let Some(e) = failed.filter(|_| !status.success()) {
        let _ = std::fs::remove_file(&silent);
        return Err(e);
    }
    if !status.success() {
        let _ = std::fs::remove_file(&silent);
        return Err(format!("FFmpeg : {status}"));
    }
    let wav_path = base.with_extension("tmp.wav");
    let has_sound = !pcm.is_empty() && info.sample_rate != 0;
    if has_sound {
        let samples = pcm.iter().map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16).collect();
        let sound = nds_nitro::sadl::Sound { rate: info.sample_rate, channels: info.channels.max(1), samples };
        std::fs::write(&wav_path, sound.wav()).map_err(|e| e.to_string())?;
    }
    let mut made = Vec::new();
    let mut encode = |extension: &str, video_args: &[&str], audio_args: &[&str]| -> Result<(), String> {
        let out = base.with_extension(extension);
        let mut command = command().ok_or("FFmpeg absent")?;
        command.arg("-i").arg(&silent);
        if has_sound {
            command.arg("-i").arg(&wav_path).args(audio_args);
        }
        command.args(video_args).arg(&out);
        let output = command.stdin(Stdio::null()).output().map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }
        made.push(out);
        Ok(())
    };
    let result = encode("mp4", &["-c:v", "copy"], &["-c:a", "aac", "-shortest"]);
    if result.is_ok() && has_encoder("libtheora") {
        let audio: &[&str] = if has_encoder("libvorbis") { &["-c:a", "libvorbis", "-shortest"] } else { &["-an"] };
        let _ = encode("ogv", &["-c:v", "libtheora", "-q:v", "7"], audio);
    }
    let _ = std::fs::remove_file(&silent);
    let _ = std::fs::remove_file(&wav_path);
    result.map(|_| made)
}
