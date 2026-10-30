//! Page Options > Sauvegardes : les sauvegardes de tous les jeux.
//!
//! - le dossier des sauvegardes et un bouton pour l'ouvrir dans l'explorateur ;
//! - la liste des sauvegardes rangées par jeu : emplacement, taille, âge, bouton « Supprimer »
//!   (premier clic : « Confirmer ? » en rouge, second clic dans les 3 s : suppression) ;
//! - la liste se relit sur le disque toutes les secondes tant que la page est montrée.
//!
//! `spawn_page` ne peut pas lire de ressources : il pose la partie fixe et un conteneur vide
//! (`SavesList`) que le système `refresh_list` remplit et refait quand quelque chose change.

use std::time::SystemTime;

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::Pressed;
use bevy::ui_widgets::{Activate, Button};

use crate::save::{delete_save, list_saves, saves_dir, SaveInfo};
use crate::studio::GameRegistry;

const ITEM: Color = Color::srgb(0.16, 0.17, 0.2);
const ITEM_HOVER: Color = Color::srgb(0.22, 0.24, 0.28);
const ITEM_ON: Color = Color::srgb(0.24, 0.36, 0.62);
const DANGER: Color = Color::srgb(0.7, 0.2, 0.2);
const DANGER_HOVER: Color = Color::srgb(0.82, 0.26, 0.26);
const TEXT: Color = Color::srgb(0.9, 0.91, 0.94);
const DIM: Color = Color::srgb(0.6, 0.63, 0.7);

/// Lignes de sauvegarde montrées au plus (le reste : « +N autres »).
const MAX_ROWS: usize = 12;
/// Délai pour confirmer une suppression (secondes).
const CONFIRM_DELAY: f64 = 3.0;
/// Relecture du disque (secondes).
const POLL_EVERY: f64 = 1.0;

/// Conteneur de la liste : ses enfants sont refaits par `refresh_list`.
#[derive(Component, Default)]
pub struct SavesList {
    /// Ce qui est affiché (pour savoir quand refaire la liste).
    built: Option<Vec<String>>,
    next_poll: f64,
}

#[derive(Component)]
struct OpenFolderButton;

#[derive(Component, Clone)]
struct DeleteButton {
    game: String,
    slot: String,
}

/// Suppression en attente de confirmation : (jeu, emplacement, heure du premier clic).
#[derive(Resource, Default)]
struct PendingDelete(Option<(String, String, f64)>);

pub struct SavesPagePlugin;

impl Plugin for SavesPagePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PendingDelete>().add_systems(Update, (expire_pending, refresh_list, button_looks).chain());
    }
}

fn label(text: impl Into<String>, size: f32, color: Color) -> impl Bundle {
    (Text::new(text), TextFont::from_font_size(size), TextColor(color))
}

fn item_node() -> Node {
    Node { padding: UiRect::axes(px(14), px(7)), border_radius: BorderRadius::all(px(6)), justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() }
}

/// Contenu de la page (appelé par `options` quand on ouvre la page).
pub fn spawn_page(c: &mut ChildSpawnerCommands) {
    c.spawn(label("Sauvegardes", 24.0, TEXT));
    c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(12), ..default() }).with_children(|row| {
        row.spawn((Button, Hovered::default(), item_node(), BackgroundColor(ITEM), OpenFolderButton)).observe(open_folder).with_child(label("Ouvrir le dossier", 15.0, TEXT));
        row.spawn(label(saves_dir().display().to_string(), 13.0, DIM));
    });
    c.spawn((Node { flex_direction: FlexDirection::Column, row_gap: px(6), max_height: px(560), overflow: Overflow::clip(), ..default() }, SavesList::default()));
}

/// Ouvre le dossier des sauvegardes dans l'explorateur de fichiers (créé s'il manque).
fn open_folder(_: On<Activate>) {
    let dir = saves_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        warn!("dossier des sauvegardes impossible a creer : {}: {e}", dir.display());
        return;
    }
    let program = if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if let Err(e) = std::process::Command::new(program).arg(&dir).spawn() {
        warn!("impossible d'ouvrir {} : {e}", dir.display());
    }
}

/// Taille lisible : « 512 o », « 3,4 Ko », « 1,2 Mo ».
pub fn format_size(bytes: u64) -> String {
    const KO: u64 = 1024;
    const MO: u64 = 1024 * 1024;
    let decimal = |value: u64, unit: u64, name: &str| {
        let tenths = (value * 10 + unit / 2) / unit;
        if tenths >= 100 { format!("{} {name}", tenths / 10) } else { format!("{},{} {name}", tenths / 10, tenths % 10) }
    };
    if bytes < KO {
        format!("{bytes} o")
    } else if bytes < MO {
        decimal(bytes, KO, "Ko")
    } else {
        decimal(bytes, MO, "Mo")
    }
}

/// Âge lisible à partir d'un nombre de secondes : « a l'instant », « il y a 5 min », « il y a 2 h »,
/// « il y a 3 j ».
pub fn format_age(seconds: u64) -> String {
    match seconds {
        0..60 => "a l'instant".into(),
        60..3600 => format!("il y a {} min", seconds / 60),
        3600..86400 => format!("il y a {} h", seconds / 3600),
        _ => format!("il y a {} j", seconds / 86400),
    }
}

fn age_of(modified: Option<SystemTime>, now: SystemTime) -> String {
    match modified {
        // Date dans le futur (horloge changée) : comptée comme maintenant
        Some(time) => format_age(now.duration_since(time).map_or(0, |d| d.as_secs())),
        None => "date inconnue".into(),
    }
}

fn is_pending(pending: &PendingDelete, game: &str, slot: &str) -> bool {
    pending.0.as_ref().is_some_and(|(g, s, _)| g == game && s == slot)
}

/// La confirmation expire après 3 s.
fn expire_pending(time: Res<Time<Real>>, mut pending: ResMut<PendingDelete>) {
    if pending.0.as_ref().is_some_and(|(_, _, since)| time.elapsed_secs_f64() - since > CONFIRM_DELAY) {
        pending.0 = None;
    }
}

/// Une ligne de la liste, déjà mise en forme.
enum Line {
    Game(String),
    Save { info: SaveInfo, size: String, age: String, pending: bool },
    More(usize),
    Empty,
}

impl Line {
    /// Clé de comparaison (refaire la liste seulement si une clé change).
    fn key(&self) -> String {
        match self {
            Line::Game(name) => format!("g:{name}"),
            Line::Save { info, size, age, pending } => format!("s:{}/{}:{size}:{age}:{pending}", info.game, info.slot),
            Line::More(n) => format!("m:{n}"),
            Line::Empty => "e".into(),
        }
    }
}

fn lines(saves: Vec<SaveInfo>, registry: Option<&GameRegistry>, pending: &PendingDelete, now: SystemTime) -> Vec<Line> {
    if saves.is_empty() {
        return vec![Line::Empty];
    }
    // Vrai nom du jeu s'il est enregistré dans le studio
    let game_name = |id: &str| registry.and_then(|r| r.specs().find(|s| s.id == id).map(|s| format!("{} ({id})", s.name))).unwrap_or_else(|| id.to_string());
    let total = saves.len();
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    for info in saves.into_iter().take(MAX_ROWS) {
        if current.as_deref() != Some(info.game.as_str()) {
            current = Some(info.game.clone());
            out.push(Line::Game(game_name(&info.game)));
        }
        let pending = is_pending(pending, &info.game, &info.slot);
        out.push(Line::Save { size: format_size(info.size), age: age_of(info.modified, now), pending, info });
    }
    if total > MAX_ROWS {
        out.push(Line::More(total - MAX_ROWS));
    }
    out
}

/// Relit les sauvegardes toutes les secondes (et tout de suite quand la confirmation change)
/// et refait la liste si ce qui est affiché a changé.
fn refresh_list(mut commands: Commands, time: Res<Time<Real>>, registry: Option<Res<GameRegistry>>, pending: Res<PendingDelete>, mut lists: Query<(Entity, &mut SavesList)>) {
    let now = time.elapsed_secs_f64();
    for (entity, mut list) in &mut lists {
        if list.built.is_some() && now < list.next_poll && !pending.is_changed() {
            continue;
        }
        list.next_poll = now + POLL_EVERY;
        let lines = lines(list_saves(), registry.as_deref(), &pending, SystemTime::now());
        let keys: Vec<String> = lines.iter().map(Line::key).collect();
        if list.built.as_ref() == Some(&keys) {
            continue;
        }
        list.built = Some(keys);
        commands.entity(entity).despawn_children().with_children(|c| {
            for line in lines {
                match line {
                    Line::Game(name) => {
                        c.spawn((label(name, 17.0, TEXT), Node { margin: UiRect::top(px(8)), ..default() }));
                    }
                    Line::Save { info, size, age, pending } => {
                        c.spawn(Node { flex_direction: FlexDirection::Row, align_items: AlignItems::Center, column_gap: px(16), padding: UiRect::left(px(16)), ..default() }).with_children(|row| {
                            row.spawn((label(info.slot.clone(), 15.0, TEXT), Node { width: px(220), ..default() }));
                            row.spawn((label(size, 14.0, DIM), Node { width: px(80), ..default() }));
                            row.spawn((label(age, 14.0, DIM), Node { width: px(140), ..default() }));
                            row.spawn((Button, Hovered::default(), Node { min_width: px(120), ..item_node() }, BackgroundColor(if pending { DANGER } else { ITEM }), DeleteButton { game: info.game, slot: info.slot }))
                                .observe(delete_clicked)
                                .with_child(label(if pending { "Confirmer ?" } else { "Supprimer" }, 14.0, TEXT));
                        });
                    }
                    Line::More(n) => {
                        c.spawn((label(format!("+{n} autres (voir le dossier)"), 14.0, DIM), Node { margin: UiRect::top(px(6)), ..default() }));
                    }
                    Line::Empty => {
                        c.spawn(label("Aucune sauvegarde pour l'instant. Les jeux enregistrent ici leur progression.", 15.0, DIM));
                    }
                }
            }
        });
    }
}

/// Premier clic : demande confirmation ; second clic dans les 3 s : supprime.
fn delete_clicked(activate: On<Activate>, buttons: Query<&DeleteButton>, time: Res<Time<Real>>, mut pending: ResMut<PendingDelete>, mut lists: Query<&mut SavesList>) {
    let Ok(button) = buttons.get(activate.entity) else { return };
    let now = time.elapsed_secs_f64();
    let confirmed = pending.0.as_ref().is_some_and(|(g, s, since)| g == &button.game && s == &button.slot && now - since <= CONFIRM_DELAY);
    if confirmed {
        delete_save(&button.game, &button.slot);
        pending.0 = None;
        for mut list in &mut lists {
            list.next_poll = 0.0;
        }
    } else {
        pending.0 = Some((button.game.clone(), button.slot.clone(), now));
    }
}

fn button_looks(pending: Res<PendingDelete>, mut buttons: Query<(&Hovered, Has<Pressed>, &mut BackgroundColor, Option<&DeleteButton>), Or<(With<OpenFolderButton>, With<DeleteButton>)>>) {
    for (hovered, pressed, mut color, delete) in &mut buttons {
        let danger = delete.is_some_and(|d| is_pending(&pending, &d.game, &d.slot));
        let wanted = match (danger, pressed, hovered.get()) {
            (true, _, true) => DANGER_HOVER,
            (true, _, false) => DANGER,
            (false, true, _) => ITEM_ON,
            (false, false, true) => ITEM_HOVER,
            (false, false, false) => ITEM,
        };
        if color.0 != wanted {
            color.0 = wanted;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn sizes_are_readable() {
        assert_eq!(format_size(0), "0 o");
        assert_eq!(format_size(512), "512 o");
        assert_eq!(format_size(1023), "1023 o");
        assert_eq!(format_size(1024), "1,0 Ko");
        assert_eq!(format_size(3500), "3,4 Ko");
        assert_eq!(format_size(20 * 1024), "20 Ko");
        assert_eq!(format_size(1024 * 1024 + 200 * 1024), "1,2 Mo");
    }

    #[test]
    fn ages_are_relative() {
        assert_eq!(format_age(0), "a l'instant");
        assert_eq!(format_age(59), "a l'instant");
        assert_eq!(format_age(5 * 60 + 10), "il y a 5 min");
        assert_eq!(format_age(2 * 3600 + 5), "il y a 2 h");
        assert_eq!(format_age(3 * 86400 + 7), "il y a 3 j");
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        assert_eq!(age_of(Some(now - Duration::from_secs(120)), now), "il y a 2 min");
        assert_eq!(age_of(Some(now + Duration::from_secs(50)), now), "a l'instant");
        assert_eq!(age_of(None, now), "date inconnue");
    }

    #[test]
    fn lines_group_by_game_and_cap() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let save = |game: &str, slot: &str| SaveInfo { game: game.into(), slot: slot.into(), path: Default::default(), size: 10, modified: Some(now) };
        assert!(matches!(lines(vec![], None, &PendingDelete::default(), now)[..], [Line::Empty]));
        let pending = PendingDelete(Some(("a".into(), "2".into(), 0.0)));
        let keys: Vec<String> = lines(vec![save("a", "1"), save("a", "2"), save("b", "1")], None, &pending, now).iter().map(Line::key).collect();
        assert_eq!(keys, ["g:a", "s:a/1:10 o:a l'instant:false", "s:a/2:10 o:a l'instant:true", "g:b", "s:b/1:10 o:a l'instant:false"]);
        let many: Vec<SaveInfo> = (0..15).map(|i| save("a", &format!("{i:02}"))).collect();
        let out = lines(many, None, &PendingDelete::default(), now);
        assert_eq!(out.len(), 1 + MAX_ROWS + 1);
        assert!(matches!(out.last(), Some(Line::More(3))));
    }
}
