use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::Path;
use std::process::Command;

#[derive(serde::Deserialize)]
struct UpdateConfig {
    zip_path: String,
    install_dir: String,
    #[allow(dead_code)] // lu dans le fichier de config, plus utilisé : le launcher lance le jeu
    game_exe: String,
    game_pid: u32,
}

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 3 || args[1] != "--apply" {
        eprintln!("Usage: nds-game-studio-updater --apply <update.json>");
        std::process::exit(1);
    }

    let config_path = &args[2];
    println!("Nds Game Studio Updater v{}", ndsgs_common::VERSION);
    println!("Reading update config: {}", config_path);

    let config: UpdateConfig = match fs::read_to_string(config_path) {
        Ok(s) => match serde_json::from_str(&s) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Failed to parse update config: {}", e);
                std::process::exit(1);
            }
        },
        Err(e) => {
            eprintln!("Failed to read update config: {}", e);
            std::process::exit(1);
        }
    };

    println!("Waiting for game (PID {}) to exit...", config.game_pid);
    wait_for_process(config.game_pid);

    println!("Extracting update...");
    if let Err(e) = extract_update(&config) {
        eprintln!("Failed to extract update: {}", e);
        eprintln!("Press Enter to exit...");
        let _ = io::stdin().read(&mut [0u8]);
        std::process::exit(1);
    }

    if let Err(e) = fs::remove_file(config_path) {
        eprintln!("Warning: could not remove update config: {}", e);
    }
    if let Err(e) = fs::remove_file(&config.zip_path) {
        eprintln!("Warning: could not remove update zip: {}", e);
    }

    // Le jeu ne démarre jamais tout seul : on rouvre le launcher, où il faut cliquer sur « Jouer »
    println!("Update complete! Opening the launcher...");
    let install_dir = Path::new(&config.install_dir);
    let launcher = install_dir.join(ndsgs_common::exe_name(ndsgs_common::LAUNCHER_BIN));
    if launcher.exists() {
        let _ = Command::new(&launcher).current_dir(install_dir).spawn();
    }
}

fn wait_for_process(pid: u32) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};

        unsafe {
            let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
            if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
                WaitForSingleObject(handle, 10_000);
                CloseHandle(handle);
            }
        }
    }

    #[cfg(unix)]
    {
        // `kill -0` teste l'existence du processus sans lui envoyer de signal.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            let alive = Command::new("kill")
                .args(["-0", &pid.to_string()])
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !alive {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }
}

fn extract_update(config: &UpdateConfig) -> Result<(), Box<dyn std::error::Error>> {
    let zip_file = fs::File::open(&config.zip_path)?;
    let mut archive = zip::ZipArchive::new(zip_file)?;
    let install_dir = Path::new(&config.install_dir);

    let updater_name = ndsgs_common::exe_name(ndsgs_common::UPDATER_BIN);
    let updater_exe = install_dir.join(&updater_name);
    let updater_old = install_dir.join(format!("{}.old", updater_name));
    if updater_exe.exists() {
        let _ = fs::remove_file(&updater_old);
        fs::rename(&updater_exe, &updater_old)?;
    }

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let raw_name = entry.name().to_string();

        let relative = raw_name
            .split('/')
            .skip(1)
            .collect::<Vec<_>>()
            .join("/");

        if relative.is_empty() {
            continue;
        }

        let out_path = install_dir.join(&relative);

        if entry.is_dir() {
            fs::create_dir_all(&out_path)?;
        } else {
            let mode = entry.unix_mode();
            ndsgs_common::write_extracted_file(&out_path, &mut entry, mode)?;
            println!("  extracted: {}", relative);
        }
    }
    ndsgs_common::clear_macos_quarantine(install_dir);

    Ok(())
}
