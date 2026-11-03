use nwd::NwgUi;
use nwg::NativeUi;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::{AtomicU64, AtomicBool, Ordering}};
use std::thread;

#[derive(Default, NwgUi)]
pub struct InstallerApp {
    #[nwg_control(
        size: (500, 340),
        position: (300, 200),
        title: "Nds Game Studio Installer",
        flags: "WINDOW|VISIBLE",
        center: true
    )]
    #[nwg_events(OnWindowClose: [InstallerApp::on_close])]
    window: nwg::Window,

    #[nwg_control(
        text: "Nds Game Studio Installer",
        size: (460, 30),
        position: (20, 15)
    )]
    title_label: nwg::Label,

    #[nwg_control(
        text: "Install folder:",
        size: (460, 20),
        position: (20, 60)
    )]
    path_label: nwg::Label,

    #[nwg_control(
        text: "",
        size: (370, 25),
        position: (20, 85)
    )]
    path_input: nwg::TextInput,

    #[nwg_control(
        text: "Parcourir",
        size: (80, 25),
        position: (400, 85)
    )]
    #[nwg_events(OnButtonClick: [InstallerApp::on_browse])]
    browse_btn: nwg::Button,

    #[nwg_control(
        text: "Create a Desktop shortcut",
        size: (250, 25),
        position: (20, 125)
    )]
    desktop_shortcut: nwg::CheckBox,

    #[nwg_control(
        text: "Create a Start Menu shortcut",
        size: (260, 25),
        position: (20, 155)
    )]
    startmenu_shortcut: nwg::CheckBox,

    #[nwg_control(
        text: "Installer",
        size: (150, 35),
        position: (175, 200)
    )]
    #[nwg_events(OnButtonClick: [InstallerApp::on_install])]
    install_btn: nwg::Button,

    #[nwg_control(
        size: (460, 20),
        position: (20, 250),
        range: 0..100
    )]
    progress: nwg::ProgressBar,

    #[nwg_control(
        text: "Ready to install",
        size: (460, 20),
        position: (20, 280)
    )]
    status_label: nwg::Label,

    #[nwg_control]
    #[nwg_events(OnNotice: [InstallerApp::on_notice])]
    notice: nwg::Notice,

    progress_val: RefCell<Arc<AtomicU64>>,
    total_val: RefCell<Arc<AtomicU64>>,
    done_flag: RefCell<Arc<AtomicBool>>,
    error_msg: RefCell<Arc<std::sync::Mutex<Option<String>>>>,
    install_dir: RefCell<PathBuf>,
}

impl InstallerApp {
    fn on_close(&self) {
        nwg::stop_thread_dispatch();
    }

    fn on_browse(&self) {
        let mut dialog = Default::default();
        let _ = nwg::FileDialog::builder()
            .title("Choose the install folder")
            .action(nwg::FileDialogAction::OpenDirectory)
            .build(&mut dialog);

        if dialog.run(Some(&self.window)) {
            if let Ok(path) = dialog.get_selected_item() {
                let path = PathBuf::from(path).join("Nds Game Studio");
                self.path_input.set_text(&path.to_string_lossy());
            }
        }
    }

    fn on_install(&self) {
        let install_path = self.path_input.text();
        if install_path.is_empty() {
            nwg::modal_info_message(&self.window, "Error", "Please choose an install folder.");
            return;
        }

        let install_dir = PathBuf::from(&install_path);
        *self.install_dir.borrow_mut() = install_dir.clone();

        self.install_btn.set_enabled(false);
        self.browse_btn.set_enabled(false);
        self.status_label.set_text("Getting information...");

        let progress = Arc::new(AtomicU64::new(0));
        let total = Arc::new(AtomicU64::new(0));
        let done = Arc::new(AtomicBool::new(false));
        let error = Arc::new(std::sync::Mutex::new(None::<String>));

        *self.progress_val.borrow_mut() = progress.clone();
        *self.total_val.borrow_mut() = total.clone();
        *self.done_flag.borrow_mut() = done.clone();
        *self.error_msg.borrow_mut() = error.clone();

        let create_desktop = self.desktop_shortcut.check_state() == nwg::CheckBoxState::Checked;
        let create_startmenu = self.startmenu_shortcut.check_state() == nwg::CheckBoxState::Checked;
        let sender = self.notice.sender();

        thread::spawn(move || {
            if let Err(e) = do_install(&install_dir, &progress, &total, create_desktop, create_startmenu, &sender) {
                *error.lock().unwrap() = Some(e);
            }
            done.store(true, Ordering::Relaxed);
            sender.notice();
        });
    }

    fn on_notice(&self) {
        let progress = self.progress_val.borrow().load(Ordering::Relaxed);
        let total = self.total_val.borrow().load(Ordering::Relaxed);

        if total > 0 {
            let pct = (progress as f64 / total as f64 * 100.0) as u32;
            self.progress.set_pos(pct);
            let mb_done = progress as f64 / 1_048_576.0;
            let mb_total = total as f64 / 1_048_576.0;
            self.status_label.set_text(&format!(
                "Telechargement... {:.1} MB / {:.1} MB ({}%)",
                mb_done, mb_total, pct
            ));
        }

        if self.done_flag.borrow().load(Ordering::Relaxed) {
            self.progress.set_pos(100);

            let err = self.error_msg.borrow().lock().unwrap().clone();
            if let Some(e) = err {
                self.status_label.set_text("Installation error");
                nwg::modal_info_message(&self.window, "Error", &format!("The installation failed:\n{}", e));
                self.install_btn.set_enabled(true);
                self.browse_btn.set_enabled(true);
            } else {
                self.status_label.set_text("Installation complete!");
                let install_dir = self.install_dir.borrow().clone();
                let msg = format!(
                    "Nds Game Studio was installed in:\n{}\n\nStart it now?",
                    install_dir.display()
                );
                let params = nwg::MessageParams {
                    title: "Installation complete",
                    content: &msg,
                    buttons: nwg::MessageButtons::YesNo,
                    icons: nwg::MessageIcons::Info,
                };
                if nwg::message(&params) == nwg::MessageChoice::Yes {
                    let launcher = install_dir.join("nds-game-studio-launcher.exe");
                    let game = install_dir.join("nds-game-studio.exe");
                    let exe = if launcher.exists() { &launcher } else { &game };
                    if let Err(e) = std::process::Command::new(exe)
                        .current_dir(&install_dir)
                        .spawn()
                    {
                        nwg::modal_info_message(
                            &self.window,
                            "Erreur",
                            &format!("Cannot start it:\n{}\n\nPath: {}", e, exe.display()),
                        );
                    }
                }
                nwg::stop_thread_dispatch();
            }
        }
    }
}

fn do_install(
    install_dir: &Path,
    progress: &AtomicU64,
    total: &AtomicU64,
    create_desktop: bool,
    create_startmenu: bool,
    sender: &nwg::NoticeSender,
) -> Result<(), String> {
    crate::install::download_and_extract(install_dir, progress, total, &|| sender.notice())?;

    let game_exe = install_dir.join("nds-game-studio.exe");
    let launcher_exe = install_dir.join("nds-game-studio-launcher.exe");
    let target_exe = if launcher_exe.exists() { &launcher_exe } else { &game_exe };

    if create_startmenu {
        if let Some(data_dir) = dirs::data_dir() {
            let programs = data_dir
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs");
            let lnk = programs.join("Nds Game Studio.lnk");
            if let Ok(mut sl) = mslnk::ShellLink::new(target_exe.to_str().unwrap_or_default()) {
                sl.set_working_dir(Some(install_dir.to_string_lossy().into_owned()));
                let _ = sl.create_lnk(lnk.to_str().unwrap_or_default());
            }
        }
    }

    if create_desktop {
        if let Some(desktop) = dirs::desktop_dir() {
            let lnk = desktop.join("Nds Game Studio.lnk");
            if let Ok(mut sl) = mslnk::ShellLink::new(target_exe.to_str().unwrap_or_default()) {
                sl.set_working_dir(Some(install_dir.to_string_lossy().into_owned()));
                let _ = sl.create_lnk(lnk.to_str().unwrap_or_default());
            }
        }
    }

    Ok(())
}

pub fn run() {
    nwg::init().expect("Failed to init Native Windows GUI");

    let mut font = Default::default();
    let _ = nwg::Font::builder()
        .family("Segoe UI")
        .size(16)
        .build(&mut font);
    nwg::Font::set_global_default(Some(font));

    let app = InstallerApp::build_ui(Default::default()).expect("Failed to build UI");

    let default_path = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("C:\\"))
        .join("Games")
        .join("Nds Game Studio");
    app.path_input.set_text(&default_path.to_string_lossy());
    app.desktop_shortcut.set_check_state(nwg::CheckBoxState::Checked);
    app.startmenu_shortcut.set_check_state(nwg::CheckBoxState::Checked);

    nwg::dispatch_thread_events();
}
