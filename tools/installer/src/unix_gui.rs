//! Installeur graphique pour Linux et macOS (eframe/egui).

use eframe::egui;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ndsgs_common::{exe_name, GAME_BIN, LAUNCHER_BIN};

pub fn run() {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Nds Game Studio Installer")
            .with_inner_size([500.0, 300.0])
            .with_resizable(false),
        centered: true,
        ..Default::default()
    };
    let _ = eframe::run_native(
        "Nds Game Studio Installer",
        options,
        Box::new(|_cc| Ok(Box::new(InstallerApp::new()))),
    );
}

#[derive(Default)]
struct Progress {
    downloaded: AtomicU64,
    total: AtomicU64,
    done: AtomicBool,
    error: Mutex<Option<String>>,
    /// Raccourcis créés, affichés à la fin.
    shortcuts: Mutex<Vec<PathBuf>>,
}

struct InstallerApp {
    path: String,
    shortcuts: bool,
    progress: Option<Arc<Progress>>,
    launch_error: Option<String>,
}

impl InstallerApp {
    fn new() -> Self {
        let default_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Games")
            .join("Nds Game Studio");
        Self {
            path: default_dir.to_string_lossy().into_owned(),
            shortcuts: true,
            progress: None,
            launch_error: None,
        }
    }

    fn install_dir(&self) -> PathBuf {
        expand_tilde(self.path.trim())
    }

    fn start_install(&mut self, ctx: &egui::Context) {
        let progress = Arc::new(Progress::default());
        self.progress = Some(progress.clone());
        let dir = self.install_dir();
        let shortcuts = self.shortcuts;
        let ctx = ctx.clone();

        std::thread::spawn(move || {
            let notify = || ctx.request_repaint();
            let result = crate::install::download_and_extract(
                &dir,
                &progress.downloaded,
                &progress.total,
                &notify,
            );
            match result {
                Ok(()) => {
                    if shortcuts {
                        match create_platform_shortcuts(&dir, &target_exe(&dir)) {
                            Ok(created) => *progress.shortcuts.lock().unwrap() = created,
                            Err(e) => eprintln!("Cannot create the shortcuts: {}", e),
                        }
                    }
                }
                Err(e) => *progress.error.lock().unwrap() = Some(e),
            }
            progress.done.store(true, Ordering::Relaxed);
            ctx.request_repaint();
        });
    }
}

impl eframe::App for InstallerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Nds Game Studio Installer");
            ui.add_space(12.0);

            let busy = self.progress.as_ref().is_some_and(|p| !p.done.load(Ordering::Relaxed));
            let finished = self.progress.as_ref().is_some_and(|p| {
                p.done.load(Ordering::Relaxed) && p.error.lock().unwrap().is_none()
            });

            ui.add_enabled_ui(!busy && !finished, |ui| {
                ui.label("Install folder:");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.path).desired_width(360.0));
                    if ui.button("Parcourir").clicked() {
                        if let Some(dir) = rfd::FileDialog::new()
                            .set_title("Choose the install folder")
                            .pick_folder()
                        {
                            self.path = dir.join("Nds Game Studio").to_string_lossy().into_owned();
                        }
                    }
                });
                ui.add_space(6.0);
                ui.checkbox(&mut self.shortcuts, shortcut_label());
                ui.add_space(10.0);
                ui.vertical_centered(|ui| {
                    if ui.add_sized([150.0, 32.0], egui::Button::new("Installer")).clicked() {
                        if self.path.trim().is_empty() {
                            self.launch_error = Some("Please choose an install folder.".into());
                        } else {
                            self.launch_error = None;
                            self.start_install(ctx);
                        }
                    }
                });
            });

            ui.add_space(12.0);

            if let Some(p) = &self.progress {
                let done = p.downloaded.load(Ordering::Relaxed);
                let total = p.total.load(Ordering::Relaxed);
                let error = p.error.lock().unwrap().clone();

                if let Some(e) = error {
                    ui.colored_label(egui::Color32::LIGHT_RED, format!("The installation failed:\n{}", e));
                } else if p.done.load(Ordering::Relaxed) {
                    ui.add(egui::ProgressBar::new(1.0).text("Installation complete!"));
                    for s in p.shortcuts.lock().unwrap().iter() {
                        ui.small(format!("Shortcut created: {}", s.display()));
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Start").clicked() {
                            let dir = self.install_dir();
                            match std::process::Command::new(target_exe(&dir)).current_dir(&dir).spawn() {
                                Ok(_) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                                Err(e) => self.launch_error = Some(format!("Cannot start it: {}", e)),
                            }
                        }
                        if ui.button("Fermer").clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                } else if total > 0 {
                    let frac = done as f32 / total as f32;
                    ui.add(egui::ProgressBar::new(frac).text(format!(
                        "Telechargement... {:.1} MB / {:.1} MB ({:.0}%)",
                        done as f64 / 1_048_576.0,
                        total as f64 / 1_048_576.0,
                        frac * 100.0
                    )));
                } else {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Getting information...");
                    });
                }
            } else {
                ui.label("Ready to install");
            }

            if let Some(e) = &self.launch_error {
                ui.colored_label(egui::Color32::LIGHT_RED, e);
            }
        });
    }
}

fn target_exe(install_dir: &Path) -> PathBuf {
    let launcher = install_dir.join(exe_name(LAUNCHER_BIN));
    if launcher.exists() {
        launcher
    } else {
        install_dir.join(exe_name(GAME_BIN))
    }
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}

#[cfg(target_os = "macos")]
fn shortcut_label() -> &'static str {
    "Create a shortcut on the Desktop and in Applications"
}

#[cfg(not(target_os = "macos"))]
fn shortcut_label() -> &'static str {
    "Add to the applications menu and the Desktop"
}

/// macOS : petit bundle `Nds Game Studio.app` dans ~/Applications (visible dans
/// le Launchpad / Spotlight) + un alias sur le Bureau. Le bundle se contente
/// de lancer le launcher installé, sans ouvrir de Terminal.
#[cfg(target_os = "macos")]
fn create_platform_shortcuts(_install_dir: &Path, target: &Path) -> io::Result<Vec<PathBuf>> {
    let mut created = Vec::new();
    let Some(home) = dirs::home_dir() else { return Ok(created) };

    let app = home.join("Applications").join("Nds Game Studio.app");
    let macos_dir = app.join("Contents").join("MacOS");
    fs::create_dir_all(&macos_dir)?;
    fs::write(
        app.join("Contents").join("Info.plist"),
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Nds Game Studio</string>
    <key>CFBundleDisplayName</key><string>Nds Game Studio</string>
    <key>CFBundleIdentifier</key><string>io.github.{}.{}</string>
    <key>CFBundleVersion</key><string>{}</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleExecutable</key><string>Nds Game Studio</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
"#,
            ndsgs_common::REPO_OWNER,
            ndsgs_common::REPO_NAME,
            ndsgs_common::VERSION
        ),
    )?;
    write_executable(
        &macos_dir.join("Nds Game Studio"),
        &format!("#!/bin/bash\ncd \"$(dirname \"{0}\")\"\nexec \"{0}\"\n", target.display()),
    )?;
    created.push(app.clone());

    if let Some(desktop) = dirs::desktop_dir() {
        let link = desktop.join("Nds Game Studio.app");
        let _ = fs::remove_file(&link);
        if std::os::unix::fs::symlink(&app, &link).is_ok() {
            created.push(link);
        }
    }
    Ok(created)
}

/// Linux : entrée `.desktop` (menu des applications + Bureau).
#[cfg(not(target_os = "macos"))]
fn create_platform_shortcuts(install_dir: &Path, target: &Path) -> io::Result<Vec<PathBuf>> {
    let entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Nds Game Studio\n\
         Comment=Voxel Universe\n\
         Exec=\"{}\"\n\
         Path={}\n\
         Terminal=false\n\
         Categories=Game;Simulation;\n",
        target.display(),
        install_dir.display()
    );
    let mut created = Vec::new();
    if let Some(data) = dirs::data_dir() {
        let apps = data.join("applications");
        fs::create_dir_all(&apps)?;
        let path = apps.join("nds-game-studio.desktop");
        write_executable(&path, &entry)?;
        created.push(path);
    }
    if let Some(desktop) = dirs::desktop_dir() {
        if desktop.is_dir() {
            let path = desktop.join("nds-game-studio.desktop");
            write_executable(&path, &entry)?;
            created.push(path);
        }
    }
    Ok(created)
}

fn write_executable(path: &Path, content: &str) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, content)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}
