use std::path::{Path, PathBuf};

// ─────────────────────────────────────────────────────────────────────────
//  Version et canal (stable / experimentale)
//
//  Ces valeurs sont injectées par la CI au moment de la compilation
//  (variables NDSGS_*). En local, la version vient de Cargo.toml
//  ([workspace.package] version) et le canal vaut "dev".
// ─────────────────────────────────────────────────────────────────────────

pub const VERSION: &str = match option_env!("NDSGS_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
/// 0.6.4 -> 604 (major * 10000 + minor * 100 + patch).
pub const VERSION_CODE: u32 = version_code(VERSION);
/// Numéro de build (experimentale uniquement, 0 sinon).
pub const BUILD: u64 = match option_env!("NDSGS_BUILD") {
    Some(b) => parse_u64(b),
    None => 0,
};
pub const CHANNEL: Channel = match option_env!("NDSGS_CHANNEL") {
    Some(c) => Channel::from_str_const(c),
    None => Channel::Dev,
};
pub const UPDATER_VERSION: u32 = 1;

pub const REPO_OWNER: &str = "liolu";
pub const REPO_NAME: &str = "nds-game-studio";
/// Version stable : version.json joint à la dernière release (branche `stable`).
pub const VERSION_URL: &str = "https://github.com/liolu/nds-game-studio/releases/latest/download/version.json";
/// Version experimentale : fichier joint à la pré-release "experimental", recréée à
/// chaque push sur la branche `experimental`.
pub const EXPERIMENTAL_URL: &str = "https://github.com/liolu/nds-game-studio/releases/download/experimental/experimental.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Experimental,
    /// Compilation locale (cargo build) : pas de mise à jour automatique.
    Dev,
}

impl Channel {
    const fn from_str_const(s: &str) -> Self {
        match s.as_bytes() {
            b"stable" => Channel::Stable,
            b"experimental" => Channel::Experimental,
            _ => Channel::Dev,
        }
    }

    pub fn from_name(s: &str) -> Self {
        Self::from_str_const(s)
    }

    pub fn name(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Experimental => "experimental",
            Channel::Dev => "dev",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Channel::Stable => "Stable",
            Channel::Experimental => "Experimentale",
            Channel::Dev => "Developpement",
        }
    }

    fn url(self) -> Option<&'static str> {
        match self {
            Channel::Stable => Some(VERSION_URL),
            Channel::Experimental => Some(EXPERIMENTAL_URL),
            Channel::Dev => None,
        }
    }
}

/// Date et heure de compilation (UTC), AA.MM.JJ_HH:MM (voir `build.rs`).
pub const BUILD_DATE: &str = env!("NDSGS_BUILD_DATE");
/// Quatrième nombre de la numérotation : révision à l'intérieur d'une version X.Y.Z.
pub const REVISION: u32 = 1;

/// Nouvelle numérotation : « AA.MM.JJ_HH:MM_vX.Y.Z.R », ex. « 26.10.01_16:10_v0.9.1.1 » (date et
/// heure de compilation, puis version et révision). Cargo n'accepte que X.Y.Z : la révision est ajoutée ici.
pub fn display_version() -> String {
    format!("{}_v{}.{}", BUILD_DATE, VERSION, REVISION)
}

/// Texte affiché pour la version installée, ex. "26.10.01_16:10_v0.9.1.1" ou
/// "26.10.01_16:10_v0.9.1.1 experimentale (build 12)".
pub fn installed_label() -> String {
    match CHANNEL {
        Channel::Stable => display_version(),
        Channel::Experimental => format!("{} experimentale (build {})", display_version(), BUILD),
        Channel::Dev => format!("{} (compilation locale)", display_version()),
    }
}

const fn parse_u64(s: &str) -> u64 {
    let b = s.as_bytes();
    let mut i = 0;
    let mut n: u64 = 0;
    while i < b.len() && b[i] >= b'0' && b[i] <= b'9' {
        n = n * 10 + (b[i] - b'0') as u64;
        i += 1;
    }
    n
}

const fn version_code(v: &str) -> u32 {
    let b = v.as_bytes();
    let mut parts = [0u32; 3];
    let mut idx = 0;
    let mut i = 0;
    while i < b.len() && idx < 3 {
        let c = b[i];
        if c == b'.' {
            idx += 1;
        } else if c >= b'0' && c <= b'9' {
            parts[idx] = parts[idx] * 10 + (c - b'0') as u32;
        } else {
            break; // suffixe type "-experimentale.12"
        }
        i += 1;
    }
    parts[0] * 10000 + parts[1] * 100 + parts[2]
}

#[derive(serde::Deserialize, Debug, Clone)]
pub struct VersionInfo {
    pub version: String,
    pub version_code: u32,
    /// Numéro de build (canal experimental).
    #[serde(default)]
    pub build: u64,
    /// URL du zip Windows (conservé tel quel pour les anciens clients).
    pub download_url: String,
    #[serde(default)]
    pub download_url_linux: Option<String>,
    #[serde(default)]
    pub download_url_macos: Option<String>,
    pub release_notes: String,
    pub min_updater_version: u32,
}

impl VersionInfo {
    /// URL du zip correspondant à la plateforme courante, s'il en existe un.
    pub fn platform_download_url(&self) -> Option<&str> {
        match PLATFORM {
            "linux" => self.download_url_linux.as_deref(),
            "macos" => self.download_url_macos.as_deref(),
            _ => Some(self.download_url.as_str()),
        }
    }

    /// Texte de la version distante, ex. "v0.6.5" ou "experimentale build 42".
    pub fn label(&self) -> String {
        if self.build > 0 {
            format!("v{} experimentale (build {})", self.version, self.build)
        } else {
            format!("v{}", self.version)
        }
    }
}

pub fn needs_update(remote_code: u32) -> bool {
    remote_code > VERSION_CODE
}

pub fn make_agent(timeout_secs: u64) -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder()
        .provider(ureq::tls::TlsProvider::NativeTls)
        .build();
    let config = ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_global(Some(std::time::Duration::from_secs(timeout_secs)))
        .build();
    ureq::Agent::new_with_config(config)
}

/// Télécharge les infos de la dernière version d'un canal.
pub fn fetch_channel(channel: Channel, timeout_secs: u64) -> Result<VersionInfo, String> {
    let url = channel.url().ok_or("aucun canal de mise a jour")?;
    let body = make_agent(timeout_secs)
        .get(url)
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&body).map_err(|e| e.to_string())
}

#[derive(serde::Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

#[derive(serde::Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

/// Toutes les versions stables publiées sur GitHub (la plus récente d'abord),
/// pour pouvoir en réinstaller une précise. La pré-release « experimental » et les
/// brouillons sont ignorés ; une version sans zip Windows/Linux/macOS aussi.
pub fn fetch_releases(timeout_secs: u64) -> Result<Vec<VersionInfo>, String> {
    let url = format!("https://api.github.com/repos/{REPO_OWNER}/{REPO_NAME}/releases?per_page=50");
    let body = make_agent(timeout_secs)
        .get(&url)
        .header("User-Agent", "nds-game-studio-launcher")
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let releases: Vec<GhRelease> = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let mut out: Vec<VersionInfo> = releases
        .into_iter()
        .filter(|r| !r.prerelease && !r.draft && r.tag_name.starts_with('v'))
        .filter_map(|r| {
            let version = r.tag_name.trim_start_matches('v').to_string();
            let find = |suffix: &str| r.assets.iter().find(|a| a.name.ends_with(suffix)).map(|a| a.browser_download_url.clone());
            let windows = find("-windows.zip");
            let linux = find("-linux.zip");
            let macos = find("-macos.zip");
            if windows.is_none() && linux.is_none() && macos.is_none() {
                return None;
            }
            Some(VersionInfo {
                version_code: version_code(&version),
                version,
                build: 0,
                download_url: windows.unwrap_or_default(),
                download_url_linux: linux,
                download_url_macos: macos,
                release_notes: r.body.unwrap_or_default(),
                min_updater_version: 1,
            })
        })
        .filter(|v| v.platform_download_url().is_some_and(|u| !u.is_empty()))
        .collect();
    out.sort_by_key(|v| std::cmp::Reverse(v.version_code));
    Ok(out)
}

/// Faut-il installer `remote` (dernière version du canal `wanted`) à la place
/// de la version actuellement installée ?
pub fn should_install(wanted: Channel, remote: &VersionInfo) -> bool {
    if remote.platform_download_url().is_none() {
        return false;
    }
    if CHANNEL != wanted {
        return true; // changement de canal
    }
    match wanted {
        Channel::Stable => needs_update(remote.version_code),
        Channel::Experimental => remote.build > BUILD,
        Channel::Dev => false,
    }
}

// ─────────────────────────────────────────────────────────────────────────
//  Plateforme
// ─────────────────────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
pub const PLATFORM: &str = "windows";
#[cfg(target_os = "macos")]
pub const PLATFORM: &str = "macos";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub const PLATFORM: &str = "linux";

pub const GAME_BIN: &str = "nds-game-studio";
pub const LAUNCHER_BIN: &str = "nds-game-studio-launcher";
pub const UPDATER_BIN: &str = "nds-game-studio-updater";

/// Nom de fichier d'un exécutable (`nds-game-studio` → `nds-game-studio.exe` sous Windows).
pub fn exe_name(bin: &str) -> String {
    format!("{}{}", bin, std::env::consts::EXE_SUFFIX)
}

/// Dossier contenant l'exécutable courant.
pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

/// Écrit un fichier extrait d'une mise à jour.
///
/// Le contenu passe par un fichier temporaire puis un renommage : sous
/// Linux/macOS, écraser directement un binaire en cours d'exécution échoue
/// ("Text file busy"), alors qu'un renommage remplace le fichier proprement.
/// Les droits d'exécution Unix sont restaurés depuis le zip (ou forcés pour
/// les binaires connus si le zip n'en contient pas).
pub fn write_extracted_file(
    out_path: &Path,
    reader: &mut dyn std::io::Read,
    unix_mode: Option<u32>,
) -> std::io::Result<()> {
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut tmp_name = out_path.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp_path = PathBuf::from(tmp_name);
    {
        let mut out = std::fs::File::create(&tmp_path)?;
        std::io::copy(reader, &mut out)?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let is_known_bin = out_path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| [GAME_BIN, LAUNCHER_BIN, UPDATER_BIN].contains(&n) || n.ends_with(".sh") || n.ends_with(".command"))
            .unwrap_or(false);
        let mut mode = unix_mode.map(|m| m & 0o777).filter(|m| *m != 0).unwrap_or(0o644);
        if is_known_bin {
            mode |= 0o755;
        }
        std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = unix_mode;

    #[cfg(windows)]
    if out_path.exists() {
        // Sous Windows, rename n'écrase pas toujours un fichier existant.
        std::fs::remove_file(out_path)?;
    }
    std::fs::rename(&tmp_path, out_path)
}

/// Retire le quarantine flag macOS des fichiers téléchargés, sinon Gatekeeper
/// refuse de lancer les binaires non signés.
pub fn clear_macos_quarantine(_dir: &Path) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("xattr")
            .args(["-dr", "com.apple.quarantine"])
            .arg(_dir)
            .status();
    }
}

// ─────────────────────────────────────────────────────────────────────────
//  Raccourcis : toujours vers le launcher, jamais vers le jeu
//
//  Le launcher vérifie les mises à jour puis lance le jeu ; un raccourci qui
//  ouvre directement `nds-game-studio` les contourne. Au démarrage du jeu (et du
//  launcher), on redirige donc vers le launcher tout raccourci du Bureau ou du
//  menu Démarrer / des applications qui pointe vers ce jeu. Seules les versions
//  installées sont concernées : une compilation locale n'est jamais touchée.
// ─────────────────────────────────────────────────────────────────────────

/// Remplace le jeu par le launcher dans les lignes `Exec=` d'une entrée `.desktop`.
/// Renvoie `None` si rien ne change.
pub fn redirect_desktop_entry(content: &str, game: &str, launcher: &str) -> Option<String> {
    // Le chemin du launcher commence par celui du jeu (`…/nds-game-studio` puis `…/nds-game-studio-launcher`) :
    // on ne remplace donc le jeu que s'il est suivi d'une fin de chemin (espace, guillemet ou fin de ligne)
    fn replace_exact(line: &str, game: &str, launcher: &str) -> Option<String> {
        let mut out = String::new();
        let mut rest = line;
        let mut changed = false;
        while let Some(pos) = rest.find(game) {
            let after = &rest[pos + game.len()..];
            let ends_path = after.chars().next().map_or(true, |c| c.is_whitespace() || c == '"' || c == '\'');
            out.push_str(&rest[..pos]);
            if ends_path {
                out.push_str(launcher);
                changed = true;
            } else {
                out.push_str(game);
            }
            rest = after;
        }
        out.push_str(rest);
        changed.then_some(out)
    }

    let mut changed = false;
    let lines: Vec<String> = content
        .lines()
        .map(|line| match line.starts_with("Exec=").then(|| replace_exact(line, game, launcher)).flatten() {
            Some(new) => {
                changed = true;
                new
            }
            None => line.to_string(),
        })
        .collect();
    changed.then(|| lines.join("\n") + "\n")
}

/// Script PowerShell qui redirige vers le launcher les raccourcis `.lnk` de ces
/// dossiers dont la cible est exactement le jeu.
pub fn shortcut_repair_script(folders: &[PathBuf], game: &Path, launcher: &Path, work_dir: &Path) -> String {
    let quote = |p: &Path| format!("'{}'", p.display().to_string().replace('\'', "''"));
    let folders: Vec<String> = folders.iter().map(|f| quote(f)).collect();
    format!(
        "$ErrorActionPreference='SilentlyContinue'; \
         $sh = New-Object -ComObject WScript.Shell; \
         foreach ($dir in @({folders})) {{ \
           if (Test-Path -LiteralPath $dir) {{ \
             foreach ($f in Get-ChildItem -LiteralPath $dir -Filter *.lnk -File) {{ \
               $l = $sh.CreateShortcut($f.FullName); \
               if ($l.TargetPath -ieq {game}) {{ \
                 $l.TargetPath = {launcher}; $l.WorkingDirectory = {work}; $l.Save() \
               }} \
             }} \
           }} \
         }}",
        folders = folders.join(","),
        game = quote(game),
        launcher = quote(launcher),
        work = quote(work_dir),
    )
}

/// Redirige vers le launcher les raccourcis qui ouvrent le jeu directement.
/// Sans effet en compilation locale, ou si le launcher n'est pas installé à côté.
pub fn repair_shortcuts() {
    if CHANNEL == Channel::Dev {
        return;
    }
    let dir = exe_dir();
    let game = dir.join(exe_name(GAME_BIN));
    let launcher = dir.join(exe_name(LAUNCHER_BIN));
    if !game.exists() || !launcher.exists() {
        return;
    }

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let mut folders: Vec<PathBuf> = Vec::new();
        folders.extend(dirs::desktop_dir());
        if let Some(data) = dirs::data_dir() {
            folders.push(data.join("Microsoft").join("Windows").join("Start Menu").join("Programs"));
        }
        let script = shortcut_repair_script(&folders, &game, &launcher, &dir);
        // Sans fenêtre : PowerShell fait le travail en arrière-plan
        let _ = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &script])
            .creation_flags(0x0800_0000)
            .spawn();
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let (game, launcher) = (game.display().to_string(), launcher.display().to_string());
        let mut folders: Vec<PathBuf> = Vec::new();
        folders.extend(dirs::desktop_dir());
        if let Some(data) = dirs::data_dir() {
            folders.push(data.join("applications"));
        }
        for folder in folders {
            let Ok(entries) = std::fs::read_dir(&folder) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                    continue;
                }
                if let Some(new) = std::fs::read_to_string(&path).ok().and_then(|c| redirect_desktop_entry(&c, &game, &launcher)) {
                    let _ = std::fs::write(&path, new);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_entries_are_redirected_to_the_launcher() {
        let entry = "[Desktop Entry]\nType=Application\nName=Nds Game Studio\nExec=\"/home/u/Games/Nds Game Studio/nds-game-studio\"\nPath=/home/u/Games/Nds Game Studio\n";
        let out = redirect_desktop_entry(entry, "/home/u/Games/Nds Game Studio/nds-game-studio", "/home/u/Games/Nds Game Studio/nds-game-studio-launcher").unwrap();
        assert!(out.contains("Exec=\"/home/u/Games/Nds Game Studio/nds-game-studio-launcher\""));
        // Seule la ligne Exec change
        assert!(out.contains("Path=/home/u/Games/Nds Game Studio\n") && out.contains("Name=Nds Game Studio\n"));
        // Déjà redirigé, ou entrée d'un autre programme : rien à faire
        assert!(redirect_desktop_entry(&out, "/home/u/Games/Nds Game Studio/nds-game-studio", "/home/u/Games/Nds Game Studio/nds-game-studio-launcher").is_none());
        assert!(out.matches("launcher").count() == 1);
        assert!(redirect_desktop_entry("[Desktop Entry]\nExec=/usr/bin/autre\n", "/x/nds-game-studio", "/x/nds-game-studio-launcher").is_none());
    }

    #[test]
    fn repair_script_only_targets_the_game_and_quotes_paths() {
        let script = shortcut_repair_script(
            &[PathBuf::from(r"C:\Users\Jo O'Neil\Desktop")],
            Path::new(r"C:\Games\Nds Game Studio\nds-game-studio.exe"),
            Path::new(r"C:\Games\Nds Game Studio\nds-game-studio-launcher.exe"),
            Path::new(r"C:\Games\Nds Game Studio"),
        );
        // Apostrophe échappée dans le chemin, comparaison sur la cible exacte du jeu
        assert!(script.contains(r"'C:\Users\Jo O''Neil\Desktop'"));
        assert!(script.contains(r"-ieq 'C:\Games\Nds Game Studio\nds-game-studio.exe'"));
        assert!(script.contains(r"$l.TargetPath = 'C:\Games\Nds Game Studio\nds-game-studio-launcher.exe'"));
        assert!(script.contains("$l.Save()"));
    }

    /// Avec un vrai raccourci Windows : celui du jeu est redirigé, les autres non.
    #[cfg(target_os = "windows")]
    #[test]
    fn windows_shortcuts_are_really_redirected() {
        let root = std::env::temp_dir().join(format!("nds-game-studio-lnk-test-{}", std::process::id()));
        let (install, desk) = (root.join("Jo O'Neil Games"), root.join("Bureau"));
        std::fs::create_dir_all(&install).unwrap();
        std::fs::create_dir_all(&desk).unwrap();
        let (game, launcher, other) = (install.join("nds-game-studio.exe"), install.join("nds-game-studio-launcher.exe"), install.join("autre.exe"));
        for f in [&game, &launcher, &other] {
            std::fs::write(f, b"x").unwrap();
        }
        let ps = |script: &str| {
            let out = std::process::Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", script]).output().unwrap();
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        let q = |p: &Path| format!("'{}'", p.display().to_string().replace('\'', "''"));
        let make = |name: &str, target: &Path| {
            ps(&format!(
                "$l = (New-Object -ComObject WScript.Shell).CreateShortcut({}); $l.TargetPath = {}; $l.Save()",
                q(&desk.join(name)),
                q(target)
            ));
        };
        let read = |name: &str| ps(&format!("(New-Object -ComObject WScript.Shell).CreateShortcut({}).TargetPath", q(&desk.join(name))));
        make("Nds Game Studio.lnk", &game);
        make("Renomme par moi.lnk", &game);
        make("Autre jeu.lnk", &other);

        ps(&shortcut_repair_script(&[desk.clone(), root.join("absent")], &game, &launcher, &install));

        assert_eq!(read("Nds Game Studio.lnk"), launcher.display().to_string());
        assert_eq!(read("Renomme par moi.lnk"), launcher.display().to_string());
        assert_eq!(read("Autre jeu.lnk"), other.display().to_string());
        // Relancé : plus rien à faire, la cible reste le launcher
        ps(&shortcut_repair_script(&[desk.clone()], &game, &launcher, &install));
        assert_eq!(read("Nds Game Studio.lnk"), launcher.display().to_string());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn version_codes() {
        assert_eq!(version_code("0.6.4"), 604);
        assert_eq!(version_code("1.12.3"), 11203);
        assert_eq!(version_code("0.6.4-experimentale.12"), 604);
        assert_eq!(parse_u64("42"), 42);
        assert_eq!(Channel::from_name("experimental"), Channel::Experimental);
        assert_eq!(Channel::from_name("n'importe quoi"), Channel::Dev);
    }

    #[test]
    fn version_label_uses_the_date_and_four_numbers() {
        // Ex. 26.10.01_16:10_v0.9.1.1
        let label = display_version();
        let (stamp, version) = label.split_once("_v").expect("AA.MM.JJ_HH:MM_vX.Y.Z.R");
        let numbers = |s: &str, n: usize| s.split('.').count() == n && s.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
        let (date, time) = stamp.split_once('_').expect("date_heure");
        assert!(numbers(date, 3) && date.len() == 8, "date {date}");
        assert!(time.len() == 5 && time.as_bytes()[2] == b':' && time.replace(':', "").chars().all(|c| c.is_ascii_digit()), "heure {time}");
        assert!(numbers(version, 4), "version {version}");
        assert!(installed_label().starts_with(&label));
    }
}
