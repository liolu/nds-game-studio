//! Jeux DS natifs : décompilation + runtime nitro-pc, sans émulateur.
//!
//! Chaque jeu est un programme `native/<code de la rom>.exe` à côté du studio (voir `native/README.md`).
//! Il tourne dans son propre processus 32 bits, car le code du jeu suppose des pointeurs 32 bits, et reçoit
//! la rom de l'utilisateur en argument. Il envoie ses deux écrans par sa sortie standard et lit les commandes
//! sur son entrée standard ; le moteur les affiche sur les écrans habituels (fenêtres liées ou séparées, F8, F9).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::input::{DsButton, DsInput, Stylus};
use crate::screens::Engine;
use crate::studio::{in_game, ActiveGame, ActiveRom, AppGameExt, StudioState};

const W: usize = 256;
const H: usize = 192;
/// Une image du jeu : les deux écrans en RGB 555 sur 16 bits.
const FRAME_BYTES: usize = W * H * 2 * 2;

/// Le dossier `native` à côté de l'exécutable.
pub fn native_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("NDSGS_NATIVE_DIR") {
        return PathBuf::from(dir);
    }
    let exe = std::env::current_exe().unwrap_or_default();
    exe.parent().unwrap_or(Path::new(".")).join("native")
}

fn module_path(code: &str) -> PathBuf {
    native_dir().join(format!("{code}.exe"))
}

pub struct NativeGamesPlugin;

impl Plugin for NativeGamesPlugin {
    fn build(&self, app: &mut App) {
        let Ok(entries) = std::fs::read_dir(native_dir()) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")) {
                continue;
            }
            let Some(code) = path.file_stem().and_then(|s| s.to_str()) else { continue };
            if code.len() != 4 {
                continue;
            }
            let spec = format!("id = \"native-{code}\"\nname = \"{code}\"\ngame_code = \"{code}\"\n");
            app.add_ds_game(&spec, start_module);
        }
        app.add_systems(Update, (show_frames, send_input).run_if(resource_exists::<Running>))
            .add_systems(OnExit(StudioState::Playing), stop_module);
    }
}

#[derive(Resource)]
struct Running {
    child: Child,
    stdin: Option<ChildStdin>,
    latest: Arc<Mutex<Option<Vec<u8>>>>,
    top: Handle<Image>,
    bottom: Handle<Image>,
    sent: [u8; 8],
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn screen_image(images: &mut Assets<Image>) -> Handle<Image> {
    let size = Extent3d { width: W as u32, height: H as u32, depth_or_array_layers: 1 };
    let mut image = Image::new_fill(
        size,
        TextureDimension::D2,
        &[0, 0, 0, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    image.sampler = ImageSampler::nearest();
    images.add(image)
}

fn start_module(mut commands: Commands, mut images: ResMut<Assets<Image>>, rom: Option<Res<ActiveRom>>, active: Res<ActiveGame>) {
    let Some(rom) = rom else {
        error!("jeu natif lance sans rom");
        commands.set_state(StudioState::Menu);
        return;
    };
    let id = active.0.clone().unwrap_or_default();
    let code = id.trim_start_matches("native-").to_string();
    let saves = crate::save::data_dir().join("saves").join(&id);
    let _ = std::fs::create_dir_all(&saves);
    let log = std::fs::File::create(saves.join("runtime.log")).map(Stdio::from).unwrap_or_else(|_| Stdio::null());

    let mut command = Command::new(module_path(&code));
    command.arg(&rom.0).arg("--pipe").arg("--save").arg(saves.join("backup.sav")).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(log);
    if std::env::var_os("NDSGS_NATIVE_MUTE").is_some() {
        command.arg("--mute");
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            error!("{code} : le module natif ne demarre pas : {e}");
            commands.set_state(StudioState::Menu);
            return;
        }
    };

    let latest = Arc::new(Mutex::new(None));
    if let Some(mut stdout) = child.stdout.take() {
        let latest = latest.clone();
        std::thread::spawn(move || {
            let mut raw = vec![0u8; FRAME_BYTES];
            while stdout.read_exact(&mut raw).is_ok() {
                let rgba: Vec<u8> = raw
                    .chunks_exact(2)
                    .flat_map(|p| {
                        let c = u16::from_le_bytes([p[0], p[1]]) as u32;
                        let scale = |v: u32| ((v & 31) * 255 / 31) as u8;
                        [scale(c), scale(c >> 5), scale(c >> 10), 255]
                    })
                    .collect();
                *latest.lock().unwrap() = Some(rgba);
            }
        });
    }

    let top = screen_image(&mut images);
    let bottom = screen_image(&mut images);
    let size = Vec2::new(W as f32, H as f32);
    for (engine, image) in [(Engine::Main, &top), (Engine::Sub, &bottom)] {
        commands.spawn((Sprite { image: image.clone(), custom_size: Some(size), ..default() }, engine.layers(), in_game()));
    }
    let stdin = child.stdin.take();
    commands.insert_resource(Running { child, stdin, latest, top, bottom, sent: [0xFF; 8] });
}

fn show_frames(mut commands: Commands, mut running: ResMut<Running>, mut images: ResMut<Assets<Image>>) {
    if let Ok(Some(status)) = running.child.try_wait() {
        if status.success() {
            info!("jeu natif termine");
        } else {
            error!("le jeu natif s'est arrete : {status}");
        }
        commands.set_state(StudioState::Menu);
        return;
    }
    let Some(rgba) = running.latest.lock().unwrap().take() else { return };
    let (top, bottom) = rgba.split_at(W * H * 4);
    for (handle, pixels) in [(running.top.clone(), top), (running.bottom.clone(), bottom)] {
        if let Some(mut image) = images.get_mut(&handle) {
            image.data = Some(pixels.to_vec());
        }
    }
}

fn send_input(mut running: ResMut<Running>, input: Res<DsInput>, stylus: Res<Stylus>) {
    // Bits du registre KEYINPUT du DS, puis X et Y
    const ORDER: [DsButton; 12] = [
        DsButton::A,
        DsButton::B,
        DsButton::Select,
        DsButton::Start,
        DsButton::Right,
        DsButton::Left,
        DsButton::Up,
        DsButton::Down,
        DsButton::R,
        DsButton::L,
        DsButton::X,
        DsButton::Y,
    ];
    let mut keys = 0u16;
    for (bit, button) in ORDER.iter().enumerate() {
        if input.pressed(*button) {
            keys |= 1 << bit;
        }
    }
    let touch = stylus.touch().filter(|p| p.x >= 0.0 && p.y >= 0.0 && p.x < W as f32 && p.y < H as f32);
    let (down, x, y) = touch.map_or((0u16, 0u16, 0u16), |p| (1, p.x as u16, p.y as u16));
    let mut packet = [0u8; 8];
    packet[0..2].copy_from_slice(&keys.to_le_bytes());
    packet[2..4].copy_from_slice(&down.to_le_bytes());
    packet[4..6].copy_from_slice(&x.to_le_bytes());
    packet[6..8].copy_from_slice(&y.to_le_bytes());
    if packet != running.sent {
        running.sent = packet;
        if let Some(stdin) = running.stdin.as_mut() {
            let _ = stdin.write_all(&packet).and_then(|_| stdin.flush());
        }
    }
}

fn stop_module(mut commands: Commands) {
    commands.remove_resource::<Running>();
}
