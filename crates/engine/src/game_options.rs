//! Réglages des jeux : touches, vitesse, son et langue, généraux et par jeu.
//!
//! - **Généraux** (Options > Jeux) : valent pour tous les jeux ;
//! - **Par jeu** (onglet Parametres de la page d'un jeu) : chaque valeur choisie pour un jeu passe avant la générale ;
//!   « Comme les reglages generaux » la retire. Une touche se change bouton par bouton : un bouton DS changé pour un
//!   jeu garde les autres boutons des réglages généraux.
//! - **Vitesse** : interrupteur du launcher (actif par défaut), qui ne joue que quand on joue avec des mods. En jeu, une
//!   touche passe de la vitesse normale à la vitesse rapide et revient, deux autres accélèrent et ralentissent pas à pas.
//!   Le module la reçoit dans le paquet des touches (mot 14, `present.c`) ; un runtime d'avant ne la lit pas (bit 1 des
//!   drapeaux de l'en-tête).
//! - **Son** : volume en pour cent du volume d'origine (paquet, mot 15).
//! - **Langue** : celle de la console que voit le jeu (`--ds-lang`, réglages de l'utilisateur du DS) ; « Celle du jeu »
//!   ne passe rien et laisse le jeu comme avant.
//!
//! Tout est gardé dans `game_options.toml` (dossier des données), écrit à chaque changement.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::{Pressed, ScrollPosition};
use bevy::ui_widgets::{Activate, Button};
use serde::{Deserialize, Serialize};

use crate::i18n::{t, tr};
use crate::input::{pad_buttons_of, DsButton, KeyMap};

/// Une couche de réglages : générale ou d'un jeu. `None` (ou absent) = pas choisi à ce niveau.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layer {
    /// Bouton DS (`a`, `start`, `up`...) -> touches (noms de `KeyCode` : `KeyX`, `ArrowUp`).
    pub keys: BTreeMap<String, Vec<String>>,
    /// `toggle`, `faster`, `slower` -> touche.
    pub speed_keys: BTreeMap<String, String>,
    /// Accélération permise (jeu avec mods seulement).
    pub speed: Option<bool>,
    /// Vitesse rapide de la touche « vitesse », en pour cent.
    pub fast_speed: Option<u32>,
    /// Volume en pour cent du volume d'origine.
    pub volume: Option<u32>,
    /// `game` (celle du jeu), `ja`, `en`, `fr`, `de`, `it`, `es`.
    pub language: Option<String>,
}

impl Layer {
    fn is_empty(&self) -> bool {
        *self == Layer::default()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OptionsFile {
    pub global: Layer,
    /// Code du jeu (`ADGE`) -> ce qui est choisi pour lui.
    pub games: BTreeMap<String, Layer>,
}

impl OptionsFile {
    pub fn parse(text: &str) -> Self {
        toml::from_str(text).unwrap_or_else(|e| {
            warn!("game_options.toml illisible, reglages par defaut : {e}");
            Self::default()
        })
    }

    fn layers<'a>(&'a self, code: Option<&str>) -> [Option<&'a Layer>; 2] {
        [code.and_then(|c| self.games.get(c)), Some(&self.global)]
    }

    /// Les touches d'un jeu (ou les générales) : par défaut, puis générales, puis celles du jeu, bouton par bouton.
    pub fn keymap(&self, code: Option<&str>) -> KeyMap {
        let mut map = KeyMap::default();
        for layer in self.layers(code).into_iter().rev().flatten() {
            for (button, names) in &layer.keys {
                let Some(button) = button_by_name(button) else { continue };
                let keys: Vec<KeyCode> = names.iter().filter_map(|n| crate::native_game::key_by_name(n)).collect();
                if !keys.is_empty() {
                    map.0.insert(button, keys);
                }
            }
        }
        map
    }

    pub fn speed_key(&self, code: Option<&str>, which: SpeedKey) -> KeyCode {
        self.layers(code).into_iter().flatten().find_map(|l| l.speed_keys.get(which.id())).and_then(|n| crate::native_game::key_by_name(n)).unwrap_or(which.default_key())
    }

    pub fn speed_enabled(&self, code: Option<&str>) -> bool {
        self.layers(code).into_iter().flatten().find_map(|l| l.speed).unwrap_or(true)
    }

    pub fn fast_speed(&self, code: Option<&str>) -> u32 {
        self.layers(code).into_iter().flatten().find_map(|l| l.fast_speed).unwrap_or(DEFAULT_FAST).clamp(SPEEDS[0], SPEEDS[SPEEDS.len() - 1])
    }

    pub fn volume(&self, code: Option<&str>) -> u32 {
        self.layers(code).into_iter().flatten().find_map(|l| l.volume).unwrap_or(100).min(MAX_VOLUME)
    }

    pub fn language(&self, code: Option<&str>) -> String {
        self.layers(code).into_iter().flatten().find_map(|l| l.language.clone()).unwrap_or_else(|| "game".into())
    }

    fn layer_mut(&mut self, code: Option<&str>) -> &mut Layer {
        match code {
            Some(code) => self.games.entry(code.to_string()).or_default(),
            None => &mut self.global,
        }
    }

    fn tidy(&mut self) {
        self.games.retain(|_, layer| !layer.is_empty());
    }
}

/// Vitesses proposées, en pour cent.
pub const SPEEDS: [u32; 9] = [25, 50, 75, 100, 150, 200, 300, 400, 800];
const DEFAULT_FAST: u32 = 200;
const MAX_VOLUME: u32 = 200;
const VOLUME_STEP: u32 = 10;
/// Langues de la console : code gardé, numéro du DS (`OS_GetLanguage`), nom.
pub const LANGUAGES: [(&str, Option<u8>, &str, &str); 7] = [
    ("game", None, "The game's", "Celle du jeu"),
    ("en", Some(1), "English", "Anglais"),
    ("fr", Some(2), "French", "Francais"),
    ("de", Some(3), "German", "Allemand"),
    ("it", Some(4), "Italian", "Italien"),
    ("es", Some(5), "Spanish", "Espagnol"),
    ("ja", Some(0), "Japanese", "Japonais"),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SpeedKey {
    Toggle,
    Faster,
    Slower,
}

impl SpeedKey {
    pub const ALL: [SpeedKey; 3] = [SpeedKey::Toggle, SpeedKey::Faster, SpeedKey::Slower];

    fn id(self) -> &'static str {
        match self {
            SpeedKey::Toggle => "toggle",
            SpeedKey::Faster => "faster",
            SpeedKey::Slower => "slower",
        }
    }

    pub fn default_key(self) -> KeyCode {
        match self {
            SpeedKey::Toggle => KeyCode::Tab,
            SpeedKey::Faster => KeyCode::PageUp,
            SpeedKey::Slower => KeyCode::PageDown,
        }
    }

    fn label(self) -> &'static str {
        match self {
            SpeedKey::Toggle => tr("Normal / fast speed", "Vitesse normale / rapide"),
            SpeedKey::Faster => tr("Faster", "Accelerer"),
            SpeedKey::Slower => tr("Slower", "Ralentir"),
        }
    }
}

fn button_id(button: DsButton) -> String {
    format!("{button:?}").to_lowercase()
}

fn button_by_name(name: &str) -> Option<DsButton> {
    DsButton::ALL.into_iter().find(|b| button_id(*b) == name)
}

// ─────────────────────────────────────────────────────────────────────────
//  Le fichier
// ─────────────────────────────────────────────────────────────────────────

static FILE: OnceLock<Mutex<OptionsFile>> = OnceLock::new();
static REVISION: AtomicU64 = AtomicU64::new(1);

pub fn path() -> std::path::PathBuf {
    crate::save::data_dir().join("game_options.toml")
}

fn file() -> &'static Mutex<OptionsFile> {
    FILE.get_or_init(|| Mutex::new(std::fs::read_to_string(path()).map(|t| OptionsFile::parse(&t)).unwrap_or_default()))
}

/// Une copie des réglages, à lire sans garder le verrou.
pub fn get() -> OptionsFile {
    file().lock().unwrap().clone()
}

/// Change les réglages, les écrit, et fait refaire les pages qui les montrent.
fn change(edit: impl FnOnce(&mut OptionsFile)) {
    let mut options = file().lock().unwrap();
    edit(&mut options);
    options.tidy();
    let text = toml::to_string_pretty(&*options).unwrap_or_default();
    drop(options);
    if let Err(e) = crate::save::write_atomic(&path(), &text) {
        warn!("game_options.toml non enregistre : {e}");
    }
    REVISION.fetch_add(1, Ordering::Relaxed);
}

/// Change à chaque modification : les pages se refont quand il change.
pub fn revision() -> u64 {
    REVISION.load(Ordering::Relaxed)
}

pub fn keymap(code: Option<&str>) -> KeyMap {
    get().keymap(code)
}

/// L'interrupteur de l'accélération (onglet Jouer d'un jeu, ou général).
pub fn set_speed(code: Option<&str>, on: bool) {
    change(|o| o.layer_mut(code).speed = Some(on));
}

/// Position de l'éditeur général (Options > Jeux), gardée quand la page se refait après un changement.
static OPTIONS_SCROLL: Mutex<f32> = Mutex::new(0.0);

pub fn saved_scroll() -> f32 {
    *OPTIONS_SCROLL.lock().unwrap()
}

/// Le numéro de langue à passer au module (`--ds-lang`), ou rien pour garder celle du jeu.
pub fn ds_language(code: &str) -> Option<u8> {
    let chosen = get().language(Some(code));
    LANGUAGES.iter().find(|(id, ..)| *id == chosen).and_then(|(_, n, ..)| *n)
}

// ─────────────────────────────────────────────────────────────────────────
//  L'éditeur, dans Options > Jeux (général) et dans les Parametres d'un jeu
// ─────────────────────────────────────────────────────────────────────────

/// Ce qui attend une touche : (jeu ou général, ce qu'on change).
#[derive(Clone, PartialEq, Eq, Debug)]
enum Target {
    Button(DsButton),
    Speed(SpeedKey),
}

static WAITING: Mutex<Option<(Option<String>, Target)>> = Mutex::new(None);

/// La touche attendue vient d'être prise à cette image : elle ne doit rien faire d'autre.
static CAUGHT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Une touche est attendue (ou vient d'être prise) : Échap annule et ne doit rien faire d'autre (retour du launcher).
pub fn waiting_for_key() -> bool {
    WAITING.lock().unwrap().is_some() || CAUGHT.load(Ordering::Relaxed)
}

const ITEM: Color = Color::srgb(0.17, 0.18, 0.22);
const ITEM_HOVER: Color = Color::srgb(0.24, 0.26, 0.31);
const ITEM_ON: Color = Color::srgb(0.24, 0.36, 0.62);
const TEXT: Color = Color::srgb(0.92, 0.93, 0.95);
const DIM: Color = Color::srgb(0.6, 0.63, 0.7);
const GOLD: Color = Color::srgb(0.98, 0.78, 0.25);
const SWITCH_OFF: Color = Color::srgb(0.3, 0.32, 0.38);
const SWITCH_ON: Color = Color::srgb(0.2, 0.55, 0.32);
const LABEL_W: f32 = 210.0;

#[derive(Component)]
struct EditLook {
    normal: Color,
    hover: Color,
}

/// Zone de l'éditeur qui défile à la molette (la fenêtre Options ne défile pas d'elle-même).
#[derive(Component)]
pub struct EditScroll;

pub struct GameOptionsPlugin;

impl Plugin for GameOptionsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (catch_key, looks, wheel));
    }
}

fn label(font: &Option<Handle<Font>>, value: impl Into<String>, size: f32, color: Color) -> (Text, TextFont, TextColor) {
    let mut text_font = TextFont::from_font_size(size);
    if let Some(font) = font {
        text_font.font = font.clone().into();
    }
    (Text::new(value), text_font, TextColor(color))
}

fn small_button<'a>(c: &'a mut ChildSpawnerCommands, on: bool) -> EntityCommands<'a> {
    let (normal, hover) = if on { (ITEM_ON, ITEM_ON) } else { (ITEM, ITEM_HOVER) };
    c.spawn((
        Button,
        Hovered::default(),
        Node { padding: UiRect::axes(px(12), px(6)), border_radius: BorderRadius::all(px(6)), justify_content: JustifyContent::Center, align_items: AlignItems::Center, flex_shrink: 0.0, ..default() },
        BackgroundColor(normal),
        EditLook { normal, hover },
    ))
}

/// Un interrupteur : une pastille qui glisse à droite (vert) quand c'est actif.
pub fn spawn_switch<'a>(c: &'a mut ChildSpawnerCommands, on: bool) -> EntityCommands<'a> {
    let color = if on { SWITCH_ON } else { SWITCH_OFF };
    let mut switch = c.spawn((
        Button,
        Hovered::default(),
        Node { width: px(46), height: px(26), flex_shrink: 0.0, padding: UiRect::all(px(3)), border_radius: BorderRadius::all(px(13)), justify_content: if on { JustifyContent::FlexEnd } else { JustifyContent::FlexStart }, align_items: AlignItems::Center, ..default() },
        BackgroundColor(color),
        EditLook { normal: color, hover: color.lighter(0.06) },
    ));
    switch.with_child((Node { width: px(20), height: px(20), border_radius: BorderRadius::all(px(10)), ..default() }, BackgroundColor(TEXT), Pickable::IGNORE));
    switch
}

fn key_text(names: &[String]) -> String {
    let labels: Vec<String> = names.iter().map(|n| crate::native_game::key_by_name(n).map_or_else(|| n.clone(), crate::native_game::key_code_label)).collect();
    labels.join(tr(" or ", " ou "))
}

/// Dit d'où vient une valeur : du jeu, des réglages généraux ou du défaut.
fn origin(font: &Option<Handle<Font>>, row: &mut ChildSpawnerCommands, scope: &Option<String>, here: bool, global_set: bool) {
    let (value, color) = match (scope.is_some(), here, global_set) {
        (true, true, _) => (tr("this game", "ce jeu"), GOLD),
        (true, false, true) => (tr("general", "general"), DIM),
        (false, true, _) => (tr("changed", "change"), GOLD),
        _ => (tr("default", "par defaut"), DIM),
    };
    row.spawn((label(font, value, 13.0, color), Node { width: px(80), flex_shrink: 0.0, ..default() }));
}

/// Retire la valeur de ce niveau (le jeu reprend la générale, le général reprend le défaut).
fn reset_button(font: &Option<Handle<Font>>, row: &mut ChildSpawnerCommands, scope: &Option<String>, edit: impl Fn(&mut Layer) + Send + Sync + 'static) {
    let scope = scope.clone();
    let text = if scope.is_some() { tr("Use general", "Comme le general") } else { tr("Default", "Par defaut") };
    small_button(row, false)
        .observe(move |_: On<Activate>| {
            let scope = scope.clone();
            change(|options| edit(options.layer_mut(scope.as_deref())));
        })
        .with_child(label(font, text, 13.0, DIM));
}

fn row_node() -> Node {
    Node { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, align_items: AlignItems::Center, column_gap: px(8), row_gap: px(4), flex_shrink: 0.0, ..default() }
}

fn heading(font: &Option<Handle<Font>>, c: &mut ChildSpawnerCommands, value: &str) {
    c.spawn((label(font, value, 17.0, TEXT), Node { margin: UiRect::top(px(10)), flex_shrink: 0.0, ..default() }));
}

/// L'éditeur : `scope` = `None` pour les réglages généraux, le code du jeu sinon.
pub fn spawn_editor(c: &mut ChildSpawnerCommands, scope: Option<String>, font: Option<Handle<Font>>) {
    let options = get();
    let code = scope.as_deref();
    let here = code.map_or(&options.global, |c| options.games.get(c).unwrap_or(&EMPTY));
    let waiting = WAITING.lock().unwrap().clone().filter(|(s, _)| *s == scope).map(|(_, t)| t);
    let font = &font;

    if scope.is_some() {
        c.spawn((label(font, tr("What you choose here is for this game only and goes before the general settings (Options > Games).", "Ce que vous choisissez ici ne vaut que pour ce jeu et passe avant les reglages generaux (Options > Jeux)."), 14.0, DIM), Node { flex_shrink: 0.0, ..default() }));
    } else {
        c.spawn((label(font, tr("For every game. A game can have its own settings in its Settings tab: they go first.", "Pour tous les jeux. Un jeu peut avoir les siens dans son onglet Parametres : ils passent avant."), 14.0, DIM), Node { flex_shrink: 0.0, ..default() }));
    }

    // Vitesse
    heading(font, c, tr("Game speed", "Vitesse du jeu"));
    c.spawn(row_node()).with_children(|row| {
        let on = options.speed_enabled(code);
        let scope_switch = scope.clone();
        spawn_switch(row, on).observe(move |_: On<Activate>| {
            let scope = scope_switch.clone();
            change(|o| o.layer_mut(scope.as_deref()).speed = Some(!on));
        });
        row.spawn((label(font, tr("Speed-up keys (only when playing with mods)", "Touches de vitesse (seulement en jouant avec des mods)"), 15.0, TEXT), Node { width: px(LABEL_W + 120.0), ..default() }));
        origin(font, row, &scope, here.speed.is_some(), options.global.speed.is_some());
        if here.speed.is_some() {
            reset_button(font, row, &scope, |l| l.speed = None);
        }
    });
    c.spawn(row_node()).with_children(|row| {
        row.spawn((label(font, tr("Fast speed", "Vitesse rapide"), 15.0, TEXT), Node { width: px(LABEL_W), ..default() }));
        let now = options.fast_speed(code);
        for speed in SPEEDS.into_iter().filter(|s| *s > 100) {
            let scope = scope.clone();
            small_button(row, speed == now)
                .observe(move |_: On<Activate>| {
                    let scope = scope.clone();
                    change(|o| o.layer_mut(scope.as_deref()).fast_speed = Some(speed));
                })
                .with_child(label(font, format!("x{}", speed as f32 / 100.0), 14.0, TEXT));
        }
        origin(font, row, &scope, here.fast_speed.is_some(), options.global.fast_speed.is_some());
        if here.fast_speed.is_some() {
            reset_button(font, row, &scope, |l| l.fast_speed = None);
        }
    });
    for which in SpeedKey::ALL {
        let names = vec![crate::native_game::key_name(options.speed_key(code, which)).to_string()];
        let set_here = here.speed_keys.contains_key(which.id());
        let set_global = options.global.speed_keys.contains_key(which.id());
        key_row(c, font, &scope, which.label(), &names, Target::Speed(which), waiting.as_ref(), set_here, set_global, None);
    }

    // Son
    heading(font, c, tr("Sound", "Son"));
    c.spawn(row_node()).with_children(|row| {
        row.spawn((label(font, tr("Volume", "Volume"), 15.0, TEXT), Node { width: px(LABEL_W), ..default() }));
        let now = options.volume(code);
        for (text, value) in [("-", now.saturating_sub(VOLUME_STEP)), ("+", (now + VOLUME_STEP).min(MAX_VOLUME))] {
            let scope = scope.clone();
            small_button(row, false)
                .observe(move |_: On<Activate>| {
                    let scope = scope.clone();
                    change(|o| o.layer_mut(scope.as_deref()).volume = Some(value));
                })
                .with_child(label(font, text, 15.0, TEXT));
            if text == "-" {
                row.spawn((label(font, if now == 0 { tr("muted", "coupe").to_string() } else { format!("{now} %") }, 15.0, TEXT), Node { width: px(64), justify_content: JustifyContent::Center, ..default() }));
            }
        }
        let scope_mute = scope.clone();
        small_button(row, now == 0)
            .observe(move |_: On<Activate>| {
                let scope = scope_mute.clone();
                change(|o| o.layer_mut(scope.as_deref()).volume = Some(if now == 0 { 100 } else { 0 }));
            })
            .with_child(label(font, tr("Mute", "Couper"), 14.0, TEXT));
        origin(font, row, &scope, here.volume.is_some(), options.global.volume.is_some());
        if here.volume.is_some() {
            reset_button(font, row, &scope, |l| l.volume = None);
        }
    });

    // Langue
    heading(font, c, tr("Language", "Langue"));
    c.spawn((label(font, tr("The console language the game sees. A game only offers the languages it has.", "La langue de la console que voit le jeu. Un jeu ne propose que les langues qu'il a."), 13.0, DIM), Node { flex_shrink: 0.0, ..default() }));
    c.spawn(row_node()).with_children(|row| {
        let now = options.language(code);
        for (id, _, en, fr) in LANGUAGES {
            let scope = scope.clone();
            small_button(row, now == id)
                .observe(move |_: On<Activate>| {
                    let scope = scope.clone();
                    change(|o| o.layer_mut(scope.as_deref()).language = Some(id.to_string()));
                })
                .with_child(label(font, tr(en, fr), 14.0, TEXT));
        }
        origin(font, row, &scope, here.language.is_some(), options.global.language.is_some());
        if here.language.is_some() {
            reset_button(font, row, &scope, |l| l.language = None);
        }
    });

    // Touches
    heading(font, c, tr("Keys", "Touches"));
    c.spawn((label(font, tr("Click a key, then press the new one (Esc cancels). Keys by position: the same on AZERTY and QWERTY.", "Cliquez une touche, puis appuyez sur la nouvelle (Echap annule). Touches par position : pareil en AZERTY et en QWERTY."), 13.0, DIM), Node { flex_shrink: 0.0, ..default() }));
    let map = options.keymap(code);
    for button in DsButton::ALL {
        let names: Vec<String> = map.0.get(&button).map(|k| k.iter().map(|k| crate::native_game::key_name(*k).to_string()).collect()).unwrap_or_default();
        let id = button_id(button);
        let pad: Vec<String> = pad_buttons_of(button).iter().map(|b| format!("{b:?}")).collect();
        key_row(c, font, &scope, &format!("{button:?}"), &names, Target::Button(button), waiting.as_ref(), here.keys.contains_key(&id), options.global.keys.contains_key(&id), Some(pad.join(tr(" or ", " ou "))));
    }

    if scope.is_some() && !here.is_empty() {
        let scope = scope.clone();
        small_button(c, false)
            .insert(Node { align_self: AlignSelf::FlexStart, margin: UiRect::top(px(12)), padding: UiRect::axes(px(12), px(6)), border_radius: BorderRadius::all(px(6)), ..default() })
            .observe(move |_: On<Activate>| {
                let scope = scope.clone();
                change(|o| {
                    if let Some(code) = scope.as_deref() {
                        o.games.remove(code);
                    }
                });
            })
            .with_child(label(font, tr("Use the general settings for everything", "Tout remettre comme les reglages generaux"), 14.0, TEXT));
    }
}

static EMPTY: Layer = Layer { keys: BTreeMap::new(), speed_keys: BTreeMap::new(), speed: None, fast_speed: None, volume: None, language: None };

#[allow(clippy::too_many_arguments)]
fn key_row(c: &mut ChildSpawnerCommands, font: &Option<Handle<Font>>, scope: &Option<String>, name: &str, names: &[String], target: Target, waiting: Option<&Target>, set_here: bool, set_global: bool, pad: Option<String>) {
    c.spawn(row_node()).with_children(|row| {
        row.spawn((label(font, name.to_string(), 15.0, TEXT), Node { width: px(LABEL_W), ..default() }));
        let is_waiting = waiting == Some(&target);
        let shown = if is_waiting { tr("Press a key... (Esc cancels)", "Appuyez sur une touche... (Echap annule)").to_string() } else { key_text(names) };
        let (scope_click, target_click) = (scope.clone(), target.clone());
        small_button(row, is_waiting)
            .insert(Node { min_width: px(150), padding: UiRect::axes(px(12), px(6)), border_radius: BorderRadius::all(px(6)), justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() })
            .observe(move |_: On<Activate>| {
                *WAITING.lock().unwrap() = Some((scope_click.clone(), target_click.clone()));
                REVISION.fetch_add(1, Ordering::Relaxed);
            })
            .with_child(label(font, shown, 14.0, TEXT));
        origin(font, row, scope, set_here, set_global);
        if set_here {
            let target = target.clone();
            reset_button(font, row, scope, move |l| match &target {
                Target::Button(b) => {
                    l.keys.remove(&button_id(*b));
                }
                Target::Speed(s) => {
                    l.speed_keys.remove(s.id());
                }
            });
        }
        if let Some(pad) = pad {
            row.spawn(label(font, t!("controller: {pad}", "manette : {pad}"), 13.0, DIM));
        }
    });
}

/// La touche attendue : la prochaine enfoncée la prend, Échap annule.
fn catch_key(keys: Res<ButtonInput<KeyCode>>) {
    CAUGHT.store(false, Ordering::Relaxed);
    let Some((scope, target)) = WAITING.lock().unwrap().clone() else { return };
    if keys.just_pressed(KeyCode::Escape) {
        CAUGHT.store(true, Ordering::Relaxed);
        *WAITING.lock().unwrap() = None;
        REVISION.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let Some(key) = keys.get_just_pressed().find(|k| crate::native_game::key_known(**k)).copied() else { return };
    CAUGHT.store(true, Ordering::Relaxed);
    *WAITING.lock().unwrap() = None;
    let name = crate::native_game::key_name(key).to_string();
    change(|o| {
        let layer = o.layer_mut(scope.as_deref());
        match target {
            Target::Button(button) => {
                layer.keys.insert(button_id(button), vec![name]);
            }
            Target::Speed(which) => {
                layer.speed_keys.insert(which.id().to_string(), name);
            }
        }
    });
}

fn looks(mut buttons: Query<(&EditLook, &Hovered, Has<Pressed>, &mut BackgroundColor)>) {
    for (look, hovered, pressed, mut color) in &mut buttons {
        let wanted = if pressed || hovered.get() { look.hover } else { look.normal };
        if color.0 != wanted {
            color.0 = wanted;
        }
    }
}

fn wheel(mut events: MessageReader<MouseWheel>, mut areas: Query<(&Hovered, &ComputedNode, &mut ScrollPosition), With<EditScroll>>) {
    for event in events.read() {
        let lines = match event.unit {
            MouseScrollUnit::Line => event.y * 60.0,
            MouseScrollUnit::Pixel => event.y,
        };
        for (hovered, node, mut position) in &mut areas {
            if hovered.get() {
                let max = (node.content_size().y - node.size().y).max(0.0) * node.inverse_scale_factor();
                position.0.y = (position.0.y - lines).clamp(0.0, max);
                *OPTIONS_SCROLL.lock().unwrap() = position.0.y;
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
//  En jeu : la vitesse
// ─────────────────────────────────────────────────────────────────────────

/// La vitesse suivante dans la liste, vers le haut ou le bas.
pub fn step_speed(now: u32, faster: bool) -> u32 {
    if faster {
        SPEEDS.into_iter().find(|s| *s > now).unwrap_or(SPEEDS[SPEEDS.len() - 1])
    } else {
        SPEEDS.into_iter().rev().find(|s| *s < now).unwrap_or(SPEEDS[0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_game_goes_before_the_general_settings() {
        let mut options = OptionsFile::default();
        options.global.volume = Some(50);
        options.global.keys.insert("a".into(), vec!["KeyK".into()]);
        options.global.keys.insert("b".into(), vec!["KeyJ".into()]);
        let game = options.games.entry("ADGE".into()).or_default();
        game.volume = Some(80);
        game.keys.insert("a".into(), vec!["Space".into()]);
        game.language = Some("fr".into());
        assert_eq!(options.volume(Some("ADGE")), 80);
        assert_eq!(options.volume(Some("AJTP")), 50);
        assert_eq!(options.volume(None), 50);
        let map = options.keymap(Some("ADGE"));
        assert_eq!(map.0[&DsButton::A], vec![KeyCode::Space]);
        assert_eq!(map.0[&DsButton::B], vec![KeyCode::KeyJ]);
        assert_eq!(map.0[&DsButton::X], vec![KeyCode::KeyS]);
        assert_eq!(options.keymap(Some("AJTP")).0[&DsButton::A], vec![KeyCode::KeyK]);
        assert_eq!(options.language(Some("ADGE")), "fr");
        assert_eq!(options.language(None), "game");
        assert!(options.speed_enabled(None));
        assert_eq!(options.speed_key(None, SpeedKey::Toggle), KeyCode::Tab);
    }

    #[test]
    fn round_trip_and_empty_games_are_dropped() {
        let mut options = OptionsFile::default();
        options.global.fast_speed = Some(300);
        options.games.insert("ADGE".into(), Layer { speed: Some(false), ..Layer::default() });
        options.games.insert("AJTP".into(), Layer::default());
        options.tidy();
        let parsed = OptionsFile::parse(&toml::to_string_pretty(&options).unwrap());
        assert_eq!(parsed, options);
        assert!(!parsed.games.contains_key("AJTP"));
        assert!(!parsed.speed_enabled(Some("ADGE")));
        assert_eq!(parsed.fast_speed(None), 300);
        assert_eq!(OptionsFile::parse("n'importe = = quoi"), OptionsFile::default());
    }

    #[test]
    fn speed_steps() {
        assert_eq!(step_speed(100, true), 150);
        assert_eq!(step_speed(100, false), 75);
        assert_eq!(step_speed(800, true), 800);
        assert_eq!(step_speed(25, false), 25);
        assert_eq!(step_speed(120, false), 100);
    }
}
