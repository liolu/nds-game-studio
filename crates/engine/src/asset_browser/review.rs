//! Validation à l'œil des fichiers des jeux : l'aperçu de la fenêtre Assets (3D, son, vidéo compris) passe de fichier en
//! fichier, jeu après jeu, et des boutons disent ce qui ne va pas, selon ce qui est montré.
//!
//! - quelques fichiers de chaque format de chaque jeu (`per_format`, 0 = tous), dans l'ordre de la rom ;
//! - le verdict va dans `rapport.jsonl` (ajouté à chaque fichier, le dernier verdict d'un fichier compte) et `RAPPORT.md`
//!   (le résumé à lire pour corriger) ; relancer reprend où on en était ;
//! - un fichier qui ne s'affiche pas est noté tout seul, et un fichier qui fait tomber l'app l'est au lancement suivant.
//!
//! Une app à part, dans son propre processus (un fichier qui la fait tomber laisse le studio ouvert) : le bouton d'Options >
//! Développement, visible avec l'accès développeur, lance `nds-game-studio --asset-review`.
//!
//! `--asset-review [--lang=fr] [--out=<dossier du rapport>] [--roms=<dossier>...] [--per-format=3] [--autotest=20] [--unfocused]`

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::PathBuf;

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::ui::Pressed;
use bevy::ui_widgets::{Activate, Button};
use serde::{Deserialize, Serialize};

use super::{item_node, label, render_preview, AssetBrowser, PreviewSound, BG, DIM, ITEM, ITEM_HOVER, SIDEBAR, TEXT};
use crate::asset_scan::{find_games, kind_text, size_text, Entry, GameDir};
use crate::i18n::{set_language, t, tr, I18nPlugin, Lang, Language};
use crate::video::{VideoAudio, VideoPlugin};

const PANEL_W: f32 = 430.0;
const OK_COLOR: Color = Color::srgb(0.16, 0.42, 0.22);
const OK_HOVER: Color = Color::srgb(0.22, 0.55, 0.3);
const BAD_COLOR: Color = Color::srgb(0.42, 0.24, 0.13);
const BAD_HOVER: Color = Color::srgb(0.58, 0.33, 0.17);
/// Images d'affichage avant de considérer qu'un fichier n'a pas fait tomber l'app.
const SURVIVED_FRAMES: u32 = 5;
/// Images d'affichage avant d'accepter un verdict.
const MIN_FRAMES_SHOWN: u32 = 3;
const AUTOTEST_SHOT_FRAME: u32 = 15;
const AUTOTEST_FRAMES: u32 = 30;

/// Ce que l'aperçu montre : les problèmes possibles en dépendent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum View {
    Image,
    Anim2d,
    Model,
    Anim3d,
    Sound,
    Video,
    Text,
    Failed,
}

impl View {
    fn name(self) -> &'static str {
        match self {
            View::Image => "image",
            View::Anim2d => "animation 2D",
            View::Model => "modele 3D",
            View::Anim3d => "animation 3D",
            View::Sound => "son",
            View::Video => "video",
            View::Text => "texte",
            View::Failed => "echec",
        }
    }

    /// Au plus 7 : `COMMON` suit, et les touches vont de 1 à 9.
    fn problems(self) -> &'static [Problem] {
        match self {
            View::Image => &[
                Problem("couleurs", "Wrong colors / palette", "Mauvaises couleurs / palette"),
                Problem("brouille", "Scrambled picture (noise)", "Image brouillee (bruit)"),
                Problem("rangement", "Tiles misplaced / wrong width", "Tuiles mal rangees / largeur"),
                Problem("transparence", "Wrong transparency", "Transparence fausse"),
                Problem("coupe", "Cut off / incomplete", "Image coupee / incomplete"),
                Problem("vide", "Empty / black", "Vide / noire"),
                Problem("pas_image", "Not a picture", "Ce n'est pas une image"),
            ],
            View::Anim2d => &[
                Problem("couleurs", "Wrong colors / palette", "Mauvaises couleurs / palette"),
                Problem("brouille", "Scrambled frames (noise)", "Images brouillees (bruit)"),
                Problem("placement", "Parts misplaced", "Morceaux mal places"),
                Problem("ordre", "Wrong frame order", "Mauvais ordre des images"),
                Problem("vitesse", "Too fast / too slow", "Trop rapide / trop lent"),
                Problem("transparence", "Wrong transparency", "Transparence fausse"),
                Problem("vide", "Empty / black", "Vide / noire"),
            ],
            View::Model => &[
                Problem("textures", "Wrong / missing textures", "Textures fausses / absentes"),
                Problem("couleurs", "Wrong colors", "Couleurs fausses"),
                Problem("forme", "Broken shape (triangles)", "Forme cassee (triangles)"),
                Problem("manque", "Missing parts", "Morceaux manquants"),
                Problem("transparence", "Wrong transparency", "Transparence fausse"),
                Problem("orientation", "Wrong size / orientation", "Mauvaise taille / orientation"),
                Problem("vide", "Nothing on screen", "Rien a l'ecran"),
            ],
            View::Anim3d => &[
                Problem("deforme", "The animation distorts the model", "L'animation deforme le modele"),
                Problem("mauvais_modele", "Wrong model", "Mauvais modele"),
                Problem("vitesse", "Too fast / too slow", "Trop rapide / trop lent"),
                Problem("immobile", "Does not move", "Ne bouge pas"),
                Problem("textures", "Wrong / missing textures", "Textures fausses / absentes"),
                Problem("manque", "Missing parts", "Morceaux manquants"),
                Problem("vide", "Nothing on screen", "Rien a l'ecran"),
            ],
            View::Sound => &[
                Problem("muet", "No sound", "Pas de son"),
                Problem("sature", "Crackling / saturated", "Gresille / sature"),
                Problem("vitesse", "Wrong speed or pitch", "Trop rapide / trop grave ou aigu"),
                Problem("notes", "Wrong notes or instruments", "Notes ou instruments faux"),
                Problem("coupe", "Cut before the end", "Coupe avant la fin"),
                Problem("bruit", "Pure noise", "Bruit pur"),
            ],
            View::Video => &[
                Problem("image", "Broken picture", "Image cassee"),
                Problem("couleurs", "Wrong colors", "Couleurs fausses"),
                Problem("muet", "No sound", "Pas de son"),
                Problem("son_faux", "Wrong sound", "Son faux"),
                Problem("desynchro", "Sound out of sync", "Son decale"),
                Problem("saccades", "Stutters / too fast", "Saccades / trop rapide"),
                Problem("coupe", "Cut before the end", "Coupe avant la fin"),
            ],
            View::Text => &[
                Problem("illisible", "Unreadable characters", "Caracteres illisibles"),
                Problem("octets", "Only bytes: format not read", "Que des octets : format pas lu"),
                Problem("coupe", "Text cut off", "Texte coupe"),
                Problem("autre_contenu", "Should be a picture or a sound", "Devrait etre une image ou un son"),
            ],
            View::Failed => &[Problem("erreur", "Does not show", "Ne s'affiche pas")],
        }
    }
}

/// Un problème : son code, son nom en anglais et en français (le rapport garde le français).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Problem(&'static str, &'static str, &'static str);

const OTHER: Problem = Problem("autre", "Other (type a note)", "Autre (tape une note)");
const COMMON: &[Problem] = &[Problem("doute", "Not sure", "Pas sur"), OTHER];

fn problem_at(view: View, index: usize) -> Option<Problem> {
    view.problems().iter().chain(COMMON).nth(index).copied()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    Good,
    Problem(Problem),
    GoodFormat,
    SkipFormat,
    Back,
    Pass,
    Play,
}

#[derive(Component, Clone, Copy)]
struct ActionButton(Action);

#[derive(Component)]
struct HeadText;

#[derive(Component)]
struct NoteText;

/// Une ligne de `rapport.jsonl`. Un chemin en `*` est une décision sur tout un format (ou le jeu fini).
#[derive(Serialize, Deserialize, Clone)]
struct Record {
    game: String,
    path: String,
    #[serde(default)]
    member: Option<String>,
    #[serde(default)]
    rom_index: Option<u32>,
    format: String,
    kind: String,
    size: usize,
    view: String,
    verdict: String,
    label: String,
    #[serde(default)]
    note: String,
    #[serde(default)]
    info: String,
    /// La palette, les tuiles... ont été changées à la main avant le verdict : `choices` est le bon réglage.
    #[serde(default)]
    tweaked: bool,
    #[serde(default)]
    choices: Vec<String>,
}

#[derive(PartialEq, Eq, Debug)]
enum Phase {
    Setup,
    Loading,
    Showing,
    Finished,
}

type ChoiceState = (usize, usize, usize, usize, u32, usize, usize, usize, usize);

#[derive(Resource)]
struct Review {
    rom_dirs: Vec<PathBuf>,
    per_format: usize,
    report: PathBuf,
    report_md: PathBuf,
    current_file: PathBuf,
    autotest: Option<usize>,
    autotest_done: usize,
    records: BTreeMap<(String, String), Record>,
    phase: Phase,
    game: usize,
    open_last: bool,
    queue: Vec<usize>,
    group_sizes: HashMap<String, usize>,
    pos: usize,
    seen_serial: u64,
    view: Option<View>,
    guess: Option<ChoiceState>,
    frames: u32,
    note: String,
    pending: Option<Action>,
    buttons: Option<Entity>,
    buttons_for: Option<View>,
}

/// Fichiers montrés par format et par jeu si rien n'est demandé.
const DEFAULT_PER_FORMAT: usize = 3;

/// L'option qui lance cette app à la place du studio.
pub const REVIEW_ARG: &str = "--asset-review";

/// Le rapport, par défaut.
pub fn default_out_dir() -> PathBuf {
    crate::save::data_dir().join("validation_assets")
}

/// L'app : les arguments de la ligne de commande choisissent les roms et le rapport.
pub fn run() {
    let (mut rom_dirs, mut out_dir, mut per_format, mut autotest, mut unfocused) = (Vec::new(), default_out_dir(), DEFAULT_PER_FORMAT, None, false);
    let mut language = Lang::default();
    for arg in std::env::args().skip(1) {
        if let Some(v) = arg.strip_prefix("--roms=") {
            rom_dirs.push(PathBuf::from(v));
        } else if let Some(v) = arg.strip_prefix("--out=") {
            out_dir = PathBuf::from(v);
        } else if let Some(v) = arg.strip_prefix("--per-format=") {
            per_format = v.parse().unwrap_or(DEFAULT_PER_FORMAT);
        } else if let Some(v) = arg.strip_prefix("--autotest=") {
            autotest = v.parse().ok();
        } else if let Some(v) = arg.strip_prefix("--lang=") {
            language = Lang::from_code(v);
        } else if arg == "--unfocused" {
            unfocused = true;
        }
    }
    if rom_dirs.is_empty() {
        rom_dirs.push(crate::library::rom_dir());
    }
    set_language(&mut Language::default(), language);
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin { primary_window: None, ..default() }))
        .add_plugins((I18nPlugin, VideoPlugin, super::AssetBrowserPlugin, ReviewPlugin { rom_dirs, out_dir, per_format, autotest, unfocused }))
        .run();
}

/// Ouvre la fenêtre Assets en mode validation.
pub struct ReviewPlugin {
    pub rom_dirs: Vec<PathBuf>,
    pub out_dir: PathBuf,
    pub per_format: usize,
    /// Essai sans clavier : ce nombre de fichiers est jugé tout seul, avec une capture de chacun.
    pub autotest: Option<usize>,
    /// La fenêtre ne prend pas le clavier en s'ouvrant (essais).
    pub unfocused: bool,
}

impl Plugin for ReviewPlugin {
    fn build(&self, app: &mut App) {
        std::fs::create_dir_all(&self.out_dir).expect("dossier du rapport");
        let mut review = Review {
            rom_dirs: self.rom_dirs.clone(),
            per_format: self.per_format,
            report: self.out_dir.join("rapport.jsonl"),
            report_md: self.out_dir.join("RAPPORT.md"),
            current_file: self.out_dir.join("en_cours.txt"),
            autotest: self.autotest,
            autotest_done: 0,
            records: BTreeMap::new(),
            phase: Phase::Setup,
            game: 0,
            open_last: false,
            queue: Vec::new(),
            group_sizes: HashMap::new(),
            pos: 0,
            seen_serial: 0,
            view: None,
            guess: None,
            frames: 0,
            note: String::new(),
            pending: None,
            buttons: None,
            buttons_for: None,
        };
        review.load();
        let mut browser = app.world_mut().resource_mut::<AssetBrowser>();
        browser.review = true;
        browser.review_unfocused = self.unfocused || self.autotest.is_some();
        browser.index_started = true;
        browser.start = Some((String::new(), None));
        app.insert_resource(review).add_systems(Update, (read_keys, tick, show_texts, button_looks).chain().after(render_preview));
    }
}

fn group_of(entry: &Entry) -> String {
    format!("{}|{}", entry.format(), entry.kind.label())
}

fn format_key(group: &str) -> String {
    format!("*{group}")
}

const GAME_DONE_KEY: &str = "*fini";

/// Les fichiers à regarder : `per_format` de chaque format, répartis du début à la fin, dans l'ordre de la rom.
fn build_queue(entries: &[Entry], per_format: usize) -> (Vec<usize>, HashMap<String, usize>) {
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, entry) in entries.iter().enumerate() {
        if entry.kind.viewable() {
            groups.entry(group_of(entry)).or_default().push(i);
        }
    }
    let sizes = groups.iter().map(|(group, list)| (group.clone(), list.len())).collect();
    let mut queue = Vec::new();
    for list in groups.values() {
        if per_format == 0 || list.len() <= per_format {
            queue.extend_from_slice(list);
        } else {
            queue.extend((0..per_format).map(|k| list[k * (list.len() - 1) / (per_format - 1).max(1)]));
        }
    }
    queue.sort_unstable();
    (queue, sizes)
}

fn choice_state(b: &AssetBrowser) -> ChoiceState {
    let c = &b.choices;
    (c.palette, c.tile, c.texture, c.bank, c.width, c.cell, c.model, c.link, c.anim)
}

/// Le réglage montré, pour pouvoir le refaire.
fn choice_text(b: &AssetBrowser) -> Vec<String> {
    let c = &b.choices;
    let mut out = Vec::new();
    let mut named = |what: &str, list: &[usize], at: usize| {
        if let Some(&i) = list.get(at) {
            out.push(format!("{what}={}", b.entries[i].path));
        }
    };
    named("palette", &c.palettes, c.palette);
    named("tuiles", &c.tiles, c.tile);
    if c.texture > 0 {
        named("textures", &c.textures, c.texture - 1);
    }
    named("lien", &c.linked, c.link);
    if c.banks > 1 {
        out.push(format!("couleurs={}/{}", c.bank + 1, c.banks));
    }
    if c.width > 0 {
        out.push(format!("largeur={}", c.width));
    }
    if c.cells > 1 {
        out.push(format!("cellule={}/{}", c.cell + 1, c.cells));
    }
    if c.models > 1 {
        out.push(format!("modele={}/{}", c.model + 1, c.models));
    }
    if c.anims > 1 {
        out.push(format!("anim={}/{}", c.anim + 1, c.anims));
    }
    out
}

fn view_of(b: &AssetBrowser) -> View {
    if b.video.is_some() {
        View::Video
    } else if b.sound.is_some() {
        View::Sound
    } else if b.is3d {
        if b.model_anim.is_some() { View::Anim3d } else { View::Model }
    } else if !b.text_lines.is_empty() {
        View::Text
    } else if b.frames.len() > 1 {
        View::Anim2d
    } else if b.image.is_some() {
        View::Image
    } else {
        View::Failed
    }
}

impl Review {
    /// Reprend le rapport ; un fichier resté « en cours » a fait tomber l'app.
    fn load(&mut self) {
        if let Ok(text) = std::fs::read_to_string(&self.report) {
            for record in text.lines().filter_map(|line| serde_json::from_str::<Record>(line).ok()) {
                self.records.insert((record.game.clone(), record.path.clone()), record);
            }
        }
        if let Some((game, path)) = std::fs::read_to_string(&self.current_file).ok().and_then(|t| t.split_once('\t').map(|(a, b)| (a.to_string(), b.to_string()))) {
            if !self.records.contains_key(&(game.clone(), path.clone())) {
                let format = path.rsplit_once('.').map_or("?", |(_, ext)| ext).to_string();
                self.add(Record { game, path, member: None, rom_index: None, format, kind: String::new(), size: 0, view: "plantage".into(), verdict: "plantage".into(), label: "A fait tomber l'app".into(), note: String::new(), info: String::new(), tweaked: false, choices: Vec::new() });
                self.write_summary();
            }
            let _ = std::fs::remove_file(&self.current_file);
        }
    }

    fn add(&mut self, record: Record) {
        if let (Ok(line), Ok(mut file)) = (serde_json::to_string(&record), std::fs::OpenOptions::new().create(true).append(true).open(&self.report)) {
            let _ = writeln!(file, "{line}");
        }
        self.records.insert((record.game.clone(), record.path.clone()), record);
    }

    fn format_decided(&self, game: &str, group: &str) -> bool {
        self.records.contains_key(&(game.to_string(), format_key(group)))
    }

    fn record_of(&self, b: &AssetBrowser, entry: &Entry, game: &str) -> Record {
        let view = self.view.unwrap_or(View::Failed);
        Record {
            game: game.to_string(),
            path: entry.path.clone(),
            member: entry.member_name.clone(),
            rom_index: entry.rom_index,
            format: entry.format(),
            kind: entry.kind.label().to_string(),
            size: entry.size,
            view: view.name().to_string(),
            verdict: String::new(),
            label: String::new(),
            note: self.note.clone(),
            info: b.info.clone(),
            tweaked: self.guess.is_some_and(|guess| guess != choice_state(b)),
            choices: choice_text(b),
        }
    }

    fn judge(&mut self, b: &AssetBrowser, verdict: &str, label: &str) {
        let Some(&sel) = self.queue.get(self.pos) else { return };
        let game = b.games[self.game].label.clone();
        let mut record = self.record_of(b, &b.entries[sel], &game);
        record.verdict = verdict.to_string();
        record.label = label.to_string();
        self.add(record);
        self.note.clear();
    }

    fn decide_format(&mut self, b: &AssetBrowser, verdict: &str, label: &str) {
        let Some(&sel) = self.queue.get(self.pos) else { return };
        let entry = &b.entries[sel];
        let game = b.games[self.game].label.clone();
        let group = group_of(entry);
        let mut record = self.record_of(b, entry, &game);
        record.path = format_key(&group);
        record.member = None;
        record.rom_index = None;
        record.verdict = verdict.to_string();
        record.label = label.to_string();
        record.size = self.group_sizes.get(&group).copied().unwrap_or(0);
        self.add(record);
        self.note.clear();
    }

    fn is_done(&self, b: &AssetBrowser, game: &str, sel: usize) -> bool {
        let entry = &b.entries[sel];
        self.records.contains_key(&(game.to_string(), entry.path.clone())) || self.format_decided(game, &group_of(entry))
    }

    fn enter_game(&mut self, b: &mut AssetBrowser, game: usize) {
        self.game = game;
        self.phase = Phase::Loading;
        self.queue.clear();
        b.open_game(game);
    }

    fn enter_next_game(&mut self, b: &mut AssetBrowser, from: usize) {
        let next = (from..b.games.len()).find(|&g| !self.records.contains_key(&(b.games[g].label.clone(), GAME_DONE_KEY.to_string())));
        match next {
            Some(game) => self.enter_game(b, game),
            None => {
                self.phase = Phase::Finished;
                self.queue.clear();
                b.selected = None;
                b.preview_dirty = true;
                self.write_summary();
            }
        }
    }

    fn show(&mut self, b: &mut AssetBrowser, pos: usize) {
        self.pos = pos;
        self.phase = Phase::Showing;
        self.guess = None;
        self.view = None;
        self.frames = 0;
        self.note.clear();
        let entry = &b.entries[self.queue[pos]];
        let _ = std::fs::write(&self.current_file, format!("{}\t{}", b.games[self.game].label, entry.path));
        let sel = self.queue[pos];
        b.select(sel);
    }

    /// Le fichier suivant à juger, ou le jeu suivant. Un jeu n'est fini que si aucun fichier n'a été passé.
    fn next_file(&mut self, b: &mut AssetBrowser, from: usize) {
        let game = b.games[self.game].label.clone();
        match (from..self.queue.len()).find(|&p| !self.is_done(b, &game, self.queue[p])) {
            Some(pos) => self.show(b, pos),
            None => {
                let _ = std::fs::remove_file(&self.current_file);
                if (0..self.queue.len()).all(|p| self.is_done(b, &game, self.queue[p])) {
                    self.add(Record { game: game.clone(), path: GAME_DONE_KEY.to_string(), member: None, rom_index: None, format: String::new(), kind: String::new(), size: self.queue.len(), view: String::new(), verdict: "jeu_fini".into(), label: "Jeu fini".into(), note: String::new(), info: String::new(), tweaked: false, choices: Vec::new() });
                }
                self.write_summary();
                self.enter_next_game(b, self.game + 1);
            }
        }
    }

    fn apply(&mut self, action: Action, b: &mut AssetBrowser) {
        match action {
            Action::Good => {
                self.judge(b, "bon", "Bon");
                self.write_summary();
                self.next_file(b, self.pos + 1);
            }
            Action::Problem(Problem(verdict, _, text)) => {
                self.judge(b, verdict, text);
                self.write_summary();
                self.next_file(b, self.pos + 1);
            }
            Action::GoodFormat => {
                self.judge(b, "bon", "Bon");
                self.decide_format(b, "bon_format", "Bon : tout le format dans ce jeu");
                self.write_summary();
                self.next_file(b, self.pos + 1);
            }
            Action::SkipFormat => {
                self.decide_format(b, "saute_format", "Format saute dans ce jeu");
                self.write_summary();
                self.next_file(b, self.pos + 1);
            }
            Action::Pass => self.next_file(b, self.pos + 1),
            Action::Back => {
                if self.pos > 0 {
                    let pos = self.pos - 1;
                    self.show(b, pos);
                } else if self.game > 0 {
                    self.open_last = true;
                    let game = self.game - 1;
                    self.enter_game(b, game);
                }
            }
            Action::Play => {}
        }
    }

    /// Le résumé pour corriger : les problèmes d'abord, par sorte et format, puis ce qui est bon.
    fn write_summary(&self) {
        let files: Vec<&Record> = self.records.values().filter(|r| !r.path.starts_with('*')).collect();
        let is_good = |r: &&Record| r.verdict == "bon";
        let is_auto = |r: &&Record| matches!(r.verdict.as_str(), "erreur_auto" | "plantage");
        let problems: Vec<&Record> = files.iter().copied().filter(|r| !is_good(r) && !is_auto(r)).collect();
        let good = files.iter().copied().filter(is_good).count();
        let mut text = format!("# Validation des fichiers\n\n{} fichiers juges : {good} bons, {} problemes, {} erreurs automatiques.\n", files.len(), problems.len(), files.iter().copied().filter(is_auto).count());

        let mut by_problem: BTreeMap<(&str, &str, &str, &str), Vec<&Record>> = BTreeMap::new();
        for record in &problems {
            by_problem.entry((record.view.as_str(), record.label.as_str(), record.format.as_str(), record.verdict.as_str())).or_default().push(record);
        }
        text.push_str("\n## Problemes\n");
        for ((view, label, format, _), records) in &by_problem {
            text += &format!("\n### {view} : {label} : .{format} ({})\n", records.len());
            for r in records {
                text += &line_of(r);
            }
        }

        text.push_str("\n## Erreurs automatiques\n\n");
        for r in files.iter().copied().filter(is_auto) {
            text += &line_of(r);
        }

        text.push_str("\n## Bons, avec un reglage change a la main (la devinette est fausse)\n\n");
        for r in files.iter().copied().filter(|r| is_good(r) && r.tweaked) {
            text += &line_of(r);
        }

        text.push_str("\n## Bons par format\n\n");
        let mut by_format: BTreeMap<(&str, &str), usize> = BTreeMap::new();
        for r in files.iter().copied().filter(is_good) {
            *by_format.entry((r.format.as_str(), r.kind.as_str())).or_default() += 1;
        }
        for ((format, kind), count) in by_format {
            text += &format!("- .{format} ({kind}) : {count}\n");
        }

        text.push_str("\n## Formats decides d'un coup\n\n");
        for r in self.records.values().filter(|r| r.path.starts_with('*') && r.verdict != "jeu_fini") {
            text += &format!("- {} : .{} ({}) : {} ({} fichiers)\n", r.game, r.format, r.kind, r.label, r.size);
        }
        let _ = std::fs::write(&self.report_md, text);
    }
}

fn line_of(r: &Record) -> String {
    let mut line = format!("- {} : `{}`", r.game, r.path);
    if let Some(member) = &r.member {
        line += &format!(" ({member})");
    }
    if r.verdict == "erreur_auto" || r.verdict == "plantage" {
        line += &format!(" : {}", r.label);
    }
    if !r.note.is_empty() {
        line += &format!(" ; note : {}", r.note);
    }
    if r.tweaked {
        line += " ; reglage change";
    }
    if !r.choices.is_empty() {
        line += &format!(" ; {}", r.choices.join(", "));
    }
    if r.verdict == "erreur_auto" {
        line += &format!(" ; {}", r.info.replace('\n', " "));
    }
    line.push('\n');
    line
}

/// Les roms de tous les dossiers, triées par nom, chacune une fois.
fn gather_games(dirs: &[PathBuf]) -> Vec<GameDir> {
    let mut games: Vec<GameDir> = dirs.iter().flat_map(|dir| find_games(dir)).collect();
    games.sort_by_key(|g| g.label.to_lowercase());
    games.dedup_by(|a, b| a.label == b.label);
    games
}

fn read_keys(mut r: ResMut<Review>, b: Res<AssetBrowser>, mut typed: MessageReader<KeyboardInput>) {
    let Some(window) = b.window else { return };
    for key in typed.read() {
        if key.window != window || !key.state.is_pressed() {
            continue;
        }
        let digit = match key.key_code {
            KeyCode::Digit0 | KeyCode::Numpad0 => Some(0),
            KeyCode::Digit1 | KeyCode::Numpad1 => Some(1),
            KeyCode::Digit2 | KeyCode::Numpad2 => Some(2),
            KeyCode::Digit3 | KeyCode::Numpad3 => Some(3),
            KeyCode::Digit4 | KeyCode::Numpad4 => Some(4),
            KeyCode::Digit5 | KeyCode::Numpad5 => Some(5),
            KeyCode::Digit6 | KeyCode::Numpad6 => Some(6),
            KeyCode::Digit7 | KeyCode::Numpad7 => Some(7),
            KeyCode::Digit8 | KeyCode::Numpad8 => Some(8),
            KeyCode::Digit9 | KeyCode::Numpad9 => Some(9),
            _ => None,
        };
        // Les chiffres servent de touches tant qu'aucune note n'est commencée (sur AZERTY, sans Maj aussi)
        if let (Some(digit), true) = (digit, r.note.is_empty()) {
            if !key.repeat {
                let view = r.view;
                r.pending = match digit {
                    0 => Some(Action::GoodFormat),
                    n => view.and_then(|v| problem_at(v, n - 1)).map(Action::Problem),
                };
            }
            continue;
        }
        match &key.logical_key {
            Key::Enter => {
                if !key.repeat {
                    r.pending = Some(if r.note.is_empty() { Action::Good } else { Action::Problem(OTHER) });
                }
            }
            Key::ArrowLeft => r.pending = Some(Action::Back),
            Key::ArrowRight => r.pending = Some(Action::Pass),
            Key::Space if r.note.is_empty() => r.pending = Some(Action::Play),
            Key::Backspace => {
                r.note.pop();
            }
            Key::Escape => r.note.clear(),
            _ => {
                if let Some(text) = &key.text {
                    let text: String = text.chars().filter(|c| !c.is_control()).collect();
                    r.note.push_str(&text);
                }
            }
        }
    }
}

fn play_or_stop(b: &mut AssetBrowser, commands: &mut Commands, sources: &mut Assets<VideoAudio>, sounds: &Query<Entity, With<PreviewSound>>) {
    if let Some(video) = &mut b.video {
        if video.is_playing() {
            video.stop(commands);
        } else {
            video.play(commands, sources);
        }
        return;
    }
    let mut stopped = false;
    for sound in sounds {
        commands.entity(sound).despawn();
        stopped = true;
    }
    if let (false, Some(sound)) = (stopped, &b.sound) {
        commands.spawn((AudioPlayer::new(sound.clone()), PlaybackSettings::DESPAWN, PreviewSound));
    }
}

/// Remplace la colonne de la liste par celle de la validation.
fn build_panel(r: &mut Review, b: &AssetBrowser, commands: &mut Commands, parents: &Query<&ChildOf>, children: &Query<&Children>, nodes: &mut Query<&mut Node>) -> bool {
    let Some(image_box) = b.image_box else { return false };
    let Ok(panel) = parents.get(image_box) else { return false };
    let Ok(root) = parents.get(panel.parent()) else { return false };
    let root = root.parent();
    let Ok(sides) = children.get(root) else { return false };
    let Some(&sidebar) = sides.first() else { return false };
    if let Ok(mut node) = nodes.get_mut(sidebar) {
        node.display = Display::None;
    }
    let mut buttons = None;
    let column = commands
        .spawn((Node { width: px(PANEL_W), flex_shrink: 0.0, height: percent(100), flex_direction: FlexDirection::Column, padding: UiRect::all(px(12)), row_gap: px(10), ..default() }, BackgroundColor(SIDEBAR)))
        .with_children(|column| {
            column.spawn((label("", 15.0, TEXT), HeadText));
            buttons = Some(column.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(5), ..default() }).id());
            column.spawn((label("", 14.0, Color::srgb(0.95, 0.8, 0.45)), NoteText));
            column.spawn(label(tr("Typing = a note on this file.\nEnter = Good (or Other if there is a note).\nSpace = play / stop.\nArrows = previous / skip without judging.\nDrag and wheel = turn and zoom the 3D.\nClose the window: the next launch resumes here.", "Taper du texte = note sur ce fichier.\nEntree = Bon (ou Autre s'il y a une note).\nEspace = jouer / arreter.\nFleches = precedent / passer sans juger.\nGlisser et molette = tourner et zoomer la 3D.\nFermer la fenetre : on reprend ici au prochain lancement."), 12.0, DIM));
            column.spawn(label(t!("Report: {}", "Rapport : {}", r.report_md.display()), 11.0, DIM));
        })
        .id();
    commands.entity(root).insert_children(0, &[column]);
    r.buttons = buttons;
    true
}

fn spawn_button(parent: &mut ChildSpawnerCommands, text: String, action: Action) {
    let color = match action {
        Action::Good | Action::GoodFormat => OK_COLOR,
        Action::Problem(..) => BAD_COLOR,
        _ => ITEM,
    };
    parent
        .spawn((Button, Hovered::default(), Node { justify_content: JustifyContent::FlexStart, ..item_node() }, BackgroundColor(color), ActionButton(action)))
        .observe(|activate: On<Activate>, buttons: Query<&ActionButton>, mut r: ResMut<Review>| {
            if let Ok(button) = buttons.get(activate.entity) {
                r.pending = Some(button.0);
            }
        })
        .with_child((label(text, 14.0, TEXT), Pickable::IGNORE));
}

fn rebuild_buttons(r: &mut Review, commands: &mut Commands, children: &Query<&Children>) {
    let Some(buttons) = r.buttons else { return };
    if r.buttons_for == r.view {
        return;
    }
    r.buttons_for = r.view;
    if let Ok(old) = children.get(buttons) {
        for child in old.iter() {
            commands.entity(child).despawn();
        }
    }
    let Some(view) = r.view else { return };
    commands.entity(buttons).with_children(|list| {
        spawn_button(list, tr("[Enter]  Good", "[Entree]  Bon").into(), Action::Good);
        spawn_button(list, tr("[0]  Good, and this whole format in this game", "[0]  Bon, et tout ce format dans ce jeu").into(), Action::GoodFormat);
        for (i, problem) in view.problems().iter().chain(COMMON).enumerate() {
            spawn_button(list, format!("[{}]  {}", i + 1, tr(problem.1, problem.2)), Action::Problem(*problem));
        }
        spawn_button(list, tr("Skip this format in this game", "Sauter ce format dans ce jeu").into(), Action::SkipFormat);
        if matches!(view, View::Sound | View::Video) {
            spawn_button(list, tr("[Space]  Play / stop", "[Espace]  Jouer / arreter").into(), Action::Play);
        }
        list.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(6), ..default() }).with_children(|row| {
            spawn_button(row, tr("[<-]  Previous", "[<-]  Precedent").into(), Action::Back);
            spawn_button(row, tr("[->]  Skip", "[->]  Passer").into(), Action::Pass);
        });
    });
}

#[allow(clippy::too_many_arguments)]
fn tick(
    mut commands: Commands,
    mut r: ResMut<Review>,
    mut b: ResMut<AssetBrowser>,
    mut sources: ResMut<Assets<VideoAudio>>,
    sounds: Query<Entity, With<PreviewSound>>,
    parents: Query<&ChildOf>,
    children: Query<&Children>,
    mut nodes: Query<&mut Node>,
    mut windows: Query<&mut Window>,
    action_buttons: Query<(Entity, &ActionButton)>,
    mut exit: MessageWriter<AppExit>,
) {
    let r = &mut *r;
    let b = &mut *b;
    if r.phase == Phase::Setup {
        if !build_panel(r, b, &mut commands, &parents, &children, &mut nodes) {
            return;
        }
        if let Some(mut window) = b.window.and_then(|w| windows.get_mut(w).ok()) {
            window.title = tr("Validate the game files", "Validation des fichiers des jeux").into();
        }
        b.games = gather_games(&r.rom_dirs);
        b.close_menu();
        r.write_summary();
        r.enter_next_game(b, 0);
        return;
    }
    if r.phase == Phase::Loading {
        if b.scanning.is_some() || b.game != Some(r.game) {
            return;
        }
        (r.queue, r.group_sizes) = build_queue(&b.entries, r.per_format);
        if std::mem::take(&mut r.open_last) && !r.queue.is_empty() {
            let last = r.queue.len() - 1;
            r.show(b, last);
        } else {
            r.next_file(b, 0);
        }
        return;
    }
    if r.phase == Phase::Showing && b.preview_serial != r.seen_serial && !b.preview_dirty {
        r.seen_serial = b.preview_serial;
        let view = view_of(b);
        r.view = Some(view);
        if r.guess.is_none() {
            r.guess = Some(choice_state(b));
            r.frames = 0;
        }
        rebuild_buttons(r, &mut commands, &children);
        let game = b.games[r.game].label.clone();
        let path = r.queue.get(r.pos).map(|&s| b.entries[s].path.clone()).unwrap_or_default();
        // Un fichier qui ne s'affiche pas se note tout seul (revenir dessus avec Précédent le montre quand même)
        if view == View::Failed && !r.records.contains_key(&(game, path)) {
            r.judge(b, "erreur_auto", "Erreur de lecture");
            r.write_summary();
            r.next_file(b, r.pos + 1);
            return;
        }
        if matches!(view, View::Sound | View::Video) && r.autotest.is_none() {
            let silent = sounds.is_empty();
            if silent {
                play_or_stop(b, &mut commands, &mut sources, &sounds);
            }
        }
    }
    if r.phase == Phase::Showing {
        r.frames += 1;
        if r.frames == SURVIVED_FRAMES {
            let _ = std::fs::remove_file(&r.current_file);
        }
    }
    if let Some(remaining) = r.autotest {
        if r.phase == Phase::Finished || r.autotest_done >= remaining {
            exit.write(AppExit::Success);
            return;
        }
        if r.phase == Phase::Showing && r.frames == AUTOTEST_SHOT_FRAME {
            let path = r.report.with_file_name(format!("auto_{:03}.png", r.autotest_done));
            if let Some(window) = b.window {
                commands.spawn(Screenshot::window(window)).observe(save_to_disk(path));
            }
        }
        if r.phase == Phase::Showing && r.frames == AUTOTEST_FRAMES && r.view.is_some() {
            r.autotest_done += 1;
            // Un clic sur un vrai bouton : à tour de rôle Bon et le premier problème
            let wanted = |a: &Action| if r.autotest_done % 2 == 0 { matches!(a, Action::Problem(..)) } else { *a == Action::Good };
            if let Some((button, _)) = action_buttons.iter().find(|(_, a)| wanted(&a.0)) {
                commands.trigger(Activate { entity: button });
            }
        }
    }
    let Some(action) = r.pending.take() else { return };
    // Une touche ou un clic parti pendant le décodage visait le fichier d'avant : il ne juge pas celui-ci
    if r.phase != Phase::Showing || r.view.is_none() || r.frames < MIN_FRAMES_SHOWN {
        return;
    }
    if action == Action::Play {
        play_or_stop(b, &mut commands, &mut sources, &sounds);
        return;
    }
    r.apply(action, b);
}

fn show_texts(r: Res<Review>, b: Res<AssetBrowser>, mut heads: Query<&mut Text, (With<HeadText>, Without<NoteText>)>, mut notes: Query<(&mut Text, &mut TextFont), (With<NoteText>, Without<HeadText>)>) {
    let judged = r.records.keys().filter(|(_, path)| !path.starts_with('*')).count();
    let problems = r.records.iter().filter(|((_, path), rec)| !path.starts_with('*') && rec.verdict != "bon").count();
    let game = b.games.get(r.game).map_or("", |g| g.label.as_str());
    let counts = t!("{judged} judged, {problems} with a problem", "{judged} juges, dont {problems} avec un probleme");
    let wanted = match r.phase {
        Phase::Setup => tr("Reading the roms...", "Lecture des roms...").to_string(),
        Phase::Finished if b.games.is_empty() => tr("No rom found.", "Aucune rom trouvee.").to_string(),
        Phase::Finished => t!("Finished: everything was seen.\n{counts}", "Fini : tout a ete vu.\n{counts}"),
        Phase::Loading => t!("Game {} / {}\n{game}\nReading the files...\n{counts}", "Jeu {} / {}\n{game}\nLecture des fichiers...\n{counts}", r.game + 1, b.games.len()),
        Phase::Showing => {
            let entry = r.queue.get(r.pos).and_then(|&s| b.entries.get(s));
            let (format, sample, former) = entry.map_or((String::new(), 0, None), |e| {
                let former = r.records.get(&(game.to_string(), e.path.clone())).map(|rec| rec.label.clone());
                (format!(".{}  ({}, {})", e.format(), kind_text(e.kind), size_text(e.size)), r.group_sizes.get(&group_of(e)).copied().unwrap_or(0), former)
            });
            let former = former.map_or(String::new(), |label| t!("\nAlready judged: {label}", "\nDeja juge : {label}"));
            t!("Game {} / {}\n{game}\nFile {} / {}\n{format}\n{sample} files of this format in this game\n{counts}{former}", "Jeu {} / {}\n{game}\nFichier {} / {}\n{format}\n{sample} fichiers de ce format dans ce jeu\n{counts}{former}", r.game + 1, b.games.len(), r.pos + 1, r.queue.len())
        }
    };
    if let Ok(mut text) = heads.single_mut() {
        if text.0 != wanted {
            text.0 = wanted;
        }
    }
    if let Ok((mut text, mut font)) = notes.single_mut() {
        let wanted = if r.note.is_empty() { String::new() } else { format!("Note : {}_", r.note) };
        if text.0 != wanted {
            text.0 = wanted;
        }
        // La police des aperçus a les accents
        if let Some(handle) = &b.text_font {
            let handle: bevy::text::FontSource = handle.clone().into();
            if font.font != handle {
                font.font = handle;
            }
        }
    }
}

fn button_looks(mut buttons: Query<(&Hovered, Has<Pressed>, &ActionButton, &mut BackgroundColor)>) {
    for (hovered, pressed, button, mut color) in &mut buttons {
        let (idle, hover) = match button.0 {
            Action::Good | Action::GoodFormat => (OK_COLOR, OK_HOVER),
            Action::Problem(..) => (BAD_COLOR, BAD_HOVER),
            _ => (ITEM, ITEM_HOVER),
        };
        let wanted = if pressed { BG } else if hovered.get() { hover } else { idle };
        if color.0 != wanted {
            color.0 = wanted;
        }
    }
}
