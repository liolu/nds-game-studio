//! Fenêtre Options (F1, ou le bouton à gauche de la barre de titre).
//!
//! - **Launcher** : vue des jeux, dossier des roms, versions (`launcher`) ;
//! - **Affichage** : format, écrans fusionnés ou séparés, plein écran, rendu HD ou pixel, et
//!   « Reinitialiser les options » (toutes les options gardées, voir `settings`) ;
//! - **Manettes** : manettes branchées et leur modèle reconnu, menu déroulant de tous les modèles,
//!   dessin de la manette choisie ; la touche pressée sur la vraie manette s'allume et son nom
//!   s'affiche (un clic sur une touche dessinée l'affiche aussi) ;
//! - **Clavier** : touches du clavier et de la manette pour chaque bouton DS ;
//! - **Sauvegardes** : les sauvegardes des jeux (`saves_page`) ;
//! - **Développement** : recompiler et relancer à l'enregistrement, fenêtre Code, fenêtre Assets ; avec l'accès
//!   développeur, l'app de validation des fichiers des jeux (`asset_browser::review`).

use bevy::camera::RenderTarget;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::Pressed;
use bevy::ui_widgets::{Activate, Button};
use bevy::window::{WindowMode, WindowRef, WindowResolution};

use crate::controllers::{self, first_gamepad, ControllerDef, PadButtonView};
use crate::devtools::{DevSettings, ToggleCodeWindow};
use crate::i18n::{lang, t, tr, Lang, Language, Tr};
use crate::input::{pad_buttons_of, DsButton, KeyMap};
use crate::screens::{Aspect, DsScreens, RenderStyle, ScreenLayout};
use crate::settings::ResetSettings;

/// La fenêtre des options (la fermer ne quitte pas le studio).
#[derive(Component)]
pub struct OptionsWindow;

/// Ouvrir ou fermer la fenêtre des options (comme F1).
#[derive(Message)]
pub struct ToggleOptions;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Page {
    #[default]
    Launcher,
    Display,
    Controllers,
    Keyboard,
    Saves,
    Dev,
}

impl Page {
    const ALL: [Page; 6] = [Page::Launcher, Page::Display, Page::Controllers, Page::Keyboard, Page::Saves, Page::Dev];

    /// Nom en anglais et en français.
    fn names(self) -> (&'static str, &'static str) {
        match self {
            Page::Launcher => ("Launcher", "Launcher"),
            Page::Display => ("Display", "Affichage"),
            Page::Controllers => ("Controllers", "Manettes"),
            Page::Keyboard => ("Keyboard", "Clavier"),
            Page::Saves => ("Saves", "Sauvegardes"),
            Page::Dev => ("Development", "Developpement"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Setting {
    Aspect,
    Screens,
    Fullscreen,
    Render,
    AutoReload,
    DeveloperAccess,
}

#[derive(Resource, Default)]
pub(crate) struct OptionsView {
    window: Option<Entity>,
    parts: Vec<Entity>,
    content: Option<Entity>,
    page: Page,
    /// Modèle de manette montré (id) ; choisi par le joueur ou reconnu sur la manette branchée.
    pub(crate) model: Option<String>,
    /// Choisi à la main dans la liste (gardé dans options.toml, voir `settings`).
    pub(crate) model_chosen: bool,
    dropdown_open: bool,
    /// Ce qui est construit dans le panneau (pour savoir quand le refaire).
    built: Option<(Page, Option<String>, bool, Lang, bool)>,
    /// Dernière touche pressée ou cliquée.
    press: String,
    /// L'app de validation lancée (une seule à la fois).
    review: Option<std::process::Child>,
}

#[derive(Component)]
struct PageButton(Page);

#[derive(Component)]
struct ChoiceButton {
    setting: Setting,
    value: usize,
}

#[derive(Component)]
struct ModelItem(String);

#[derive(Component)]
struct DropdownButton;

#[derive(Component)]
struct CodeWindowButton;

#[derive(Component)]
struct ResetButton;

#[derive(Component)]
struct AssetsWindowButton;

#[derive(Component)]
struct ReviewButton;

#[derive(Component)]
struct ConnectedText;

#[derive(Component)]
struct PressText;

const BG: Color = Color::srgb(0.1, 0.11, 0.13);
const SIDEBAR: Color = Color::srgb(0.075, 0.08, 0.095);
const ITEM: Color = Color::srgb(0.16, 0.17, 0.2);
const ITEM_HOVER: Color = Color::srgb(0.22, 0.24, 0.28);
const ITEM_ON: Color = Color::srgb(0.24, 0.36, 0.62);
const TEXT: Color = Color::srgb(0.9, 0.91, 0.94);
const DIM: Color = Color::srgb(0.6, 0.63, 0.7);

pub struct OptionsPlugin;

impl Plugin for OptionsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<OptionsView>().add_message::<ToggleOptions>().add_systems(
            Update,
            (toggle_window, auto_model, build_page, refresh_choices, refresh_connected, read_presses, refresh_press, item_looks, controllers::animate_drawing).chain(),
        );
    }
}

fn label(text: impl Into<String>, size: f32, color: Color) -> impl Bundle {
    (Text::new(text), TextFont::from_font_size(size), TextColor(color))
}

fn item_node() -> Node {
    Node { padding: UiRect::axes(px(14), px(7)), border_radius: BorderRadius::all(px(6)), justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() }
}

fn toggle_window(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    mut requests: MessageReader<ToggleOptions>,
    mut view: ResMut<OptionsView>,
    windows: Query<(), With<Window>>,
) {
    if view.window.is_some_and(|w| windows.get(w).is_err()) {
        view.window = None;
        for part in std::mem::take(&mut view.parts) {
            commands.entity(part).despawn();
        }
    }
    let requested = requests.read().count() % 2 == 1;
    if !keys.just_pressed(KeyCode::F1) && !requested {
        return;
    }
    if let Some(window) = view.window.take() {
        commands.entity(window).despawn();
        for part in std::mem::take(&mut view.parts) {
            commands.entity(part).despawn();
        }
        return;
    }
    let window = commands.spawn((Window { title: "Nds Game Studio - Options (F1)".into(), resolution: WindowResolution::new(1000, 720), ..default() }, OptionsWindow)).id();
    let camera = commands
        .spawn((Camera2d, Camera { order: 6, clear_color: ClearColorConfig::Custom(BG), ..default() }, RenderTarget::Window(WindowRef::Entity(window)), bevy::camera::visibility::RenderLayers::none()))
        .id();
    let mut content = None;
    let root = commands
        .spawn((Node { width: percent(100), height: percent(100), flex_direction: FlexDirection::Row, ..default() }, UiTargetCamera(camera)))
        .with_children(|root| {
            root.spawn((Node { width: px(210), height: percent(100), flex_direction: FlexDirection::Column, padding: UiRect::all(px(12)), row_gap: px(6), ..default() }, BackgroundColor(SIDEBAR)))
                .with_children(|side| {
                    side.spawn((label(tr("Settings", "Options"), 22.0, TEXT), Tr("Settings", "Options"), Node { margin: UiRect::bottom(px(12)), ..default() }));
                    for page in Page::ALL {
                        side.spawn((Button, Hovered::default(), item_node(), BackgroundColor(ITEM), PageButton(page))).observe(select_page).with_child({
                            let (en, fr) = page.names();
                            (label(tr(en, fr), 16.0, TEXT), Tr(en, fr))
                        });
                    }
                });
            content = Some(
                root.spawn(Node { flex_grow: 1.0, height: percent(100), flex_direction: FlexDirection::Column, padding: UiRect::all(px(20)), row_gap: px(14), overflow: Overflow::clip(), ..default() })
                    .id(),
            );
        })
        .id();
    view.window = Some(window);
    view.parts = vec![camera, root];
    view.content = content;
    view.built = None;
}

fn select_page(activate: On<Activate>, pages: Query<&PageButton>, mut view: ResMut<OptionsView>) {
    if let Ok(page) = pages.get(activate.entity) {
        view.page = page.0;
        view.dropdown_open = false;
    }
}

/// Modèle montré : celui de la manette branchée, tant que le joueur n'en a pas choisi un.
fn auto_model(mut view: ResMut<OptionsView>, pads: Query<(Entity, &Gamepad, Option<&Name>)>) {
    if view.model_chosen && view.model.is_some() {
        return;
    }
    let detected = first_gamepad(&pads).and_then(|e| pads.get(e).ok()).map(|(_, g, n)| controllers::detect(n.map_or("", |n| n.as_str()), g.vendor_id(), g.product_id()).id.clone());
    let wanted = detected.or_else(|| Some("generic".into()));
    if view.model != wanted {
        view.model = wanted;
    }
}

fn current_model(view: &OptionsView) -> &'static ControllerDef {
    view.model.as_deref().and_then(controllers::by_id).unwrap_or(&controllers::controllers()[0])
}

fn build_page(mut commands: Commands, mut view: ResMut<OptionsView>, keymap: Res<KeyMap>, language: Res<Language>, dev: Res<DevSettings>, children: Query<&Children>) {
    let (Some(_), Some(content)) = (view.window, view.content) else { return };
    let key = (view.page, view.model.clone(), view.dropdown_open, language.0, dev.developer_access);
    if view.built.as_ref() == Some(&key) {
        return;
    }
    view.built = Some(key);
    if let Ok(old) = children.get(content) {
        for child in old.iter() {
            commands.entity(child).despawn();
        }
    }
    let model = current_model(&view);
    let dropdown_open = view.dropdown_open;
    let page = view.page;
    commands.entity(content).with_children(|c| match page {
        Page::Launcher => crate::launcher::spawn_options_page(c),
        Page::Display => {
            c.spawn(label(tr("Display", "Affichage"), 24.0, TEXT));
            choice_row(c, tr("Picture format (F8)", "Format de l'image (F8)"), Setting::Aspect, &[tr("Fill", "Remplir"), "4:3 (DS)", "16:9", "16:10"]);
            choice_row(c, tr("Screens (F9, or the title bar button)", "Ecrans (F9, ou le bouton de la barre de titre)"), Setting::Screens, &[tr("Merged", "Fusionnes"), tr("Separate", "Separes")]);
            choice_row(c, tr("Fullscreen", "Plein ecran"), Setting::Fullscreen, &[tr("No", "Non"), tr("Yes", "Oui")]);
            choice_row(c, tr("Rendering", "Rendu"), Setting::Render, &[tr("HD (maximum quality)", "HD (qualite maximale)"), tr("Pixel (like the console)", "Pixel (comme la console)")]);
            c.spawn(label(tr("The next game can force its format or rendering in its game.toml.", "Le jeu suivant peut imposer son format ou son rendu dans son game.toml."), 13.0, DIM));
            // Toutes les options (aussi manettes, developpement, place de la fenetre) : options.toml efface
            c.spawn((Button, Hovered::default(), Node { align_self: AlignSelf::FlexStart, margin: UiRect::top(px(10)), ..item_node() }, BackgroundColor(ITEM), ResetButton))
                .observe(|_: On<Activate>, mut reset: MessageWriter<ResetSettings>| {
                    reset.write(ResetSettings);
                })
                .with_child(label(tr("Reset settings", "Reinitialiser les options"), 15.0, TEXT));
        }
        Page::Controllers => {
            c.spawn(label(tr("Controllers", "Manettes"), 24.0, TEXT));
            c.spawn((label("", 14.0, DIM), ConnectedText));
            // Menu déroulant des modèles (la liste passe par-dessus le dessin)
            c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(10), ..default() }).with_children(|row| {
                row.spawn(label(tr("Model:", "Modele :"), 15.0, TEXT));
                row.spawn(Node { position_type: PositionType::Relative, ..default() }).with_children(|drop| {
                    drop.spawn((Button, Hovered::default(), Node { min_width: px(380), ..item_node() }, BackgroundColor(ITEM), DropdownButton))
                        .observe(toggle_dropdown)
                        .with_child(label(format!("{}   {}", model.display_name(), if dropdown_open { "^" } else { "v" }), 15.0, TEXT));
                    if dropdown_open {
                        drop.spawn((
                            Node { position_type: PositionType::Absolute, top: px(38), left: px(0), min_width: px(380), flex_direction: FlexDirection::Column, padding: UiRect::all(px(4)), row_gap: px(2), border_radius: BorderRadius::all(px(6)), ..default() },
                            BackgroundColor(Color::srgb(0.13, 0.14, 0.17)),
                            GlobalZIndex(10),
                        ))
                        .with_children(|list| {
                            for def in controllers::controllers() {
                                list.spawn((Button, Hovered::default(), Node { justify_content: JustifyContent::FlexStart, ..item_node() }, BackgroundColor(ITEM), ModelItem(def.id.clone())))
                                    .observe(pick_model)
                                    .with_child(label(def.display_name().to_string(), 15.0, TEXT));
                            }
                        });
                    }
                });
            });
            controllers::spawn_drawing(c, model, 1.3);
            c.spawn((label("", 17.0, TEXT), PressText));
        }
        Page::Keyboard => {
            c.spawn(label(tr("Keyboard and controller", "Clavier et manette"), 24.0, TEXT));
            c.spawn(label(tr("Keyboard keys by position (same on AZERTY and QWERTY). A game can change them in its game.toml.", "Touches du clavier par position (pareil en AZERTY et en QWERTY). Un jeu peut les changer dans son game.toml."), 13.0, DIM));
            for button in DsButton::ALL {
                let keys: Vec<String> = keymap.0.get(&button).map(|k| k.iter().map(|k| format!("{k:?}").replace("Key", "").replace("Arrow", tr("arrow ", "fleche "))).collect()).unwrap_or_default();
                let pad: Vec<String> = pad_buttons_of(button).iter().map(|b| format!("{b:?}")).collect();
                c.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(16), ..default() }).with_children(|row| {
                    row.spawn((label(format!("{button:?}"), 16.0, TEXT), Node { width: px(90), ..default() }));
                    row.spawn((label(t!("keyboard: {}", "clavier : {}", keys.join(tr(" or ", " ou "))), 15.0, TEXT), Node { width: px(300), ..default() }));
                    row.spawn(label(t!("controller: {}", "manette : {}", pad.join(tr(" or ", " ou "))), 15.0, DIM));
                });
            }
        }
        Page::Saves => crate::saves_page::spawn_page(c),
        Page::Dev => {
            c.spawn(label(tr("Development", "Developpement"), 24.0, TEXT));
            choice_row(c, tr("Rebuild and restart when a file is saved", "Recompiler et relancer quand un fichier est enregistre"), Setting::AutoReload, &[tr("Yes", "Oui"), tr("No", "Non")]);
            c.spawn((Button, Hovered::default(), Node { align_self: AlignSelf::FlexStart, ..item_node() }, BackgroundColor(ITEM), CodeWindowButton))
                .observe(|_: On<Activate>, mut toggle: MessageWriter<ToggleCodeWindow>| {
                    toggle.write(ToggleCodeWindow);
                })
                .with_child(label(tr("Open / close the Code window (F5)", "Ouvrir / fermer la fenetre Code (F5)"), 15.0, TEXT));
            c.spawn(label(tr("Works with the repository on this PC (local build).", "Marche avec le depot sur ce PC (compilation locale)."), 13.0, DIM));
            c.spawn((Button, Hovered::default(), Node { align_self: AlignSelf::FlexStart, margin: UiRect::top(px(10)), ..item_node() }, BackgroundColor(ITEM), AssetsWindowButton))
                .observe(|_: On<Activate>, mut toggle: MessageWriter<crate::asset_browser::ToggleAssets>| {
                    toggle.write(crate::asset_browser::ToggleAssets);
                })
                .with_child(label(tr("Open / close the Assets window (F6)", "Ouvrir / fermer la fenetre Assets (F6)"), 15.0, TEXT));
            c.spawn(label(tr("3D models and images of decompiled games (standard Nintendo formats), with their path in NitroFS.", "Modeles 3D et images des jeux decompiles (formats Nintendo standard), avec leur chemin dans NitroFS."), 13.0, DIM));
            c.spawn(Node { margin: UiRect::top(px(10)), ..default() }).with_children(|row| {
                choice_row(row, tr("Developer access", "Acces developpeur"), Setting::DeveloperAccess, &[tr("No", "Non"), tr("Yes", "Oui")]);
            });
            if dev.developer_access {
                c.spawn((Button, Hovered::default(), Node { align_self: AlignSelf::FlexStart, ..item_node() }, BackgroundColor(ITEM), ReviewButton))
                    .observe(start_review)
                    .with_child(label(tr("Validate the game files", "Valider les fichiers des jeux"), 15.0, TEXT));
                let report = crate::asset_browser::review::default_out_dir();
                c.spawn(label(t!("A few files of each format, game after game: buttons say what is wrong. Report: {}", "Quelques fichiers de chaque format, jeu apres jeu : des boutons disent ce qui ne va pas. Rapport : {}", report.display()), 13.0, DIM));
            }
        }
    });
}

fn choice_row(c: &mut ChildSpawnerCommands, title: &str, setting: Setting, values: &[&str]) {
    c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: px(6), ..default() }).with_children(|col| {
        col.spawn(label(title, 15.0, DIM));
        col.spawn(Node { flex_direction: FlexDirection::Row, column_gap: px(6), flex_wrap: FlexWrap::Wrap, ..default() }).with_children(|row| {
            for (value, text) in values.iter().enumerate() {
                row.spawn((Button, Hovered::default(), item_node(), BackgroundColor(ITEM), ChoiceButton { setting, value })).observe(apply_choice).with_child(label(*text, 15.0, TEXT));
            }
        });
    });
}

/// Lance l'app de validation dans son propre processus : un fichier qui la fait tomber laisse le studio ouvert.
fn start_review(_: On<Activate>, mut view: ResMut<OptionsView>) {
    if view.review.as_mut().is_some_and(|child| matches!(child.try_wait(), Ok(None))) {
        return;
    }
    let language = format!("--lang={}", lang().code());
    match std::env::current_exe().and_then(|exe| std::process::Command::new(exe).args([crate::asset_browser::review::REVIEW_ARG, language.as_str()]).spawn()) {
        Ok(child) => view.review = Some(child),
        Err(e) => warn!("validation des fichiers non lancee : {e}"),
    }
}

const ASPECTS: [Aspect; 4] = [Aspect::Fill, Aspect::Ds, Aspect::Wide16x9, Aspect::Wide16x10];

fn apply_choice(
    activate: On<Activate>,
    choices: Query<&ChoiceButton>,
    mut layout: ResMut<ScreenLayout>,
    mut style: ResMut<RenderStyle>,
    mut dev: ResMut<DevSettings>,
    screens: Res<DsScreens>,
    mut windows: Query<&mut Window>,
) {
    let Ok(choice) = choices.get(activate.entity) else { return };
    match choice.setting {
        Setting::Aspect => layout.aspect = ASPECTS[choice.value],
        Setting::Screens => layout.merged = choice.value == 0,
        Setting::Render => *style = if choice.value == 0 { RenderStyle::Hd } else { RenderStyle::Pixel },
        Setting::AutoReload => dev.auto_reload = choice.value == 0,
        Setting::DeveloperAccess => dev.developer_access = choice.value == 1,
        Setting::Fullscreen => {
            if let Ok(mut window) = windows.get_mut(screens.main_window()) {
                window.mode = if choice.value == 1 { WindowMode::BorderlessFullscreen(MonitorSelection::Current) } else { WindowMode::Windowed };
            }
        }
    }
}

/// Le choix actif de chaque ligne est en couleur.
fn refresh_choices(
    view: Res<OptionsView>,
    layout: Res<ScreenLayout>,
    style: Res<RenderStyle>,
    dev: Res<DevSettings>,
    screens: Res<DsScreens>,
    windows: Query<&Window>,
    mut choices: Query<(&ChoiceButton, &Hovered, Has<Pressed>, &mut BackgroundColor)>,
) {
    if view.window.is_none() {
        return;
    }
    let fullscreen = windows.get(screens.main_window()).is_ok_and(|w| w.mode != WindowMode::Windowed);
    for (choice, hovered, pressed, mut color) in &mut choices {
        let current = match choice.setting {
            Setting::Aspect => ASPECTS.iter().position(|a| *a == layout.aspect).unwrap_or(0),
            Setting::Screens => usize::from(!layout.merged),
            Setting::Fullscreen => usize::from(fullscreen),
            Setting::Render => usize::from(*style == RenderStyle::Pixel),
            Setting::AutoReload => usize::from(!dev.auto_reload),
            Setting::DeveloperAccess => usize::from(dev.developer_access),
        };
        let wanted = if current == choice.value || pressed { ITEM_ON } else if hovered.get() { ITEM_HOVER } else { ITEM };
        if color.0 != wanted {
            color.0 = wanted;
        }
    }
}

fn toggle_dropdown(_: On<Activate>, mut view: ResMut<OptionsView>) {
    view.dropdown_open = !view.dropdown_open;
}

fn pick_model(activate: On<Activate>, items: Query<&ModelItem>, mut view: ResMut<OptionsView>) {
    if let Ok(item) = items.get(activate.entity) {
        view.model = Some(item.0.clone());
        view.model_chosen = true;
        view.dropdown_open = false;
        view.press.clear();
    }
}

fn refresh_connected(view: Res<OptionsView>, pads: Query<(Entity, &Gamepad, Option<&Name>)>, mut texts: Query<&mut Text, With<ConnectedText>>) {
    let Ok(mut text) = texts.single_mut() else { return };
    let mut lines: Vec<String> = pads
        .iter()
        .map(|(_, g, n)| {
            let name = n.map_or(tr("Controller", "Manette"), |n| n.as_str());
            t!("Connected: {name}  ->  recognized as {}", "Branchee : {name}  ->  reconnue comme {}", controllers::detect(name, g.vendor_id(), g.product_id()).display_name())
        })
        .collect();
    if lines.is_empty() {
        lines.push(tr("No controller connected. Plug one in: it is recognized automatically.", "Aucune manette branchee. Branchez-en une : elle est reconnue toute seule.").into());
    }
    if view.model_chosen {
        lines.push(tr("Model chosen by hand in the list.", "Modele choisi a la main dans la liste.").into());
    }
    let joined = lines.join("\n");
    if text.0 != joined {
        text.0 = joined;
    }
}

/// Touche pressée sur la vraie manette : son nom et le bouton DS qu'elle commande.
fn read_presses(mut view: ResMut<OptionsView>, pads: Query<(Entity, &Gamepad, Option<&Name>)>) {
    if view.window.is_none() {
        return;
    }
    let Some((_, pad, _)) = first_gamepad(&pads).and_then(|e| pads.get(e).ok()) else { return };
    let model = current_model(&view);
    if let Some(button) = pad.get_just_pressed().next().copied() {
        view.press = describe(model, button, tr("Button pressed", "Touche pressee"));
    }
}

fn describe(model: &ControllerDef, button: GamepadButton, what: &str) -> String {
    let drawn = model.buttons.iter().find(|b| controllers::pad_button(&b.input) == Some(button)).map(|b| b.label.clone()).filter(|l| !l.is_empty());
    let ds = controllers::ds_button_for(button).map_or(tr("none", "aucun").to_string(), |d| format!("{d:?}"));
    match drawn {
        Some(name) => t!("{what}: {name}  ({button:?})  ->  DS button: {ds}", "{what} : {name}  ({button:?})  ->  bouton DS : {ds}"),
        None => t!("{what}: {button:?}  ->  DS button: {ds}", "{what} : {button:?}  ->  bouton DS : {ds}"),
    }
}

fn refresh_press(mut view: ResMut<OptionsView>, clicked: Query<(&PadButtonView, &Pressed), Added<Pressed>>, mut texts: Query<&mut Text, With<PressText>>) {
    // Clic sur une touche dessinée
    if let Some((button, _)) = clicked.iter().next() {
        let model = current_model(&view);
        view.press = describe(model, button.input, tr("Button clicked", "Touche cliquee"));
    }
    let Ok(mut text) = texts.single_mut() else { return };
    let wanted = if view.press.is_empty() { tr("Press a button on the controller (or click a button on the drawing).", "Appuyez sur une touche de la manette (ou cliquez une touche du dessin).").to_string() } else { view.press.clone() };
    if text.0 != wanted {
        text.0 = wanted;
    }
}

fn item_looks(view: Res<OptionsView>, mut items: Query<(&Hovered, Has<Pressed>, &mut BackgroundColor, Option<&PageButton>, Option<&ModelItem>), Or<(With<PageButton>, With<ModelItem>, With<DropdownButton>, With<CodeWindowButton>, With<ResetButton>, With<AssetsWindowButton>, With<ReviewButton>)>>) {
    for (hovered, pressed, mut color, page, model) in &mut items {
        let selected = page.is_some_and(|p| p.0 == view.page) || model.is_some_and(|m| view.model.as_deref() == Some(m.0.as_str()));
        let wanted = if selected || pressed { ITEM_ON } else if hovered.get() { ITEM_HOVER } else { ITEM };
        if color.0 != wanted {
            color.0 = wanted;
        }
    }
}
