//! Le programme du bundle `NdsGameStudio.app` (macOS) : ouvre le launcher posé à côté du bundle.
//!
//! Un vrai programme et pas un script : la signature d'un script est gardée dans des attributs que
//! le zip ne conserve pas, et macOS prendrait alors le bundle pour une application endommagée.
//!
//! Si le launcher n'est pas là, le bundle a été déplacé sans le reste du dossier, ou macOS l'a isolé
//! dans un dossier temporaire parce qu'il vient d'internet (App Translocation). Sans ce message, le
//! double-clic ne ferait rien.

#[cfg(target_os = "macos")]
fn main() {
    use std::os::unix::process::CommandExt;

    const FILES_MISSING: &str = "Nds Game Studio can't find its files: macOS isolated this app after the download, or it was moved out of its folder.\n\n\
        Easiest: use nds-game-studio-installer-macos.zip from the GitHub releases.\n\
        Otherwise, put this app back in its folder and run in Terminal:\n\
        xattr -dr com.apple.quarantine <that folder>\n\n\
        Nds Game Studio ne trouve pas ses fichiers : macOS a isole cette application apres le telechargement, ou elle a ete sortie de son dossier.\n\n\
        Le plus simple : utilisez nds-game-studio-installer-macos.zip dans les releases GitHub.\n\
        Sinon, remettez cette application dans son dossier et lancez dans le Terminal :\n\
        xattr -dr com.apple.quarantine <ce dossier>";

    let exe = std::env::current_exe().unwrap_or_default();
    // <dossier>/NdsGameStudio.app/Contents/MacOS/NdsGameStudio
    let folder = exe.ancestors().nth(4).unwrap_or(std::path::Path::new("."));
    let launcher = folder.join("nds-game-studio-launcher");
    if !launcher.is_file() {
        alert(FILES_MISSING);
        return;
    }
    // Ne revient que si le launcher n'a pas pu démarrer
    let error = std::process::Command::new(&launcher).current_dir(folder).exec();
    alert(&format!("{}: {error}", launcher.display()));
}

/// Fenêtre d'erreur du système : une application ouverte depuis le Finder n'a pas de console.
#[cfg(target_os = "macos")]
fn alert(message: &str) {
    let _ = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", "on run argv", "-e", "display alert \"Nds Game Studio\" message (item 1 of argv)", "-e", "end run", message])
        .status();
}

#[cfg(not(target_os = "macos"))]
fn main() {}
