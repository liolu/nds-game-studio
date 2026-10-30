// ─────────────────────────────────────────────────────────────────────────
//  Panneau Multijoueur (F2 ou bouton « Multijoueur »)
//
//  La connexion est automatique : rien à faire pour jouer en réseau local.
//  Le panneau sert seulement à :
//   - choisir son pseudo et la couleur du contour de son vaisseau ;
//   - voir qui est connecté ;
//   - donner son code à un ami (Internet) ou taper le code d'un ami.
//
//  Chat écrit : Entrée pour écrire, Entrée pour envoyer, Échap pour annuler.
//  « /g message » : message réservé à sa guilde (joueurs du même tag [TAG]).
//  Les messages s'affichent en bas à gauche et s'effacent après un moment.
// ─────────────────────────────────────────────────────────────────────────

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;

use crate::net::{display_name, sanitize_tag, Invite, Net, NetCommand, NetMode, MAX_CHAT_LEN, MAX_NAME_LEN, MAX_TAG_LEN};
use crate::diplomacy::{faction_key, faction_label, my_declared, relation_with, same_guild, set_personal, Relation};
use crate::guild::{has_rank, Guilds, Role, MAX_GUILD_NAME};
use crate::settings::GameSettings;

pub(crate) const BG_DARK: Color = Color::srgba(0.06, 0.06, 0.10, 0.97);
const BG_FIELD: Color = Color::srgba(0.12, 0.12, 0.18, 1.0);
pub(crate) const BG_BUTTON: Color = Color::srgba(0.14, 0.14, 0.22, 1.0);
pub(crate) const ACCENT: Color = Color::srgb(0.3, 0.6, 1.0);
pub(crate) const TEXT_COLOR: Color = Color::srgb(0.9, 0.9, 0.95);
pub(crate) const TEXT_DIM: Color = Color::srgb(0.55, 0.55, 0.62);
const ERROR_COLOR: Color = Color::srgb(1.0, 0.45, 0.4);
pub(crate) const OK_COLOR: Color = Color::srgb(0.45, 0.9, 0.55);
const GUILD_COLOR: Color = Color::srgb(0.55, 1.0, 0.7);
pub(crate) const CODE_COLOR: Color = Color::srgb(1.0, 0.85, 0.35);
pub(crate) const RED_SOFT: Color = Color::srgb(0.85, 0.35, 0.35);

/// Couleurs de contour proposées.
pub const AURA_PALETTE: [[f32; 3]; 12] = [
    [0.20, 0.90, 1.00], // cyan
    [0.25, 0.45, 1.00], // bleu
    [0.60, 0.35, 1.00], // violet
    [1.00, 0.35, 0.80], // rose
    [1.00, 0.25, 0.25], // rouge
    [1.00, 0.55, 0.15], // orange
    [1.00, 0.90, 0.20], // jaune
    [0.60, 1.00, 0.20], // citron vert
    [0.20, 0.90, 0.40], // vert
    [0.10, 0.75, 0.65], // turquoise
    [0.95, 0.95, 1.00], // blanc
    [0.85, 0.65, 0.35], // or
];

pub struct NetUiPlugin;

impl Plugin for NetUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NetPanel>()
            .add_systems(Startup, (setup_net_panel, setup_chat))
            // Avant Update : le menu Options (Échap) doit voir que le panneau a consommé la touche
            .add_systems(PreUpdate, toggle_net_panel)
            .add_systems(
                Update,
                (
                    handle_panel_buttons,
                    handle_text_input,
                    update_panel_visibility,
                    update_fields,
                    update_swatches,
                    update_texts,
                    handle_relation_buttons,
                    update_guild_summary,
                    rebuild_players_list,
                    update_chat,
                )
                    .chain(),
            );
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Field {
    Name,
    Code,
    /// Nom et tag de la guilde à créer (panneau Guilde).
    GuildName,
    GuildTag,
    /// Saisie d'un message de chat (bloque les touches du jeu, comme les champs du panneau).
    Chat,
    /// Recherche ou note du dex (`dex.rs` lit le clavier lui-même ; ici, seulement le blocage des
    /// touches du jeu).
    Dex,
}

#[derive(Resource, Default)]
pub struct NetPanel {
    pub open: bool,
    /// Panneau Guilde (touche G ou bouton « Guilde »).
    pub guild_open: bool,
    pub focus: Option<Field>,
    /// Échap a été utilisé par ce panneau pendant cette frame.
    pub esc_consumed: bool,
    code: String,
    chat: String,
    /// Nom et tag saisis pour créer une guilde.
    pub guild_name: String,
    pub guild_tag: String,
    /// Messages et commandes envoyés (flèches haut / bas), gardés d'une partie à l'autre.
    history: Vec<String>,
    history_loaded: bool,
    /// Position dans l'historique pendant qu'on le parcourt, et la ligne en cours mise de côté.
    history_pos: Option<usize>,
    draft: String,
    /// Propositions de Tab en cours de parcours, et celle affichée.
    tab: Option<(Vec<String>, usize)>,
}

/// Messages gardés dans l'historique du chat.
const HISTORY_MAX: usize = 100;

fn history_path() -> std::path::PathBuf {
    crate::settings::data_dir().join("chat_history.txt")
}

impl NetPanel {
    fn load_history(&mut self) {
        if self.history_loaded {
            return;
        }
        self.history_loaded = true;
        if let Ok(text) = std::fs::read_to_string(history_path()) {
            self.history = text.lines().filter(|l| !l.trim().is_empty()).map(String::from).collect();
            let extra = self.history.len().saturating_sub(HISTORY_MAX);
            self.history.drain(..extra);
        }
    }

    /// Garde un message envoyé (sans doublon consécutif).
    fn remember(&mut self, msg: &str) {
        let msg = msg.trim();
        if msg.is_empty() || self.history.last().is_some_and(|l| l == msg) {
            return;
        }
        self.history.push(msg.to_string());
        let extra = self.history.len().saturating_sub(HISTORY_MAX);
        self.history.drain(..extra);
        if !cfg!(test) {
            let _ = std::fs::write(history_path(), self.history.join("\n"));
        }
    }

    /// Flèche haut (`up`) ou bas : message précédent / suivant de l'historique.
    fn browse(&mut self, up: bool) {
        self.load_history();
        if self.history.is_empty() {
            return;
        }
        let last = self.history.len() - 1;
        let next = match (self.history_pos, up) {
            (None, true) => {
                self.draft = self.chat.clone();
                Some(last)
            }
            (None, false) => None,
            (Some(p), true) => Some(p.saturating_sub(1)),
            (Some(p), false) if p >= last => None,
            (Some(p), false) => Some(p + 1),
        };
        self.history_pos = next;
        self.chat = match next {
            Some(p) => self.history[p].clone(),
            None => std::mem::take(&mut self.draft),
        };
    }

    /// Tab : complète la commande ; plusieurs propositions → préfixe commun, puis on les fait défiler.
    fn complete(&mut self, galaxy_kinds: &[String], galaxies: usize, backwards: bool) {
        if let Some((list, i)) = &mut self.tab {
            if !list.is_empty() && self.chat == list[*i] {
                *i = if backwards { (*i + list.len() - 1) % list.len() } else { (*i + 1) % list.len() };
                self.chat = list[*i].clone();
                return;
            }
        }
        let list = crate::chat_cmd::suggestions(&self.chat, galaxy_kinds, galaxies);
        match list.len() {
            0 => {}
            1 => {
                self.chat = format!("{} ", list[0]);
                self.tab = None;
            }
            _ => {
                let common = list.iter().skip(1).fold(list[0].clone(), |acc, s| {
                    acc.chars().zip(s.chars()).take_while(|(a, b)| a == b).map(|(a, _)| a).collect()
                });
                if common.chars().count() > self.chat.trim_end().chars().count() {
                    self.chat = common;
                    self.tab = None;
                } else {
                    self.chat = list[0].clone();
                    self.tab = Some((list, 0));
                }
            }
        }
    }
}

/// Types de galaxies, en minuscules sans accents (propositions de `/tp`).
fn galaxy_kind_names(settings: &GameSettings) -> Vec<String> {
    let mut names: Vec<String> = settings.galaxies.iter().map(|g| plain_name(g.kind.name())).collect();
    names.sort();
    names.dedup();
    names
}

fn plain_name(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| match c {
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'à' | 'â' | 'ä' => 'a',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            _ => c,
        })
        .collect()
}

#[derive(Component)]
struct ChatLines;

#[derive(Component)]
struct ChatInputBox;

#[derive(Component)]
struct ChatInputText;

/// Durée d'affichage d'un message quand le chat est fermé (secondes).
const CHAT_SHOW_SECS: f64 = 12.0;
const CHAT_FADE_SECS: f64 = 2.0;
const CHAT_VISIBLE_LINES: usize = 10;

#[derive(Component)]
struct NetPanelRoot;

#[derive(Component)]
struct NetOpenButton;

#[derive(Component)]
struct NetOpenLabel;

#[derive(Component)]
struct CloseButton;

#[derive(Component)]
struct FieldBox(Field);

#[derive(Component)]
struct FieldText(Field);

#[derive(Component)]
struct Swatch(usize);

#[derive(Component)]
struct JoinButton;

#[derive(Component)]
struct LeaveButton;

#[derive(Component)]
struct StatusText;

#[derive(Component)]
struct InviteText;

#[derive(Component)]
struct InviteHint;

#[derive(Component)]
struct CopyCodeButton;

#[derive(Component)]
struct NoticeText;

#[derive(Component)]
struct PlayersList;

/// Bouton de diplomatie d'un joueur : (faction, ma déclaration actuelle).
#[derive(Component)]
struct RelationButton {
    key: String,
    label: String,
    current: Relation,
}

#[derive(Component)]
struct GuildOpenButton;

#[derive(Component)]
struct GuildSummaryText;

/// Ligne « code d'un ami + Rejoindre » (masquée quand on a déjà rejoint un ami).
#[derive(Component)]
struct JoinRow;

// ─────────────────────────────────────────────────────────────────────────
//  Construction
// ─────────────────────────────────────────────────────────────────────────

pub(crate) fn text(label: impl Into<String>, size: f32, color: Color) -> impl Bundle {
    (Text::new(label.into()), TextFont { font_size: size, ..default() }, TextColor(color))
}

pub(crate) fn button(commands: &mut Commands, label: &str, border: Color, marker: impl Component) -> Entity {
    commands
        .spawn((
            Node {
                padding: UiRect::axes(Val::Px(12.0), Val::Px(7.0)),
                border: UiRect::all(Val::Px(2.0)),
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(BG_BUTTON),
            BorderColor(border),
            BorderRadius::all(Val::Px(6.0)),
            Button,
            marker,
        ))
        .with_child(text(label, 14.0, TEXT_COLOR))
        .id()
}

pub(crate) fn field(commands: &mut Commands, kind: Field, width: Val) -> Entity {
    commands
        .spawn((
            Node {
                width,
                height: Val::Px(30.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                border: UiRect::all(Val::Px(2.0)),
                align_items: AlignItems::Center,
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(BG_FIELD),
            BorderColor(TEXT_DIM),
            BorderRadius::all(Val::Px(5.0)),
            Button,
            FieldBox(kind),
        ))
        .with_child((text("", 14.0, TEXT_COLOR), FieldText(kind)))
        .id()
}

pub(crate) fn section_title(commands: &mut Commands, label: &str) -> Entity {
    commands
        .spawn((
            text(label, 12.0, TEXT_DIM),
            Node { margin: UiRect::top(Val::Px(4.0)), ..default() },
        ))
        .id()
}

fn setup_net_panel(mut commands: Commands) {
    // Bouton d'ouverture, à gauche du bouton « Options »
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(130.0),
                top: Val::Px(20.0),
                padding: UiRect::axes(Val::Px(16.0), Val::Px(8.0)),
                border: UiRect::all(Val::Px(2.0)),
                ..default()
            },
            BackgroundColor(BG_DARK),
            BorderColor(ACCENT),
            BorderRadius::all(Val::Px(6.0)),
            Button,
            NetOpenButton,
        ))
        .with_child((text("Multijoueur", 18.0, TEXT_COLOR), NetOpenLabel));

    let root = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(20.0),
                top: Val::Px(70.0),
                width: Val::Px(360.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(14.0)),
                row_gap: Val::Px(8.0),
                border: UiRect::all(Val::Px(2.0)),
                ..default()
            },
            BackgroundColor(BG_DARK),
            BorderColor(ACCENT),
            BorderRadius::all(Val::Px(10.0)),
            Visibility::Hidden,
            NetPanelRoot,
        ))
        .id();

    // Titre + fermer
    let header = commands
        .spawn(Node {
            width: Val::Percent(100.0),
            justify_content: JustifyContent::SpaceBetween,
            align_items: AlignItems::Center,
            ..default()
        })
        .id();
    let title = commands.spawn(text("MULTIJOUEUR", 20.0, TEXT_COLOR)).id();
    let close = button(&mut commands, "X", RED_SOFT, CloseButton);
    commands.entity(header).add_children(&[title, close]);

    // Profil
    let name_title = section_title(&mut commands, "VOTRE PSEUDO");
    let name_field = field(&mut commands, Field::Name, Val::Percent(100.0));
    let tag_title = section_title(&mut commands, "GUILDE");
    let tag_row = commands
        .spawn(Node {
            width: Val::Percent(100.0),
            column_gap: Val::Px(8.0),
            align_items: AlignItems::Center,
            ..default()
        })
        .id();
    let guild_btn = button(&mut commands, "Guilde (G)", ACCENT, GuildOpenButton);
    let guild_label = commands.spawn((text("", 12.0, TEXT_DIM), GuildSummaryText)).id();
    commands.entity(tag_row).add_children(&[guild_btn, guild_label]);
    let color_title = section_title(&mut commands, "COULEUR DU CONTOUR DE VOTRE VAISSEAU");
    let palette = commands
        .spawn(Node {
            width: Val::Percent(100.0),
            flex_wrap: FlexWrap::Wrap,
            column_gap: Val::Px(6.0),
            row_gap: Val::Px(6.0),
            ..default()
        })
        .id();
    for (i, c) in AURA_PALETTE.iter().enumerate() {
        let swatch = commands
            .spawn((
                Node {
                    width: Val::Px(24.0),
                    height: Val::Px(24.0),
                    border: UiRect::all(Val::Px(2.0)),
                    ..default()
                },
                BackgroundColor(Color::srgb(c[0], c[1], c[2])),
                BorderColor(Color::NONE),
                BorderRadius::all(Val::Px(12.0)),
                Button,
                Swatch(i),
            ))
            .id();
        commands.entity(palette).add_child(swatch);
    }

    // Joueurs
    let players_title = section_title(&mut commands, "DANS LA PARTIE");
    let status = commands.spawn((text("", 13.0, OK_COLOR), StatusText)).id();
    let players = commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(3.0),
                ..default()
            },
            PlayersList,
        ))
        .id();

    // Inviter un ami
    let invite_title = section_title(&mut commands, "INVITER UN AMI PAR INTERNET");
    let invite_row = commands.spawn(Node {
        width: Val::Percent(100.0),
        column_gap: Val::Px(8.0),
        align_items: AlignItems::Center,
        ..default()
    }).id();
    let invite = commands.spawn((text("", 26.0, CODE_COLOR), InviteText)).id();
    let copy_btn = button(&mut commands, "Copier", ACCENT, CopyCodeButton);
    commands.entity(invite_row).add_children(&[invite, copy_btn]);
    let invite_hint = commands.spawn((text("", 11.0, TEXT_DIM), InviteHint)).id();

    // Rejoindre un ami
    let join_title = section_title(&mut commands, "REJOINDRE UN AMI");
    let join_row = commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                column_gap: Val::Px(6.0),
                align_items: AlignItems::Center,
                ..default()
            },
            JoinRow,
        ))
        .id();
    let code_field = field(&mut commands, Field::Code, Val::Px(200.0));
    let join = button(&mut commands, "Rejoindre", ACCENT, JoinButton);
    commands.entity(join_row).add_children(&[code_field, join]);
    let leave = button(&mut commands, "Quitter la partie de votre ami", RED_SOFT, LeaveButton);
    let notice = commands.spawn((text("", 12.0, TEXT_DIM), NoticeText)).id();

    let help = commands
        .spawn((
            text("Sur le meme wifi / la meme box, les joueurs se retrouvent tout seuls.\nF2 : ouvrir / fermer ce panneau\nEntree : ecrire dans le chat  -  /g message : chat de guilde  -  /tp n : aller au trou noir d une galaxie (/aide)\nC : revendiquer / abandonner une etoile (5 max), ou assieger celle d'un autre\nF : tirer sur le vaisseau neutre ou ennemi le plus proche\nG : panneau Guilde (creer, rejoindre, gerer les membres et les relations)\nBouton Neutre / Allie / Ennemi a cote d'un joueur : changer de relation", 11.0, TEXT_DIM),
            Node { margin: UiRect::top(Val::Px(4.0)), ..default() },
        ))
        .id();

    commands.entity(root).add_children(&[
        header, name_title, name_field, tag_title, tag_row, color_title, palette,
        players_title, status, players,
        invite_title, invite_row, invite_hint,
        join_title, join_row, leave, notice,
        help,
    ]);
}

// ─────────────────────────────────────────────────────────────────────────
//  Interaction
// ─────────────────────────────────────────────────────────────────────────

fn toggle_net_panel(
    keys: Res<ButtonInput<KeyCode>>,
    open_btn: Query<&Interaction, (Changed<Interaction>, With<NetOpenButton>)>,
    close_btn: Query<&Interaction, (Changed<Interaction>, With<CloseButton>)>,
    guild_btn: Query<&Interaction, (Changed<Interaction>, With<GuildOpenButton>)>,
    menu: Res<crate::ui::MenuState>,
    mut panel: ResMut<NetPanel>,
    mut settings: ResMut<GameSettings>,
    mut net: ResMut<Net>,
) {
    let clicked_open = open_btn.iter().any(|i| *i == Interaction::Pressed);
    let clicked_close = close_btn.iter().any(|i| *i == Interaction::Pressed);
    let key = keys.just_pressed(KeyCode::F2);
    // Échap pendant la saisie d'un message : annule le message, rien d'autre
    if keys.just_pressed(KeyCode::Escape) && panel.focus == Some(Field::Chat) {
        panel.focus = None;
        panel.chat.clear();
        panel.esc_consumed = true;
        return;
    }
    // Panneau Guilde : G ou son bouton pour l'ouvrir, Échap le ferme en premier
    let guild_clicked = guild_btn.iter().any(|i| *i == Interaction::Pressed);
    let guild_key = keys.just_pressed(KeyCode::KeyG) && panel.focus.is_none() && !menu.open;
    if guild_clicked || guild_key {
        panel.guild_open = !panel.guild_open;
        if panel.guild_open {
            net.enable();
        } else if matches!(panel.focus, Some(Field::GuildName | Field::GuildTag)) {
            panel.focus = None;
        }
    }
    if keys.just_pressed(KeyCode::Escape) && matches!(panel.focus, Some(Field::GuildName | Field::GuildTag)) {
        panel.focus = None;
        panel.esc_consumed = true;
        return;
    }
    if keys.just_pressed(KeyCode::Escape) && panel.guild_open && panel.focus.is_none() {
        panel.guild_open = false;
        panel.esc_consumed = true;
        return;
    }
    let escape = keys.just_pressed(KeyCode::Escape) && panel.open && panel.focus.is_none();
    panel.esc_consumed = escape;

    if clicked_open || key {
        panel.open = !panel.open;
    } else if clicked_close || escape {
        panel.open = false;
    } else {
        return;
    }
    if panel.open {
        // Les ports ne sont ouverts qu'à partir d'ici
        net.enable();
    } else {
        commit_focus(&mut panel, &mut settings);
    }
}

/// Quitte le champ en cours d'édition (et sauvegarde le pseudo).
fn commit_focus(panel: &mut NetPanel, settings: &mut GameSettings) {
    if panel.focus == Some(Field::Name) {
        let trimmed = settings.player_name.trim().to_string();
        settings.player_name = if trimmed.is_empty() { crate::settings::default_player_name() } else { trimmed };
        settings.save();
    }
    panel.focus = None;
}

fn handle_panel_buttons(
    fields: Query<(&Interaction, &FieldBox), Changed<Interaction>>,
    swatches: Query<(&Interaction, &Swatch), Changed<Interaction>>,
    join: Query<&Interaction, (Changed<Interaction>, With<JoinButton>)>,
    leave: Query<&Interaction, (Changed<Interaction>, With<LeaveButton>)>,
    copy_btn: Query<&Interaction, (Changed<Interaction>, With<CopyCodeButton>)>,
    mouse: Res<ButtonInput<MouseButton>>,
    net: Res<Net>,
    mut panel: ResMut<NetPanel>,
    mut settings: ResMut<GameSettings>,
    mut commands_out: EventWriter<NetCommand>,
) {
    if !panel.open && !panel.guild_open {
        return;
    }
    let pressed = |i: &Interaction| *i == Interaction::Pressed;

    let clicked_field = fields.iter().find(|(i, _)| pressed(i)).map(|(_, f)| f.0);
    if let Some(f) = clicked_field {
        if panel.focus != Some(f) {
            commit_focus(&mut panel, &mut settings);
            panel.focus = Some(f);
        }
    } else if mouse.just_pressed(MouseButton::Left) && panel.focus.is_some() {
        // Clic ailleurs : on quitte le champ
        commit_focus(&mut panel, &mut settings);
    }

    for (i, s) in &swatches {
        if pressed(i) {
            settings.aura_color = AURA_PALETTE[s.0];
            settings.save();
        }
    }
    if join.iter().any(pressed) {
        commit_focus(&mut panel, &mut settings);
        commands_out.send(NetCommand::JoinCode(panel.code.clone()));
    }
    if leave.iter().any(pressed) {
        commands_out.send(NetCommand::Leave);
    }
    if copy_btn.iter().any(pressed) {
        if let Invite::Ready(code) = net.invite() {
            if let Ok(mut clip) = arboard::Clipboard::new() {
                clip.set_text(code).ok();
            }
        }
    }
}

fn handle_text_input(
    mut events: EventReader<KeyboardInput>,
    mut panel: ResMut<NetPanel>,
    mut settings: ResMut<GameSettings>,
    net: Res<Net>,
    menu: Res<crate::ui::MenuState>,
    keys: Res<ButtonInput<KeyCode>>,
    mut commands_out: EventWriter<NetCommand>,
    mut local_cmds: EventWriter<crate::chat_cmd::ChatCommand>,
) {
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    for ev in events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        let Some(focus) = panel.focus else {
            // Entrée : ouvrir le chat (seulement une fois le multijoueur activé)
            if ev.logical_key == Key::Enter && !menu.open {
                panel.focus = Some(Field::Chat);
            }
            continue;
        };
        if focus == Field::Dex {
            continue;
        }
        if !matches!(ev.logical_key, Key::Tab | Key::Shift) {
            panel.tab = None;
        }
        match &ev.logical_key {
            Key::Enter => {
                if focus == Field::Chat {
                    let msg = std::mem::take(&mut panel.chat);
                    panel.load_history();
                    panel.remember(&msg);
                    panel.history_pos = None;
                    panel.tab = None;
                    // Les commandes du jeu (/tp, /aide…) restent locales ; hors multijoueur, le chat aussi
                    if crate::chat_cmd::is_local(&msg) || !net.is_enabled() {
                        local_cmds.send(crate::chat_cmd::ChatCommand(msg));
                    } else {
                        commands_out.send(NetCommand::Chat(msg));
                    }
                }
                commit_focus(&mut panel, &mut settings);
                if focus == Field::Code {
                    commands_out.send(NetCommand::JoinCode(panel.code.clone()));
                }
                return;
            }
            Key::Escape => {
                panel.history_pos = None;
                panel.tab = None;
                commit_focus(&mut panel, &mut settings);
                return;
            }
            // Tab : propositions de commandes (Maj+Tab : en arrière)
            Key::Tab if focus == Field::Chat => {
                let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
                let kinds = galaxy_kind_names(&settings);
                let count = settings.galaxies.len();
                panel.complete(&kinds, count, shift);
                continue;
            }
            // Flèches : messages envoyés avant / après
            Key::ArrowUp if focus == Field::Chat => {
                panel.browse(true);
                continue;
            }
            Key::ArrowDown if focus == Field::Chat => {
                panel.browse(false);
                continue;
            }
            // Ctrl+Retour arrière : efface le dernier mot
            Key::Backspace if ctrl && focus == Field::Chat => {
                let trimmed = panel.chat.trim_end().to_string();
                let cut = trimmed.rfind(' ').map_or(0, |i| i + 1);
                panel.chat.truncate(cut);
                panel.tab = None;
                continue;
            }
            Key::Backspace => match focus {
                Field::Name => { settings.player_name.pop(); }
                Field::Code => { panel.code.pop(); }
                Field::GuildName => { panel.guild_name.pop(); }
                Field::GuildTag => { panel.guild_tag.pop(); }
                Field::Chat => { panel.chat.pop(); }
                Field::Dex => {}
            },
            Key::Space => {
                if focus == Field::GuildName && panel.guild_name.chars().count() < MAX_GUILD_NAME {
                    panel.guild_name.push(' ');
                }
                if focus == Field::Name && settings.player_name.chars().count() < MAX_NAME_LEN {
                    settings.player_name.push(' ');
                }
                if focus == Field::Chat && panel.chat.chars().count() < MAX_CHAT_LEN {
                    panel.chat.push(' ');
                }
            }
            Key::Character(s) if ctrl => {
                // Ctrl+V : coller ; Ctrl+C : copier le contenu du champ ; Ctrl+X : le couper
                match s.to_lowercase().as_str() {
                    "v" => {
                        if let Some(text) = arboard::Clipboard::new().ok().and_then(|mut c| c.get_text().ok()) {
                            for c in text.chars() {
                                insert_char(focus, c, &mut panel, &mut settings);
                            }
                        }
                    }
                    "c" | "x" => {
                        let content = match focus {
                            Field::Name => settings.player_name.clone(),
                            Field::Code => panel.code.clone(),
                            Field::GuildName => panel.guild_name.clone(),
                            Field::GuildTag => panel.guild_tag.clone(),
                            Field::Chat => panel.chat.clone(),
                            Field::Dex => String::new(),
                        };
                        if let Ok(mut clip) = arboard::Clipboard::new() {
                            clip.set_text(content).ok();
                        }
                        if s.eq_ignore_ascii_case("x") {
                            match focus {
                                Field::Name => settings.player_name.clear(),
                                Field::Code => panel.code.clear(),
                                Field::GuildName => panel.guild_name.clear(),
                                Field::GuildTag => panel.guild_tag.clear(),
                                Field::Chat => panel.chat.clear(),
                                Field::Dex => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            Key::Character(s) => {
                for c in s.chars() {
                    insert_char(focus, c, &mut panel, &mut settings);
                }
            }
            _ => {}
        }
    }
}

/// Ajoute un caractère saisi (ou collé) au champ actif, selon ses règles.
fn insert_char(focus: Field, c: char, panel: &mut NetPanel, settings: &mut GameSettings) {
    match focus {
        Field::GuildTag => {
            if c.is_alphanumeric() && panel.guild_tag.chars().count() < MAX_TAG_LEN {
                panel.guild_tag.extend(c.to_uppercase());
            }
        }
        Field::GuildName => {
            if !c.is_control() && panel.guild_name.chars().count() < MAX_GUILD_NAME {
                panel.guild_name.push(c);
            }
        }
        Field::Chat => {
            if !c.is_control() && panel.chat.chars().count() < MAX_CHAT_LEN {
                panel.chat.push(c);
            }
        }
        Field::Dex => {}
        Field::Name => {
            if !c.is_control() && settings.player_name.chars().count() < MAX_NAME_LEN {
                settings.player_name.push(c);
            }
        }
        Field::Code => {
            // Code d'invitation (ou, à défaut, une adresse IP)
            let allowed = c.is_ascii_alphanumeric() || ".:-[]".contains(c);
            if allowed && panel.code.len() < 64 {
                panel.code.push(c.to_ascii_uppercase());
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
//  Rafraîchissement de l'affichage
// ─────────────────────────────────────────────────────────────────────────

pub(crate) fn set_display(node: &mut Node, visible: bool) {
    let want = if visible { Display::Flex } else { Display::None };
    if node.display != want {
        node.display = want;
    }
}

fn update_panel_visibility(
    panel: Res<NetPanel>,
    net: Res<Net>,
    mut root: Query<&mut Visibility, With<NetPanelRoot>>,
    mut join_row: Query<&mut Node, (With<JoinRow>, Without<LeaveButton>)>,
    mut leave: Query<&mut Node, (With<LeaveButton>, Without<JoinRow>)>,
) {
    for mut vis in &mut root {
        let want = if panel.open { Visibility::Visible } else { Visibility::Hidden };
        if *vis != want {
            *vis = want;
        }
    }
    let joined = net.joined_by_code();
    for mut node in &mut join_row {
        set_display(&mut node, !joined);
    }
    for mut node in &mut leave {
        set_display(&mut node, joined);
    }
}

fn update_fields(
    time: Res<Time>,
    panel: Res<NetPanel>,
    settings: Res<GameSettings>,
    mut boxes: Query<(&FieldBox, &mut BorderColor)>,
    mut texts: Query<(&FieldText, &mut Text, &mut TextColor)>,
) {
    if !panel.open && !panel.guild_open {
        return;
    }
    let caret = (time.elapsed_secs() * 2.0) as u32 % 2 == 0;
    for (fb, mut border) in &mut boxes {
        let c = if panel.focus == Some(fb.0) { ACCENT } else { TEXT_DIM };
        if border.0 != c {
            border.0 = c;
        }
    }
    for (ft, mut t, mut color) in &mut texts {
        let (value, placeholder) = match ft.0 {
            Field::Name => (settings.player_name.as_str(), "Votre pseudo"),
            Field::GuildName => (panel.guild_name.as_str(), "Nom de la guilde"),
            Field::GuildTag => (panel.guild_tag.as_str(), "TAG"),
            Field::Code => (panel.code.as_str(), "Code de votre ami"),
            Field::Chat | Field::Dex => continue,
        };
        let focused = panel.focus == Some(ft.0);
        let shown = if value.is_empty() && !focused {
            placeholder.to_string()
        } else if focused && caret {
            format!("{value}|")
        } else {
            value.to_string()
        };
        if t.0 != shown {
            t.0 = shown;
        }
        let c = if value.is_empty() && !focused { TEXT_DIM } else { TEXT_COLOR };
        if color.0 != c {
            color.0 = c;
        }
    }
}

fn update_swatches(settings: Res<GameSettings>, mut swatches: Query<(&Swatch, &mut BorderColor)>) {
    for (s, mut border) in &mut swatches {
        let selected = AURA_PALETTE[s.0] == settings.aura_color;
        let c = if selected { Color::WHITE } else { Color::NONE };
        if border.0 != c {
            border.0 = c;
        }
    }
}

fn set_text(t: &mut Text, c: &mut TextColor, value: String, color: Color) {
    if t.0 != value {
        t.0 = value;
    }
    if c.0 != color {
        c.0 = color;
    }
}

fn update_texts(
    net: Res<Net>,
    mut q: ParamSet<(
        Query<(&mut Text, &mut TextColor), With<NetOpenLabel>>,
        Query<(&mut Text, &mut TextColor), With<StatusText>>,
        Query<(&mut Text, &mut TextColor), With<InviteText>>,
        Query<(&mut Text, &mut TextColor), With<InviteHint>>,
        Query<(&mut Text, &mut TextColor), With<NoticeText>>,
    )>,
) {
    let count = net.player_count();
    let mode = net.mode();

    // Bouton : nombre de joueurs connectés
    let label = if count > 1 { format!("Multijoueur ({count})") } else { "Multijoueur".into() };
    for (mut t, mut c) in &mut q.p0() {
        set_text(&mut t, &mut c, label.clone(), TEXT_COLOR);
    }

    let (status, color) = match mode {
        NetMode::Joining => ("Connexion...".to_string(), TEXT_DIM),
        _ if count > 1 => (format!("{count} joueurs connectes"), OK_COLOR),
        _ => ("Vous etes seul pour l'instant. Recherche d'autres joueurs...".to_string(), TEXT_DIM),
    };
    for (mut t, mut c) in &mut q.p1() {
        set_text(&mut t, &mut c, status.clone(), color);
    }

    let (code, code_color, hint) = match net.invite() {
        Invite::Ready(code) => (
            code,
            CODE_COLOR,
            "Donnez ce code a votre ami : il le tape dans \"Rejoindre un ami\".".to_string(),
        ),
        Invite::Pending => ("...".to_string(), TEXT_DIM, "Preparation du code...".to_string()),
        Invite::Unavailable(reason) => (
            "Indisponible".to_string(),
            TEXT_DIM,
            format!("{reason}\nVous pouvez quand meme jouer : demandez son code a votre ami (c'est lui qui heberge)."),
        ),
    };
    for (mut t, mut c) in &mut q.p2() {
        set_text(&mut t, &mut c, code.clone(), code_color);
    }
    for (mut t, mut c) in &mut q.p3() {
        set_text(&mut t, &mut c, hint.clone(), TEXT_DIM);
    }

    let notice_color = if net.notice_is_error { ERROR_COLOR } else { OK_COLOR };
    for (mut t, mut c) in &mut q.p4() {
        set_text(&mut t, &mut c, net.notice.clone(), notice_color);
    }
}

struct PlayerRow {
    name: String,
    tag: String,
    color: [f32; 3],
    me: bool,
    /// (faction, ma déclaration, relation effective, même guilde) pour les autres joueurs.
    relation: Option<(String, Relation, Relation, bool)>,
}

fn rebuild_players_list(
    mut commands: Commands,
    net: Res<Net>,
    settings: Res<GameSettings>,
    list: Query<Entity, With<PlayersList>>,
    guilds: Res<Guilds>,
    mut last: Local<Option<String>>,
) {
    // Regroupés par guilde, « sans guilde » à la fin
    let can_set = settings.guild.is_none() || has_rank(&settings, Role::Deputy);
    let my_tag = sanitize_tag(&settings.clan_tag);
    let mut players = vec![PlayerRow {
        name: settings.player_name.clone(),
        tag: my_tag.clone(),
        color: settings.aura_color,
        me: true,
        relation: None,
    }];
    players.extend(net.peers.values().map(|p| {
        let key = faction_key(p.gid, &p.name);
        let mine = my_declared(&settings, &key);
        PlayerRow {
            name: p.name.clone(),
            tag: p.tag.clone(),
            color: p.color,
            me: false,
            relation: Some((key, mine, relation_with(p, &settings), same_guild(p, &settings))),
        }
    }));
    players.sort_by(|a, b| {
        (a.tag.is_empty(), &a.tag, !a.me, a.name.to_lowercase())
            .cmp(&(b.tag.is_empty(), &b.tag, !b.me, b.name.to_lowercase()))
    });
    let any_guild = players.iter().any(|p| !p.tag.is_empty());

    let signature: String = players
        .iter()
        .map(|p| format!("{}|{}|{:?}|{:?}|{can_set}|{};", p.tag, p.name, p.color, p.relation, guilds.by_tag(&p.tag).map_or("", |g| g.name.as_str())))
        .collect();
    if last.as_deref() == Some(signature.as_str()) {
        return;
    }
    *last = Some(signature);
    let Ok(list) = list.get_single() else { return };
    commands.entity(list).despawn_descendants();

    let mut current_group: Option<String> = None;
    for p in players {
        if any_guild && current_group.as_deref() != Some(p.tag.as_str()) {
            let count = net.peers.values().filter(|o| o.tag == p.tag).count() + usize::from(my_tag == p.tag);
            // Relation avec cette guilde, bien visible dans l'en-tête
            let (header, color) = if p.tag.is_empty() {
                (format!("Sans guilde ({count})"), CODE_COLOR)
            } else {
                let name = guilds.by_tag(&p.tag).map_or("", |g| g.name.as_str());
                let base = format!("Guilde [{}] {name} ({count})", p.tag);
                match p.relation {
                    Some((_, _, _, true)) | None => (format!("{base} - VOTRE GUILDE"), CODE_COLOR),
                    Some((_, _, effective, false)) => {
                        (format!("{base} - {}", effective.label().to_uppercase()), effective.color())
                    }
                }
            };
            let h = commands
                .spawn((text(header, 12.0, color), Node { margin: UiRect::top(Val::Px(3.0)), ..default() }))
                .id();
            commands.entity(list).add_child(h);
            current_group = Some(p.tag.clone());
        }
        let row = commands
            .spawn(Node {
                width: Val::Percent(100.0),
                column_gap: Val::Px(8.0),
                align_items: AlignItems::Center,
                flex_wrap: FlexWrap::Wrap,
                ..default()
            })
            .id();
        let c = p.color;
        let dot = commands
            .spawn((
                Node { width: Val::Px(12.0), height: Val::Px(12.0), ..default() },
                BackgroundColor(Color::srgb(c[0], c[1], c[2])),
                BorderRadius::all(Val::Px(6.0)),
            ))
            .id();
        let name = display_name(&p.tag, &p.name);
        let label = if p.me { format!("{name} (vous)") } else { name };
        let t = commands.spawn(text(label, 13.0, TEXT_COLOR)).id();
        commands.entity(row).add_children(&[dot, t]);

        // Diplomatie : bouton pour changer ma position, puis la relation réelle
        match p.relation {
            Some((_, _, _, true)) => {
                let note = commands.spawn(text("meme guilde : allie", 11.0, Relation::Ally.color())).id();
                commands.entity(row).add_child(note);
            }
            Some((key, mine, effective, false)) => {
                // Dans une guilde, seuls le Chef et les Sous-chefs décident des relations
                let btn = if can_set {
                    commands
                        .spawn((
                            Node {
                                padding: UiRect::axes(Val::Px(8.0), Val::Px(2.0)),
                                border: UiRect::all(Val::Px(1.0)),
                                ..default()
                            },
                            BackgroundColor(BG_BUTTON),
                            BorderColor(mine.color()),
                            BorderRadius::all(Val::Px(5.0)),
                            Button,
                            RelationButton { key, label: faction_label(&p.tag, &p.name), current: mine },
                        ))
                        .with_child(text(mine.label(), 12.0, mine.color()))
                        .id()
                } else {
                    commands.spawn(text(effective.label(), 12.0, effective.color())).id()
                };
                commands.entity(row).add_child(btn);
                let note = match (mine, effective) {
                    (Relation::Ally, Relation::Ally) => Some("alliance conclue"),
                    (Relation::Ally, Relation::Neutral) => Some("en attente de son accord"),
                    (Relation::Ally, Relation::Enemy) | (Relation::Neutral, Relation::Enemy) => {
                        Some("vous a declare la guerre")
                    }
                    (Relation::Neutral, _) => None,
                    (Relation::Enemy, _) => Some("en guerre"),
                };
                if let Some(note) = note {
                    let n = commands.spawn(text(note, 11.0, effective.color())).id();
                    commands.entity(row).add_child(n);
                }
            }
            None => {}
        }
        commands.entity(list).add_child(row);
    }
}

/// Clic sur le bouton de relation d'un joueur : Neutre → Allié → Ennemi → Neutre.
/// Dans une guilde, c'est la relation de toute la guilde qui change.
fn handle_relation_buttons(
    time: Res<Time>,
    buttons: Query<(&Interaction, &RelationButton), Changed<Interaction>>,
    mut settings: ResMut<GameSettings>,
    mut guilds: ResMut<Guilds>,
    mut net: ResMut<Net>,
) {
    for (interaction, button) in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        change_relation(&mut settings, &mut guilds, &mut net, time.elapsed_secs_f64(), &button.key, &button.label, button.current.next());
    }
}

/// Change ma relation (ou celle de ma guilde) envers une faction.
pub(crate) fn change_relation(
    settings: &mut GameSettings,
    guilds: &mut Guilds,
    net: &mut Net,
    now: f64,
    key: &str,
    label: &str,
    relation: Relation,
) {
    if settings.guild.is_some() {
        crate::guild::edit(settings, guilds, net, now, |g, me| {
            g.set_relation(me, key, label, relation)?;
            Ok(Some(format!("Votre guilde est maintenant {} envers {label}.", relation.label().to_lowercase())))
        });
    } else {
        set_personal(settings, key, relation);
    }
}

fn update_guild_summary(settings: Res<GameSettings>, mut q: Query<(&mut Text, &mut TextColor), With<GuildSummaryText>>) {
    let (label, color) = match &settings.guild {
        Some(g) => {
            let role = g.role_of(settings.player_id).map_or("", |r| r.label());
            (format!("[{}] {}\n{role}", g.tag, g.name), TEXT_COLOR)
        }
        None => ("Sans guilde : creez-en une\nou demandez a en rejoindre une.".to_string(), TEXT_DIM),
    };
    for (mut t, mut c) in &mut q {
        set_text(&mut t, &mut c, label.clone(), color);
    }
}

// ─────────────────────────────────────────────────────────────────────────
//  Chat écrit
// ─────────────────────────────────────────────────────────────────────────

/// Élément d'un message qui s'efface avec le temps (`base` = couleur pleine).
#[derive(Component)]
struct ChatFade {
    time: f64,
    base: Color,
}

fn setup_chat(mut commands: Commands) {
    let root = commands
        .spawn(Node {
            position_type: PositionType::Absolute,
            left: Val::Px(20.0),
            bottom: Val::Px(20.0),
            width: Val::Px(440.0),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(4.0),
            ..default()
        })
        .id();
    let lines = commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::FlexStart,
                row_gap: Val::Px(2.0),
                ..default()
            },
            ChatLines,
        ))
        .id();
    let input = commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                min_height: Val::Px(30.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(5.0)),
                border: UiRect::all(Val::Px(2.0)),
                align_items: AlignItems::Center,
                display: Display::None,
                ..default()
            },
            BackgroundColor(BG_DARK),
            BorderColor(ACCENT),
            BorderRadius::all(Val::Px(5.0)),
            ChatInputBox,
        ))
        .with_child((text("", 14.0, TEXT_COLOR), ChatInputText))
        .id();
    commands.entity(root).add_children(&[lines, input]);
}

fn chat_alpha(open: bool, age: f64) -> f32 {
    if open {
        1.0
    } else {
        (((CHAT_SHOW_SECS + CHAT_FADE_SECS - age) / CHAT_FADE_SECS).clamp(0.0, 1.0)) as f32
    }
}

fn update_chat(
    mut commands: Commands,
    time: Res<Time>,
    net: Res<Net>,
    panel: Res<NetPanel>,
    settings: Res<GameSettings>,
    lines_q: Query<Entity, With<ChatLines>>,
    mut input_box: Query<&mut Node, With<ChatInputBox>>,
    mut input_text: Query<&mut Text, With<ChatInputText>>,
    mut fade_text: Query<(&ChatFade, &mut TextColor), Without<BackgroundColor>>,
    mut fade_bg: Query<(&ChatFade, &mut BackgroundColor)>,
    mut last: Local<Option<(u64, bool, usize)>>,
) {
    let now = time.elapsed_secs_f64();
    let open = panel.focus == Some(Field::Chat);

    // Saisie
    for mut node in &mut input_box {
        set_display(&mut node, open);
    }
    if open {
        let caret = if (time.elapsed_secs() * 2.0) as u32 % 2 == 0 { "|" } else { "" };
        let prompt = if panel.chat.trim_start().starts_with("/g ") { "Guilde >" } else { ">" };
        let mut shown = format!("{prompt} {}{caret}", panel.chat);
        // Sous la ligne : l'aide de la commande et les propositions (Tab)
        if panel.chat.trim_start().starts_with('/') {
            let list = crate::chat_cmd::suggestions(&panel.chat, &galaxy_kind_names(&settings), settings.galaxies.len());
            if let Some(u) = crate::chat_cmd::usage(&panel.chat) {
                shown.push_str(&format!("\n   {u}"));
            }
            if !list.is_empty() && !(list.len() == 1 && list[0].trim() == panel.chat.trim()) {
                let words: Vec<String> = list.iter().take(8).map(|l| l.rsplit(' ').next().unwrap_or(l).to_string()).collect();
                let more = if list.len() > 8 { format!("  (+{})", list.len() - 8) } else { String::new() };
                shown.push_str(&format!("\n   Tab : {}{more}", words.join("  ")));
            }
        } else if panel.chat.is_empty() {
            shown.push_str("\n   / : commandes (Tab pour completer)   Fleches : messages precedents");
        }
        for mut t in &mut input_text {
            if t.0 != shown {
                t.0 = shown.clone();
            }
        }
    }

    // Messages : tous les derniers si le chat est ouvert, sinon les récents
    let entries: Vec<_> = net.chat.lines.iter()
        .rev()
        .take(CHAT_VISIBLE_LINES)
        .filter(|e| open || now - e.time < CHAT_SHOW_SECS + CHAT_FADE_SECS)
        .collect();
    let signature = (net.chat.total, open, entries.len());
    if *last != Some(signature) {
        *last = Some(signature);
        let Ok(list) = lines_q.get_single() else { return };
        commands.entity(list).despawn_descendants();
        for e in entries.iter().rev() {
            let name_color = Color::srgb(e.color[0], e.color[1], e.color[2]);
            let bg = Color::srgba(0.0, 0.0, 0.0, 0.45);
            let line = commands
                .spawn((
                    Text::new(""),
                    TextFont { font_size: 15.0, ..default() },
                    Node { padding: UiRect::axes(Val::Px(6.0), Val::Px(2.0)), max_width: Val::Percent(100.0), ..default() },
                    BackgroundColor(bg),
                    BorderRadius::all(Val::Px(4.0)),
                    ChatFade { time: e.time, base: bg },
                ))
                .with_children(|p| {
                    let mut span = |s: String, color: Color| {
                        p.spawn((
                            TextSpan::new(s),
                            TextFont { font_size: 15.0, ..default() },
                            TextColor(color),
                            ChatFade { time: e.time, base: color },
                        ));
                    };
                    if e.system {
                        span(e.text.clone(), TEXT_DIM);
                        return;
                    }
                    if e.guild {
                        span(format!("(Guilde [{}]) ", e.tag), GUILD_COLOR);
                    }
                    span(format!("{} : ", display_name(&e.tag, &e.name)), name_color);
                    span(e.text.clone(), if e.guild { GUILD_COLOR } else { TEXT_COLOR });
                })
                .id();
            commands.entity(list).add_child(line);
        }
    }

    // Effacement progressif
    for (f, mut c) in &mut fade_text {
        let a = f.base.alpha() * chat_alpha(open, now - f.time);
        if (c.0.alpha() - a).abs() > 0.01 {
            c.0 = f.base.with_alpha(a);
        }
    }
    for (f, mut c) in &mut fade_bg {
        let a = f.base.alpha() * chat_alpha(open, now - f.time);
        if (c.0.alpha() - a).abs() > 0.01 {
            c.0 = f.base.with_alpha(a);
        }
    }
}

#[cfg(test)]
mod chat_input_tests {
    use super::*;

    #[test]
    fn arrows_browse_history_and_restore_the_draft() {
        let mut p = NetPanel { history_loaded: true, ..Default::default() };
        p.remember("/stats");
        p.remember("bonjour");
        p.remember("bonjour");
        p.chat = "en cours".into();
        p.browse(true);
        assert_eq!(p.chat, "bonjour");
        p.browse(true);
        assert_eq!(p.chat, "/stats");
        p.browse(true);
        assert_eq!(p.chat, "/stats", "reste sur le plus ancien");
        p.browse(false);
        assert_eq!(p.chat, "bonjour");
        p.browse(false);
        assert_eq!(p.chat, "en cours", "la ligne en cours revient");
    }

    #[test]
    fn tab_completes_then_cycles() {
        let mut p = NetPanel::default();
        p.chat = "/st".into();
        p.complete(&[], 21, false);
        assert_eq!(p.chat, "/stats ");
        p.chat = "/aller planete m".into();
        p.complete(&[], 21, false);
        // mini-neptune, methane, mars : pas de préfixe commun plus long -> on parcourt
        let first = p.chat.clone();
        p.complete(&[], 21, false);
        assert_ne!(p.chat, first);
        assert!(p.chat.starts_with("/aller planete m"));
        p.complete(&[], 21, true);
        assert_eq!(p.chat, first, "Maj+Tab revient en arriere");
    }
}
