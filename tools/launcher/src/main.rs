#![windows_subsystem = "windows"]

use eframe::egui;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ndsgs_common::{exe_name, Channel, VersionInfo, CHANNEL, GAME_BIN, LAUNCHER_BIN};

fn main() -> eframe::Result {
    // Raccourcis qui ouvriraient le jeu directement : on les redirige vers ce launcher
    std::thread::spawn(ndsgs_common::repair_shortcuts);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Nds Game Studio Launcher")
            .with_inner_size([460.0, 400.0])
            .with_resizable(false),
        centered: true,
        ..Default::default()
    };
    eframe::run_native(
        "Nds Game Studio Launcher",
        options,
        Box::new(|cc| Ok(Box::new(LauncherApp::new(cc.egui_ctx.clone())))),
    )
}

// ─────────────────────────────────────────────────────────────────────────
//  Canal choisi (stable / experimentale), mémorisé dans launcher.json
// ─────────────────────────────────────────────────────────────────────────

#[derive(serde::Serialize, serde::Deserialize)]
struct LauncherConfig {
    channel: String,
    /// Version stable choisie à la main : pas de mise à jour automatique tant qu'elle est installée.
    #[serde(default)]
    pinned: Option<String>,
}

fn config_path() -> PathBuf {
    ndsgs_common::exe_dir().join("launcher.json")
}

fn load_channel() -> Channel {
    let saved = fs::read_to_string(config_path())
        .ok()
        .and_then(|s| serde_json::from_str::<LauncherConfig>(&s).ok())
        .map(|c| Channel::from_name(&c.channel));
    match saved {
        Some(c @ (Channel::Stable | Channel::Experimental)) => c,
        // Par défaut : le canal de la version installée (stable si compilation locale)
        _ if CHANNEL == Channel::Experimental => Channel::Experimental,
        _ => Channel::Stable,
    }
}

fn load_pinned() -> Option<String> {
    fs::read_to_string(config_path()).ok().and_then(|s| serde_json::from_str::<LauncherConfig>(&s).ok()).and_then(|c| c.pinned)
}

fn save_config(channel: Channel, pinned: Option<String>) {
    let cfg = LauncherConfig { channel: channel.name().into(), pinned };
    if let Ok(json) = serde_json::to_string_pretty(&cfg) {
        let _ = fs::write(config_path(), json);
    }
}

// ─────────────────────────────────────────────────────────────────────────
//  État partagé entre la fenêtre et le thread de mise à jour
// ─────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
enum Stage {
    Checking,
    /// Une autre version est disponible sur le canal choisi.
    Available(VersionInfo),
    /// Rien à installer : le joueur peut lancer le jeu.
    ReadyToPlay(String),
    Downloading,
    Extracting,
    /// Mise à jour installée : on lance le jeu.
    Installed(String),
    Failed(String),
}

struct Shared {
    stage: Mutex<Stage>,
    downloaded: AtomicU64,
    total: AtomicU64,
    /// Incrémenté à chaque vérification : une vérification périmée
    /// (changement de canal entre-temps) est ignorée.
    check_id: AtomicU64,
    /// Liste des versions publiées (chargée à la première ouverture du sélecteur).
    releases: Mutex<ReleaseList>,
}

enum ReleaseList {
    NotLoaded,
    Loading,
    Loaded(Vec<VersionInfo>),
    Failed(String),
}

impl Shared {
    fn set(&self, stage: Stage) {
        *self.stage.lock().unwrap() = stage;
    }
}

struct LauncherApp {
    shared: Arc<Shared>,
    ctx: egui::Context,
    install_dir: PathBuf,
    game_exe: PathBuf,
    channel: Channel,
    launched: bool,
    /// La mise à jour automatique a déjà été tentée (pas de boucle en cas d'échec).
    auto_update_tried: bool,
    launch_error: Option<String>,
    /// Version choisie dans le sélecteur (tag sans le « v »).
    picked: Option<String>,
    /// Version verrouillée (voir `LauncherConfig::pinned`).
    pinned: Option<String>,
    /// `--versions` (Options > Launcher du studio) : rester ouvert pour choisir une version.
    /// Sinon, rien à installer = le studio s'ouvre tout de suite : c'est lui qui montre les jeux.
    stay_open: bool,
}

impl LauncherApp {
    fn new(ctx: egui::Context) -> Self {
        let install_dir = ndsgs_common::exe_dir();
        // Reste d'une mise à jour précédente du launcher (renommé avant d'être remplacé)
        let _ = fs::remove_file(install_dir.join(format!("{}.old", exe_name(LAUNCHER_BIN))));
        let game_exe = install_dir.join(exe_name(GAME_BIN));
        let shared = Arc::new(Shared {
            stage: Mutex::new(Stage::Checking),
            downloaded: AtomicU64::new(0),
            total: AtomicU64::new(0),
            check_id: AtomicU64::new(0),
            releases: Mutex::new(ReleaseList::NotLoaded),
        });
        let app = Self {
            shared,
            ctx,
            install_dir,
            game_exe,
            channel: load_channel(),
            launched: false,
            auto_update_tried: false,
            launch_error: None,
            picked: None,
            pinned: load_pinned(),
            stay_open: std::env::args().any(|a| a == "--versions"),
        };
        app.start_check();
        app
    }

    fn start_check(&self) {
        let shared = self.shared.clone();
        let ctx = self.ctx.clone();
        let channel = self.channel;
        let pinned = self.pinned.clone();
        let id = shared.check_id.fetch_add(1, Ordering::SeqCst) + 1;
        shared.set(Stage::Checking);
        std::thread::spawn(move || {
            let is_pinned = channel == Channel::Stable
                && CHANNEL == Channel::Stable
                && pinned.as_deref() == Some(ndsgs_common::VERSION);
            let stage = if is_pinned {
                Stage::ReadyToPlay(format!("Version v{} verrouillee (choisie manuellement)", ndsgs_common::VERSION))
            } else {
                match ndsgs_common::fetch_channel(channel, 5) {
                Ok(info) if ndsgs_common::should_install(channel, &info) => Stage::Available(info),
                Ok(_) => Stage::ReadyToPlay("Le jeu est a jour".into()),
                Err(e) => Stage::ReadyToPlay(format!("Hors-ligne ou aucune version disponible ({})", e)),
                }
            };
            if shared.check_id.load(Ordering::SeqCst) == id {
                shared.set(stage);
            }
            ctx.request_repaint();
        });
    }

    fn load_releases(&self) {
        let shared = self.shared.clone();
        let ctx = self.ctx.clone();
        *shared.releases.lock().unwrap() = ReleaseList::Loading;
        std::thread::spawn(move || {
            let result = match ndsgs_common::fetch_releases(10) {
                Ok(list) => ReleaseList::Loaded(list),
                Err(e) => ReleaseList::Failed(e),
            };
            *shared.releases.lock().unwrap() = result;
            ctx.request_repaint();
        });
    }

    fn start_update(&self, info: VersionInfo) {
        let shared = self.shared.clone();
        let ctx = self.ctx.clone();
        let dir = self.install_dir.clone();
        shared.downloaded.store(0, Ordering::Relaxed);
        shared.total.store(0, Ordering::Relaxed);
        shared.set(Stage::Downloading);
        std::thread::spawn(move || {
            let result = download_and_apply(&info, &dir, &shared, &ctx);
            shared.set(match result {
                Ok(()) => Stage::Installed(format!("{} installee", info.label())),
                Err(e) => Stage::Failed(e.to_string()),
            });
            ctx.request_repaint();
        });
    }

    /// Lance le jeu et ferme le launcher.
    fn launch_game(&mut self, ctx: &egui::Context) {
        if self.launched {
            return;
        }
        if !self.game_exe.exists() {
            self.launch_error = Some(format!("{} introuvable", self.game_exe.display()));
            return;
        }
        match Command::new(&self.game_exe).current_dir(&self.install_dir).spawn() {
            Ok(_) => {
                self.launched = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(e) => self.launch_error = Some(format!("Impossible de lancer le jeu: {}", e)),
        }
    }
}

impl eframe::App for LauncherApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let ctx = ctx.clone();
        let stage = self.shared.stage.lock().unwrap().clone();
        let busy = matches!(stage, Stage::Downloading | Stage::Extracting | Stage::Installed(_));

        egui::CentralPanel::default().show(&ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(8.0);
                ui.heading("Nds Game Studio");
                // Sur quelle version on se trouve : canal, numéro et, pour l'experimentale, le build
                let (badge, color) = match CHANNEL {
                    Channel::Stable => ("STABLE", egui::Color32::from_rgb(90, 200, 120)),
                    Channel::Experimental => ("EXPERIMENTAL", egui::Color32::from_rgb(240, 170, 60)),
                    Channel::Dev => ("LOCAL", egui::Color32::GRAY),
                };
                ui.horizontal(|ui| {
                    ui.label("Installe :");
                    ui.label(egui::RichText::new(badge).strong().color(color));
                    ui.label(ndsgs_common::installed_label());
                });
                ui.small("Nouvelle numerotation : AA.MM.JJ_HH:MM_vVERSION.REVISION (heure UTC)");
                ui.add_space(8.0);

                // ── Choix du canal ──
                ui.add_enabled_ui(!busy, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Version :");
                        let before = self.channel;
                        ui.radio_value(&mut self.channel, Channel::Stable, "Stable")
                            .on_hover_text("Versions publiees, testees");
                        ui.radio_value(&mut self.channel, Channel::Experimental, "Experimentale")
                            .on_hover_text("Derniere version en cours de developpement (peut contenir des bugs)");
                        if self.channel != before {
                            self.pinned = None;
                            save_config(self.channel, None);
                            self.start_check();
                        }
                    });
                });

                // ── Choix d'une version précise (retour à une ancienne version possible) ──
                ui.add_enabled_ui(!busy, |ui| {
                    let header = egui::CollapsingHeader::new("Choisir une version precise").show(ui, |ui| {
                        let state = std::mem::replace(&mut *self.shared.releases.lock().unwrap(), ReleaseList::Loading);
                        match state {
                            ReleaseList::NotLoaded => {
                                // Le chargement démarre après cette image
                                *self.shared.releases.lock().unwrap() = ReleaseList::NotLoaded;
                                self.load_releases();
                            }
                            ReleaseList::Loading => {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label("Chargement des versions...");
                                });
                            }
                            ReleaseList::Failed(e) => {
                                ui.colored_label(egui::Color32::LIGHT_RED, format!("Liste indisponible : {e}"));
                                let retry = ui.small_button("Reessayer").clicked();
                                *self.shared.releases.lock().unwrap() = ReleaseList::Failed(e);
                                if retry {
                                    self.load_releases();
                                }
                            }
                            ReleaseList::Loaded(list) => {
                                let current = format!("v{}", ndsgs_common::VERSION);
                                let label_of = |v: &VersionInfo| {
                                    let tag = format!("v{}", v.version);
                                    if CHANNEL == Channel::Stable && tag == current { format!("{tag} (installee)") } else { tag }
                                };
                                let chosen = self.picked.clone().or_else(|| list.first().map(|v| v.version.clone()));
                                let shown = list
                                    .iter()
                                    .find(|v| Some(&v.version) == chosen.as_ref())
                                    .map(|v| label_of(v))
                                    .unwrap_or_else(|| "-".into());
                                egui::ComboBox::from_id_salt("version_picker").selected_text(shown).show_ui(ui, |ui| {
                                    for v in &list {
                                        let selected = Some(&v.version) == chosen.as_ref();
                                        if ui.selectable_label(selected, label_of(v)).clicked() {
                                            self.picked = Some(v.version.clone());
                                        }
                                    }
                                });
                                if let Some(v) = list.iter().find(|v| Some(&v.version) == chosen.as_ref()) {
                                    if !v.release_notes.trim().is_empty() {
                                        let notes: String = v.release_notes.chars().take(300).collect();
                                        ui.small(notes);
                                    }
                                    if ui.button(format!("Installer v{}", v.version)).clicked() {
                                        // Une version plus ancienne que la dernière est verrouillée,
                                        // sinon la mise à jour automatique la remplacerait au lancement suivant
                                        let is_latest = list.first().is_some_and(|l| l.version == v.version);
                                        self.channel = Channel::Stable;
                                        self.pinned = (!is_latest).then(|| v.version.clone());
                                        save_config(Channel::Stable, self.pinned.clone());
                                        self.start_update(v.clone());
                                    }
                                }
                                *self.shared.releases.lock().unwrap() = ReleaseList::Loaded(list);
                            }
                        }
                        if self.pinned.is_some() && ui.small_button("Reprendre les mises a jour automatiques").clicked() {
                            self.pinned = None;
                            save_config(self.channel, None);
                            self.auto_update_tried = false;
                            self.start_check();
                        }
                    });
                    let _ = header;
                });
                ui.separator();
                ui.add_space(4.0);

                if let Some(err) = self.launch_error.clone() {
                    ui.colored_label(egui::Color32::LIGHT_RED, err);
                    ui.add_space(8.0);
                    if ui.button("Quitter").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    return;
                }

                match stage {
                    Stage::Checking => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Verification des mises a jour...");
                        });
                    }
                    Stage::Available(info) => {
                        let switching = CHANNEL != self.channel;
                        // Mise à jour du même canal : installée tout de suite, sans clic (le launcher
                        // fait partie du paquet, il se met donc à jour avec le jeu). Changer de
                        // canal reste un choix du joueur, et un échec ne se relance pas en boucle.
                        if !switching && !self.auto_update_tried {
                            self.auto_update_tried = true;
                            self.start_update(info.clone());
                        }
                        let title = if switching {
                            format!("Disponible : {}", info.label())
                        } else {
                            format!("Nouvelle version disponible : {}", info.label())
                        };
                        ui.label(egui::RichText::new(title).strong());
                        ui.label(&info.release_notes);
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            let action = if switching { "Installer" } else { "Mettre a jour" };
                            if ui.button(action).clicked() {
                                self.start_update(info.clone());
                            }
                            if ui.button("Jouer").clicked() {
                                self.launch_game(&ctx);
                            }
                        });
                    }
                    Stage::ReadyToPlay(msg) => {
                        if !self.stay_open {
                            self.launch_game(&ctx);
                        }
                        ui.label(msg);
                        ui.add_space(10.0);
                        if ui.add_sized([140.0, 34.0], egui::Button::new("Jouer")).clicked() {
                            self.launch_game(&ctx);
                        }
                    }
                    Stage::Downloading => {
                        let done = self.shared.downloaded.load(Ordering::Relaxed);
                        let total = self.shared.total.load(Ordering::Relaxed);
                        ui.label("Telechargement...");
                        let (frac, text) = if total > 0 {
                            (
                                done as f32 / total as f32,
                                format!("{:.1} MB / {:.1} MB", done as f64 / 1_048_576.0, total as f64 / 1_048_576.0),
                            )
                        } else {
                            (0.0, format!("{:.1} MB", done as f64 / 1_048_576.0))
                        };
                        ui.add(egui::ProgressBar::new(frac).text(text).animate(true));
                    }
                    Stage::Extracting => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Installation...");
                        });
                    }
                    // Le jeu ne se lance jamais tout seul : il faut cliquer sur « Jouer »
                    Stage::Installed(msg) => {
                        ui.label(msg);
                        ui.add_space(10.0);
                        if ui.add_sized([140.0, 34.0], egui::Button::new("Jouer")).clicked() {
                            self.launch_game(&ctx);
                        }
                    }
                    Stage::Failed(err) => {
                        ui.colored_label(egui::Color32::LIGHT_RED, format!("Erreur : {}", err));
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            if ui.button("Jouer").clicked() {
                                self.launch_game(&ctx);
                            }
                            if ui.button("Quitter").clicked() {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        });
                    }
                }
            });
        });
    }
}

// ─────────────────────────────────────────────────────────────────────────
//  Téléchargement / installation
// ─────────────────────────────────────────────────────────────────────────

/// Écarte le launcher actuel (en cours d'exécution) pour pouvoir écrire le nouveau à sa place.
fn replace_running_launcher(install_dir: &Path, launcher_name: &str) -> io::Result<()> {
    // Anciennes copies des mises à jour précédentes (celles encore ouvertes sont ignorées)
    if let Ok(entries) = fs::read_dir(install_dir) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with(&format!("{launcher_name}.old")) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    let current = install_dir.join(launcher_name);
    if !current.exists() {
        return Ok(());
    }
    let old = install_dir.join(format!("{launcher_name}.old-{}", std::process::id()));
    // L'antivirus ou une autre fenêtre peut tenir le fichier un instant : on réessaie
    let mut last = None;
    for _ in 0..8 {
        match fs::rename(&current, &old) {
            Ok(()) => return Ok(()),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    let e = last.unwrap();
    Err(io::Error::new(
        e.kind(),
        format!("{launcher_name} : impossible de le remplacer ({e}). Fermez les autres fenetres Nds Game Studio et le jeu, puis reessayez."),
    ))
}

fn download_and_apply(
    info: &VersionInfo,
    install_dir: &Path,
    shared: &Shared,
    ctx: &egui::Context,
) -> Result<(), Box<dyn std::error::Error>> {
    let url = info
        .platform_download_url()
        .ok_or_else(|| format!("Aucune version disponible pour {}", ndsgs_common::PLATFORM))?;

    let temp_dir = std::env::temp_dir().join("nds-game-studio-update");
    fs::create_dir_all(&temp_dir)?;
    let zip_path = temp_dir.join("update.zip");

    let agent = ndsgs_common::make_agent(120);
    let response = agent.get(url).call()?;

    let total_size = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    shared.total.store(total_size, Ordering::Relaxed);

    let mut body = response.into_body();
    let mut file = fs::File::create(&zip_path)?;
    let mut downloaded: u64 = 0;
    let mut buf = [0u8; 8192];

    loop {
        let n = body.as_reader().read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        downloaded += n as u64;
        shared.downloaded.store(downloaded, Ordering::Relaxed);
        ctx.request_repaint();
    }
    drop(file);

    if total_size > 0 && downloaded != total_size {
        return Err(format!("Telechargement incomplet: {} / {} octets", downloaded, total_size).into());
    }

    shared.set(Stage::Extracting);
    ctx.request_repaint();

    let zip_file = fs::File::open(&zip_path)?;
    let mut archive = zip::ZipArchive::new(zip_file)?;
    let launcher_name = exe_name(LAUNCHER_BIN);
    let downgrade = info.build == 0 && info.version_code < ndsgs_common::VERSION_CODE;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let raw_name = entry.name().to_string();

        let relative = raw_name.split('/').skip(1).collect::<Vec<_>>().join("/");
        if relative.is_empty() {
            continue;
        }

        // Retour à une version plus ancienne : on garde le launcher actuel, qui doit toujours
        // être le plus récent (un vieux launcher ne saurait pas verrouiller la version choisie)
        if relative == launcher_name && downgrade {
            continue;
        }

        // Ne pas écraser le launcher lui-même pendant qu'il tourne : sous Windows on peut renommer
        // un exécutable en cours d'exécution, pas le remplacer. Chaque lancement utilise son propre
        // nom d'ancienne copie (celle d'un autre launcher encore ouvert ne bloque donc rien).
        if relative == launcher_name {
            replace_running_launcher(install_dir, &launcher_name)?;
        }

        let out_path = install_dir.join(&relative);

        if entry.is_dir() {
            fs::create_dir_all(&out_path)?;
        } else {
            let mode = entry.unix_mode();
            ndsgs_common::write_extracted_file(&out_path, &mut entry, mode)
                .map_err(|e| io::Error::new(e.kind(), format!("{}: {}", relative, e)))?;
        }
    }
    ndsgs_common::clear_macos_quarantine(install_dir);

    let _ = fs::remove_file(&zip_path);
    let _ = fs::remove_dir_all(&temp_dir);
    Ok(())
}
