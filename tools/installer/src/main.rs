#![windows_subsystem = "windows"]

// À la racine du crate pour que `nwg` / `nwd` soient visibles partout
// (y compris dans le code généré par la macro NwgUi).
#[cfg(windows)]
extern crate native_windows_gui as nwg;
#[cfg(windows)]
extern crate native_windows_derive as nwd;

mod install;

#[cfg(windows)]
mod windows_gui;
#[cfg(not(windows))]
mod unix_gui;

fn main() {
    #[cfg(windows)]
    windows_gui::run();
    #[cfg(not(windows))]
    unix_gui::run();
}
