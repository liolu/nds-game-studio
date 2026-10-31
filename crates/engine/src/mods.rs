//! Les mods : l'outil `nds-mod-tools` (`tools/modtools`) est une option téléchargée à la demande depuis l'onglet Moding.
//!
//! Il exporte les fichiers d'un jeu en PNG, OBJ, WAV, MP3, MP4 et TXT dans `mods/<code>/fichiers`, et fabrique
//! `mods/<code>/jeu-modifie.nds` avec les fichiers modifiés. La rom de l'utilisateur n'est jamais modifiée : jouer sur les
//! originaux revient à lui passer sa rom. Retirer l'outil ne retire pas les mods.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::activity::{self, What};
use crate::i18n::{lang, t, tr, Lang};

/// La version de l'outil à télécharger : à augmenter quand il change (`tools/modtools/version.txt`).
const VERSION: &str = include_str!("../../../tools/modtools/version.txt");
const RELEASE_URL: &str = "https://github.com/liolu/nds-game-studio/releases/download/mod-tools";
const MODDED_ROM: &str = "jeu-modifie.nds";
const FILES: &str = "fichiers";
const ON_FILE: &str = "actif.txt";
const REPORT: &str = "rapport.json";
const TOOL: &str = if cfg!(windows) { "nds-mod-tools.exe" } else { "nds-mod-tools" };
/// Lignes de l'outil gardées pour la page.
const KEPT_LINES: usize = 4;

fn platform() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

fn install_root() -> PathBuf {
    crate::save::data_dir().join("mod-tools")
}

fn install_dir() -> PathBuf {
    install_root().join(VERSION.trim())
}

fn tool_path() -> PathBuf {
    install_dir().join(TOOL)
}

pub fn installed() -> bool {
    tool_path().is_file()
}

/// Le dossier de mod d'un jeu.
pub fn mod_dir(code: &str) -> PathBuf {
    crate::save::data_dir().join("mods").join(code)
}

pub fn files_dir(code: &str) -> PathBuf {
    mod_dir(code).join(FILES)
}

pub fn exported(code: &str) -> bool {
    files_dir(code).is_dir()
}

pub fn has_modded_rom(code: &str) -> bool {
    mod_dir(code).join(MODDED_ROM).is_file()
}

/// Le jeu se lance avec les mods. Choisi une fois pour toutes par jeu, jusqu'au prochain clic.
pub fn is_on(code: &str) -> bool {
    has_modded_rom(code) && std::fs::read_to_string(mod_dir(code).join(ON_FILE)).is_ok_and(|text| text.trim() == "1")
}

pub fn set_on(code: &str, on: bool) {
    let dir = mod_dir(code);
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join(ON_FILE), if on { "1" } else { "0" });
}

/// La copie modifiée de la rom d'un jeu.
pub fn modded_rom(code: &str) -> PathBuf {
    mod_dir(code).join(MODDED_ROM)
}

/// La rom à donner au jeu : la copie modifiée si les mods sont actifs, sinon la rom de l'utilisateur.
pub fn rom_to_play(code: &str, rom: &Path) -> PathBuf {
    if is_on(code) { modded_rom(code) } else { rom.to_path_buf() }
}

/// Retire l'outil, jamais les mods.
pub fn remove_tool() {
    let _ = std::fs::remove_dir_all(install_root());
}

#[derive(Default)]
struct Job {
    code: Option<String>,
    lines: Vec<String>,
    running: bool,
    error: Option<String>,
}

static JOB: Mutex<Job> = Mutex::new(Job { code: None, lines: Vec::new(), running: false, error: None });
/// Change à chaque nouvelle ligne : la page se refait.
static REVISION: AtomicU64 = AtomicU64::new(0);

pub fn revision() -> u64 {
    REVISION.load(Ordering::Relaxed)
}

pub fn running() -> bool {
    JOB.lock().unwrap().running
}

pub(crate) fn say(line: String) {
    let mut job = JOB.lock().unwrap();
    job.lines.push(line);
    let extra = job.lines.len().saturating_sub(KEPT_LINES);
    job.lines.drain(..extra);
    REVISION.fetch_add(1, Ordering::Relaxed);
}

/// Ce que dit le travail en cours ou le dernier travail de ce jeu.
pub fn lines_for(code: &str) -> (Vec<String>, Option<String>) {
    let job = JOB.lock().unwrap();
    if job.code.as_deref().is_none_or(|c| c == code) { (job.lines.clone(), job.error.clone()) } else { (Vec::new(), None) }
}

pub(crate) fn start(code: Option<String>, work: impl FnOnce() -> Result<(), String> + Send + 'static) {
    {
        let mut job = JOB.lock().unwrap();
        if job.running {
            return;
        }
        *job = Job { code: code.clone(), lines: Vec::new(), running: true, error: None };
    }
    REVISION.fetch_add(1, Ordering::Relaxed);
    std::thread::spawn(move || {
        let result = work();
        let mut job = JOB.lock().unwrap();
        job.running = false;
        job.error = result.err();
        drop(job);
        REVISION.fetch_add(1, Ordering::Relaxed);
    });
}

/// Télécharge l'outil, dans un fil à part.
pub fn install() {
    start(None, || {
        let result = download_and_unpack();
        match &result {
            Ok(()) => activity::record(What::ModTools),
            Err(e) => activity::record(What::ModFailed { game: String::new(), error: e.clone() }),
        }
        result
    });
}

fn download_and_unpack() -> Result<(), String> {
    let dir = install_dir();
    let _ = std::fs::remove_dir_all(install_root());
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let archive = dir.join("tools.zip");
    if let Some(local) = std::env::var_os("NDSGS_MODTOOLS_ZIP") {
        std::fs::copy(local, &archive).map_err(|e| e.to_string())?;
    } else {
        say(tr("Downloading the mod tools...", "Telechargement des outils de mod...").to_string());
        let url = format!("{RELEASE_URL}/mod-tools-{}-{}.zip", VERSION.trim(), platform());
        crate::native_game::download(&url, &archive)?;
    }
    say(tr("Installing...", "Installation...").to_string());
    let file = std::fs::File::open(&archive).map_err(|e| e.to_string())?;
    zip::ZipArchive::new(file).and_then(|mut z| z.extract(&dir)).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&archive);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for name in [TOOL, "ffmpeg"] {
            let _ = std::fs::set_permissions(dir.join(name), std::fs::Permissions::from_mode(0o755));
        }
    }
    if !installed() {
        return Err(tr("the download does not contain the tool", "le telechargement ne contient pas l'outil").to_string());
    }
    say(tr("The mod tools are ready.", "Les outils de mod sont prets.").to_string());
    Ok(())
}

fn run_tool(action: &str, code: &str, rom: &Path, extra: &[&str]) -> Result<(), String> {
    let dir = mod_dir(code);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut command = Command::new(tool_path());
    command.arg(action).arg("--rom").arg(rom).arg("--mod-dir").arg(&dir).arg("--lang").arg(if lang() == Lang::Fr { "fr" } else { "en" }).args(extra);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
        command.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
    }
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
    let errors = Mutex::new(String::new());
    std::thread::scope(|scope| {
        use std::io::BufRead;
        scope.spawn(|| {
            for line in stdout.into_iter().flat_map(|out| std::io::BufReader::new(out).split(b'\n')).map_while(Result::ok) {
                let line = String::from_utf8_lossy(&line).trim_end().to_string();
                if !line.is_empty() {
                    say(line);
                }
            }
        });
        scope.spawn(|| {
            for line in stderr.into_iter().flat_map(|err| std::io::BufReader::new(err).split(b'\n')).map_while(Result::ok) {
                errors.lock().unwrap().push_str(&format!("{}\n", String::from_utf8_lossy(&line).trim_end()));
            }
        });
    });
    let status = child.wait().map_err(|e| e.to_string())?;
    if status.success() {
        return Ok(());
    }
    let text = errors.into_inner().unwrap();
    Err(text.lines().last().map_or_else(|| status.to_string(), str::to_string))
}

pub fn export(code: &str, rom: &Path) {
    let (code, rom) = (code.to_string(), rom.to_path_buf());
    start(Some(code.clone()), move || {
        run_tool("export", &code, &rom, &[]).inspect_err(|e| activity::record(What::ModFailed { game: code.clone(), error: e.clone() }))
    });
}

/// Réimporte ce qui a changé et refait la copie de la rom ; les mods deviennent actifs s'il y a une copie.
pub fn apply(code: &str, rom: &Path) {
    let (code, rom) = (code.to_string(), rom.to_path_buf());
    start(Some(code.clone()), move || apply_now(&code, &rom));
}

/// Comme `apply`, dans le travail en cours.
pub(crate) fn apply_now(code: &str, rom: &Path) -> Result<(), String> {
    let result = run_tool("import", code, rom, &[]);
    match &result {
        Ok(()) => {
            set_on(code, has_modded_rom(code));
            let (applied, errors) = report(code).map_or((0, 0), |r| (r.applied.len(), r.errors.len()));
            activity::record(What::ModApplied { game: code.to_string(), applied, errors });
        }
        Err(e) => activity::record(What::ModFailed { game: code.to_string(), error: e.clone() }),
    }
    result
}

/// Le fichier exporté qui correspond à un fichier du jeu : pas un `.mtl`, qui ne sert qu'à accompagner un `.obj`.
fn exported_file(code: &str, entry_path: &str) -> Option<PathBuf> {
    #[derive(serde::Deserialize)]
    struct Item {
        file: String,
        source: String,
    }
    #[derive(serde::Deserialize)]
    struct Manifest {
        items: Vec<Item>,
    }
    let manifest: Manifest = serde_json::from_slice(&std::fs::read(mod_dir(code).join("manifest.json")).ok()?).ok()?;
    let mine: Vec<&Item> = manifest.items.iter().filter(|item| item.source == entry_path).collect();
    // Un modèle se montre par son .obj, et non par la première de ses textures
    let item = mine.iter().find(|item| item.file.ends_with(".obj")).or_else(|| mine.iter().find(|item| !item.file.ends_with(".mtl"))).or(mine.first())?;
    let path = files_dir(code).join(item.file.replace('/', std::path::MAIN_SEPARATOR_STR));
    path.is_file().then_some(path)
}

/// Essais : le chemin va dans ce fichier, sans ouvrir de fenêtre devant l'utilisateur.
fn test_log(action: &str, path: &Path) -> bool {
    let Some(log) = std::env::var_os("NDSGS_REVEAL_LOG") else { return false };
    let _ = std::fs::write(log, format!("{action} {}", path.display()));
    true
}

/// Montre un fichier dans l'explorateur du système, sélectionné.
fn show_in_folder(path: &Path) {
    if test_log("dossier", path) {
        return;
    }
    let mut command;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command = Command::new("explorer");
        command.raw_arg(format!("/select,\"{}\"", path.display()));
    }
    #[cfg(target_os = "macos")]
    {
        command = Command::new("open");
        command.arg("-R").arg(path);
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        command = Command::new("xdg-open");
        command.arg(path.parent().unwrap_or(path));
    }
    let _ = command.spawn();
}

/// Ouvre un fichier avec le logiciel que le système lui associe.
fn open_with_system(path: &Path) {
    if test_log("fichier", path) {
        return;
    }
    let mut command;
    #[cfg(windows)]
    {
        command = Command::new("explorer");
        command.arg(path);
    }
    #[cfg(target_os = "macos")]
    {
        command = Command::new("open");
        command.arg(path);
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        command = Command::new("xdg-open");
        command.arg(path);
    }
    let _ = command.spawn();
}

/// Les fichiers d'un jeu sont dans sa rom : on montre leur export, fait à la demande pour ce fichier. Rend ce que dit la fenêtre.
pub fn reveal(rom: &Path, entry_path: &str, open_file: bool) -> String {
    let code = match crate::library::read_rom(rom) {
        Ok(info) => info.code,
        Err(e) => return e,
    };
    let act = move |path: &Path| if open_file { open_with_system(path) } else { show_in_folder(path) };
    if let Some(path) = exported_file(&code, entry_path) {
        act(&path);
        return t!("Opened: {}", "Ouvert : {}", path.display());
    }
    if !installed() {
        return tr("This file is inside the rom. Download the mod tools (Modding tab) to get it as a file.", "Ce fichier est dans la rom. Telechargez les outils de mod (onglet Moding) pour l'avoir en fichier.").to_string();
    }
    if running() {
        return tr("The mod tools are busy: try again in a moment.", "Les outils de mod travaillent : reessayez dans un instant.").to_string();
    }
    let (rom, entry, game) = (rom.to_path_buf(), entry_path.to_string(), code.clone());
    start(Some(code), move || {
        run_tool("export", &game, &rom, &["--only", &entry])?;
        match exported_file(&game, &entry) {
            Some(path) => {
                act(&path);
                Ok(())
            }
            // Une archive s'exporte par ses fichiers : son dossier
            None => {
                let folder = files_dir(&game).join(entry.split('#').next().unwrap_or(&entry).replace('/', std::path::MAIN_SEPARATOR_STR));
                if folder.is_dir() {
                    act(&folder);
                    return Ok(());
                }
                act(&mod_dir(&game));
                Ok(())
            }
        }
    });
    tr("Exporting this file from the rom...", "Export de ce fichier depuis la rom...").to_string()
}

/// Le compte rendu du dernier import.
#[derive(serde::Deserialize, Default)]
pub struct Report {
    pub applied: Vec<String>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub export_only: Vec<String>,
}

pub fn report(code: &str) -> Option<Report> {
    serde_json::from_slice(&std::fs::read(mod_dir(code).join(REPORT)).ok()?).ok()
}

/// Résumé pour la page : une ligne et les premières erreurs.
pub fn report_lines(code: &str) -> Vec<String> {
    const SHOWN: usize = 6;
    let Some(report) = report(code) else { return Vec::new() };
    let mut lines = vec![t!("Last import: {} files changed, {} errors, {} notes", "Dernier import : {} fichiers changes, {} erreurs, {} remarques", report.applied.len(), report.errors.len(), report.warnings.len())];
    if !report.export_only.is_empty() {
        lines.push(t!("{} files cannot go back into the game yet (previews): first one: {}", "{} fichiers ne peuvent pas encore revenir dans le jeu (apercus) : le premier : {}", report.export_only.len(), report.export_only[0]));
    }
    lines.extend(report.errors.iter().chain(&report.warnings).take(SHOWN).cloned());
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_original_rom_is_played_unless_the_mods_are_on() {
        // Code propre à ce test : pas de dossier de mod, donc la rom de l'utilisateur
        let rom = Path::new("x/mon jeu.nds");
        assert_eq!(rom_to_play("TEST-pas-de-mod", rom), rom);
        assert!(!is_on("TEST-pas-de-mod"));
    }

    #[test]
    fn the_version_is_one_number() {
        assert!(VERSION.trim().parse::<u32>().is_ok());
    }
}
