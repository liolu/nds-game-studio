// ─────────────────────────────────────────────────────────────────────────
//  Commandes du chat (locales : elles ne sont jamais envoyées aux autres joueurs)
//
//    /tp <n>         : va au trou noir de la galaxie n (0 = notre galaxie)
//    /tp <type>      : va à la première galaxie de ce type (ex. /tp annulaire)
//    /tp liste       : les types de galaxies et leurs numéros
//    /profil         : exporte en JSON le profil de l'astre ciblé (étoile, planète, lune)
//    /graine         : la graine du monde en code court (copiée dans le presse-papiers)
//    /graine <code>  : la graine qui correspond à un code
//    /aller ...      : tests, aller à un type d'étoile, de planète ou de lune (`test_cmd.rs`)
//    /stats [n|tout] : statistiques de tous les astres d'une galaxie (`stats.rs`), F3 : masquer
//    /aide           : la liste des commandes
// ─────────────────────────────────────────────────────────────────────────

use bevy::prelude::*;

use crate::galaxy_shape::GalaxyKind;
use crate::net::Net;
use crate::planet::{StarId, StarRoot};
use crate::planetgen::cache::{profile_of, ProfileCache};
use crate::planetgen::live::BodyId;
use crate::planetgen::seed_code;
use crate::settings::{data_dir, GameSettings};
use crate::ui::{CameraTarget, TargetKind};
use crate::wormhole::WormholeTravel;
use crate::CameraController;

pub struct ChatCmdPlugin;

impl Plugin for ChatCmdPlugin {
    fn build(&self, app: &mut App) {
        app.add_event::<ChatCommand>().add_systems(Update, run_chat_commands);
    }
}

/// Une ligne tapée dans le chat qui reste sur cette machine.
#[derive(Event)]
pub struct ChatCommand(pub String);

const COMMANDS: [&str; 37] = ["/tunnel", "/meteo", "/brouillard", "/vent", "/bouclier", "/nuage", "/volume", "/mer", "/jour", "/nuit", "/vol", "/essai", "/geologie", "/relief", "/echelle", "/impact", "/ceinture", "/comete", "/eclipse", "/editeur", "/grotte", "/surplomb", "/tp", "/galaxie", "/profil", "/profile", "/graine", "/seed", "/aller", "/go", "/stats", "/aide", "/help", "/heure", "/time", "/temps", "/speed"];

/// La ligne est une commande du jeu (et non un message à envoyer).
pub fn is_local(line: &str) -> bool {
    let word = line.trim_start().split_whitespace().next().unwrap_or("");
    COMMANDS.contains(&word.to_lowercase().as_str())
}

/// Minuscules sans accents, pour comparer « Spirale barrée » et « spirale barree ».
fn plain(s: &str) -> String {
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

/// Galaxie visée par l'argument de `/tp` : un numéro, « maison », ou un type de galaxie.
fn find_galaxy(arg: &str, settings: &GameSettings, current: usize) -> Result<usize, String> {
    let count = settings.galaxies.len();
    let arg = plain(arg.trim());
    if let Ok(n) = arg.parse::<usize>() {
        return if n < count {
            Ok(n)
        } else {
            Err(format!("Il n'y a pas de galaxie {n} : choisissez de 0 a {}.", count - 1))
        };
    }
    if matches!(arg.as_str(), "maison" | "home" | "nous" | "ici") {
        return Ok(0);
    }
    let matches: Vec<usize> = settings
        .galaxies
        .iter()
        .enumerate()
        .filter(|(_, g)| plain(g.kind.name()).contains(&arg))
        .map(|(i, _)| i)
        .collect();
    // Première galaxie de ce type, autre que celle où l'on se trouve
    matches
        .iter()
        .copied()
        .find(|&i| i != current)
        .or_else(|| matches.first().copied())
        .ok_or_else(|| format!("Aucune galaxie ne correspond a \"{arg}\". Essayez /tp liste."))
}

// ─────────────────────────────────────────────────────────────────────────
//  Aide, propositions (Tab) et commande la plus proche
// ─────────────────────────────────────────────────────────────────────────

/// Commandes : (nom, arguments, description). Ordre d'affichage des propositions.
pub const COMMAND_HELP: [(&str, &str, &str); 31] = [
    ("/tunnel", "[creuser <1|2|3|c> [voies] | liste | entrer [n] | sortir]", "tunnels du sub-espace : 1 cellule de carburant = 1 u, foreuses I 100 u, II 500 u, III 5000 u, c clandestine 250 u"),
    ("/aide", "[commande]", "la liste des commandes, ou l'aide d'une commande"),
    ("/aller", "etoile|planete|lune <type> | suivant", "tests : aller a un type d'etoile, de planete ou de lune"),
    ("/stats", "[n | tout]", "statistiques de tous les astres d'une galaxie (F3 : masquer)"),
    ("/heure", "[0-23.9]", "heure locale, hauteur du soleil et saison de l'astre ou l'on est (ou cible) ; avec un nombre : regle l'heure (hote, tests)"),
    ("/jour", "", "tests : midi sur l'astre cible (hote ; inutile en rotation synchrone)"),
    ("/nuit", "", "tests : minuit sur l'astre cible (hote)"),
    ("/vol", "", "tests : descendre en vol bas sur l'astre cible (comme la molette)"),
    ("/essai", "mer", "tests : va a une planete avec mer, de jour, en vol bas, au-dessus de la mer (enchaine /aller, /jour, /vol, /mer)"),
    ("/temps", "<facteur>", "tests : accelerer le temps (1 = normal, 60 = une heure de la planete par seconde)"),
    ("/surplomb", "", "tests : aller a l'arche de test en voxels 3D de l'astre (pose ou en vol bas)"),
    ("/echelle", "<k : 1, 8, 16, 32, 64>", "tests (0.13 E1) : voxels k fois plus petits au prochain atterrissage"),
    ("/grotte", "", "aller a l'entree de grotte la plus proche (pose ou en vol bas)"),
    ("/mer", "", "aller au rivage de la mer la plus proche (pose ou en vol bas)"),
    ("/volume", "[0-100]", "volume du son (0 = muet)"),
    ("/meteo", "[clair | auto]", "tests : ciel clair (ni nuages, ni pluie, ni brouillard) ou la meteo du monde"),
    ("/brouillard", "[oui | non | auto | 0-100]", "brouillard volumetrique : option, ou densite forcee en % (tests)"),
    ("/vent", "[oui | non]", "stabilisateur de vent : le vent ne deplace plus le vaisseau en vol (actif par defaut)"),
    ("/bouclier", "[oui | non]", "bouclier thermique (option) : trop vite trop bas a la rentree = surchauffe puis degats (desactive par defaut)"),
    ("/nuage", "[dedans | dessus | dessous]", "tests : va dans la couche de nuages (en vol bas) pour traverser, ou au-dessus (mer de nuages)"),
    ("/geologie", "[geyser | fumerolle | cryovolcan | lave | seisme]", "aller a l'evenement geologique le plus proche (pose ou en vol bas) ; seisme = tests"),
    ("/relief", "[corniche | grotte | strates | piton | blocs | gorge | pont | arche | cheminee]", "aller a la forme du relief la plus proche (pose ou en vol bas)"),
    ("/ceinture", "", "aller au champ dense d'une ceinture d'asteroides du systeme charge"),
    ("/comete", "", "aller a la comete la plus active du systeme charge"),
    ("/eclipse", "[lune]", "aller a la prochaine eclipse de soleil (ou de lune) du systeme charge"),
    ("/editeur", "[vaisseau | autre]", "ouvrir l'editeur de modeles (personnage par defaut)"),
    ("/impact", "", "tests : une meteorite s'ecrase tout pres (cratere sauve et partage)"),
    ("/tp", "<n | type de galaxie | liste>", "aller au trou noir d'une galaxie (0 = la notre)"),
    ("/profil", "", "exporter en JSON le profil de l'astre cible"),
    ("/graine", "[code]", "le code court du monde, ou la graine d'un code"),
    ("/g", "<message>", "message a votre guilde"),
];

/// Ce que l'on peut taper après une commande, selon la position de l'argument.
fn arguments(command: &str, previous: &[&str], galaxy_kinds: &[String], galaxies: usize) -> Vec<String> {
    // (10 000 galaxies : seulement les 100 premières numéros proposés)
    let numbers = || (0..galaxies.min(100)).map(|n| n.to_string());
    match (command, previous.len()) {
        ("/aller" | "/go", 0) => ["etoile", "planete", "lune", "suivant"].map(String::from).to_vec(),
        ("/aller" | "/go", 1) => {
            let list = match previous[0] {
                "etoile" => crate::test_cmd::STAR_TYPES,
                "planete" => crate::test_cmd::PLANET_TYPES,
                "lune" => crate::test_cmd::MOON_TYPES,
                _ => "",
            };
            list.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect()
        }
        ("/stats", 0) => std::iter::once("tout".to_string()).chain(numbers()).collect(),
        ("/temps" | "/speed", 0) => ["1", "10", "60", "600", "3600"].map(String::from).to_vec(),
        ("/tp" | "/galaxie", _) => ["liste", "maison"].map(String::from).into_iter().chain(galaxy_kinds.iter().cloned()).chain(numbers()).collect(),
        ("/aide" | "/help", 0) => COMMAND_HELP.iter().map(|(n, _, _)| n.trim_start_matches('/').to_string()).collect(),
        ("/tunnel", 0) => ["creuser", "liste", "entrer", "sortir"].map(String::from).to_vec(),
        ("/tunnel", 1) if previous[0] == "creuser" => ["1", "2", "3", "c"].map(String::from).to_vec(),
        ("/tunnel", 2) if previous[0] == "creuser" => ["1", "2", "3"].map(String::from).to_vec(),
        ("/tunnel", _) => Vec::new(),
        // Les autres : les choix écrits dans l'aide (« [clair | auto] », « <oui | non> »), pour le premier mot
        (_, 0) => help_choices(command),
        _ => Vec::new(),
    }
}

/// Choix du premier argument lus dans `COMMAND_HELP` : « [geyser | fumerolle] » -> geyser, fumerolle (pas les « <n> »).
fn help_choices(command: &str) -> Vec<String> {
    let Some((_, args, _)) = COMMAND_HELP.iter().find(|(n, _, _)| *n == command) else { return Vec::new() };
    let first = args.split(['[', ']', '<', '>']).map(str::trim).find(|s| !s.is_empty()).unwrap_or("");
    first
        .split('|')
        .map(|w| w.trim())
        .filter(|w| !w.is_empty() && !w.contains(' ') && !w.starts_with(|c: char| c.is_ascii_digit()) && w.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '%'))
        .filter(|w| !matches!(*w, "n" | "k" | "code" | "message" | "facteur"))
        .map(String::from)
        .collect()
}

/// Lignes complètes proposées pour la ligne en cours de saisie (Tab).
pub fn suggestions(line: &str, galaxy_kinds: &[String], galaxies: usize) -> Vec<String> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('/') {
        return Vec::new();
    }
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    let typing_command = words.len() <= 1 && !trimmed.ends_with(' ');
    if typing_command {
        let start = plain(words.first().copied().unwrap_or("/"));
        return COMMAND_HELP.iter().map(|(n, _, _)| n.to_string()).filter(|n| n.starts_with(&start)).collect();
    }
    let command = plain(words[0]);
    // /tp : le reste de la ligne est un seul argument (« spirale barree »)
    if command == "/tp" || command == "/galaxie" {
        let partial = plain(trimmed[words[0].len()..].trim_start());
        return arguments(&command, &[], galaxy_kinds, galaxies)
            .into_iter()
            .filter(|a| plain(a).starts_with(&partial))
            .map(|a| format!("{command} {a}"))
            .collect();
    }
    let (done, partial) = if trimmed.ends_with(' ') { (&words[1..], String::new()) } else { (&words[1..words.len() - 1], plain(words[words.len() - 1])) };
    let done_plain: Vec<String> = done.iter().map(|w| plain(w)).collect();
    let done_refs: Vec<&str> = done_plain.iter().map(String::as_str).collect();
    let prefix = std::iter::once(command.clone()).chain(done_plain.iter().cloned()).collect::<Vec<_>>().join(" ");
    arguments(&command, &done_refs, galaxy_kinds, galaxies)
        .into_iter()
        .filter(|a| a.starts_with(&partial))
        .map(|a| format!("{prefix} {a}"))
        .collect()
}

/// Aide de la commande en cours de saisie : « /stats [n | tout] : statistiques... ».
pub fn usage(line: &str) -> Option<String> {
    let word = plain(line.trim_start().split_whitespace().next()?);
    let word = match word.as_str() {
        "/go" => "/aller".to_string(),
        "/time" => "/heure".to_string(),
        "/speed" => "/temps".to_string(),
        "/help" => "/aide".to_string(),
        "/galaxie" => "/tp".to_string(),
        "/seed" => "/graine".to_string(),
        "/profile" => "/profil".to_string(),
        _ => word,
    };
    COMMAND_HELP.iter().find(|(n, _, _)| *n == word).map(|(n, a, d)| if a.is_empty() { format!("{n} : {d}") } else { format!("{n} {a} : {d}") })
}

/// Distance d'édition (fautes de frappe).
fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut prev = row[0];
        row[0] = i;
        for j in 1..=b.len() {
            let tmp = row[j];
            row[j] = (row[j] + 1).min(row[j - 1] + 1).min(prev + usize::from(a[i - 1] != b[j - 1]));
            prev = tmp;
        }
    }
    row[b.len()]
}

/// Commande la plus proche d'un mot mal tapé (« /stat » → « /stats »).
pub fn closest_command(word: &str) -> Option<&'static str> {
    let word = plain(word);
    COMMAND_HELP
        .iter()
        .map(|(n, _, _)| (*n, edit_distance(&word, n)))
        .filter(|(_, d)| *d <= 2)
        .min_by_key(|(_, d)| *d)
        .map(|(n, _)| n)
}

fn help() -> String {
    let list: Vec<String> = COMMAND_HELP.iter().map(|(n, a, _)| if a.is_empty() { n.to_string() } else { format!("{n} {a}") }).collect();
    format!("Commandes : {}. Tab : completer, fleches : messages precedents, /aide <commande> : details.", list.join(" ; "))
}

fn list_kinds(settings: &GameSettings) -> String {
    let mut parts = Vec::new();
    for kind in GalaxyKind::ALL {
        let ids: Vec<String> = settings
            .galaxies
            .iter()
            .enumerate()
            .filter(|(_, g)| g.kind == kind)
            .take(4)
            .map(|(i, _)| i.to_string())
            .collect();
        if !ids.is_empty() {
            parts.push(format!("{} ({})", kind.name(), ids.join(", ")));
        }
    }
    format!("Galaxies 0 a {} : {}", settings.galaxies.len() - 1, parts.join(" - "))
}

/// Astre du monde visé par la caméra. `star_loaded` : l'étoile ciblée est celle du système chargé
/// (id = système × 1000 + n) et non une étoile lointaine (id = indice du système).
pub(crate) fn target_body(kind: &TargetKind, star_loaded: bool) -> Option<BodyId> {
    Some(match *kind {
        TargetKind::Planet(id) => BodyId::Planet { system: (id / 1000) as u32, index: (id % 1000) as u16 },
        TargetKind::Moon(id, m) => BodyId::Moon { system: (id / 1000) as u32, planet: (id % 1000) as u16, index: m as u16 },
        TargetKind::Star(id) if star_loaded => BodyId::Star { system: (id / 1000) as u32, index: (id % 1000) as u16 },
        TargetKind::Star(id) => BodyId::Star { system: id as u32, index: 0 },
        _ => return None,
    })
}

fn copy_to_clipboard(text: &str) -> bool {
    arboard::Clipboard::new().and_then(|mut c| c.set_text(text.to_string())).is_ok()
}

/// Écrit le profil JSON de l'astre ciblé dans `saves/vX.Y.Z/profils/` et le copie.
fn export_profile(settings: &GameSettings, cache: &ProfileCache, kind: &TargetKind, star_loaded: bool) -> String {
    let Some(id) = target_body(kind, star_loaded) else {
        return "Ciblez une etoile, une planete ou une lune, puis tapez /profil.".into();
    };
    let Some(profile) = profile_of(settings, cache, id) else {
        return "Cet astre n'existe pas (ou plus).".into();
    };
    let name = profile.name().to_string();
    let Ok(json) = serde_json::to_string_pretty(&profile) else { return "Export impossible.".into() };
    let dir = data_dir().join("profils");
    let path = dir.join(format!("{}.json", id.key()));
    let written = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, &json)).is_ok();
    let copied = copy_to_clipboard(&json);
    match (written, copied) {
        (true, true) => format!("Profil de {name} exporte : {} (copie dans le presse-papiers).", path.display()),
        (true, false) => format!("Profil de {name} exporte : {}.", path.display()),
        (false, true) => format!("Profil de {name} copie dans le presse-papiers (ecriture du fichier impossible)."),
        (false, false) => "Export impossible : ni fichier ni presse-papiers.".into(),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_chat_commands(
    time: Res<Time>,
    mut events: EventReader<ChatCommand>,
    mut settings: ResMut<GameSettings>,
    travel: Res<WormholeTravel>,
    mut target: ResMut<CameraTarget>,
    mut cam_q: Query<&mut CameraController>,
    mut net: ResMut<Net>,
    mut go: EventWriter<crate::test_cmd::GoCommand>,
    mut stats: EventWriter<crate::stats::StatsCommand>,
    mut clock: EventWriter<crate::world_clock::ClockCommand>,
    mut overhang: EventWriter<crate::surface::OverhangCommand>,
    mut cave: EventWriter<crate::surface::CaveCommand>,
    mut impact: EventWriter<crate::meteors::ImpactCommand>,
    mut small: (EventWriter<crate::asteroids::BeltCommand>, EventWriter<crate::asteroids::CometCommand>, EventWriter<crate::sky::EclipseCommand>, EventWriter<crate::editeur::OpenEditor>, EventWriter<crate::surface::SeaCommand>, EventWriter<crate::test_cmd::DescendCommand>, EventWriter<crate::approche_fx::CloudCommand>, EventWriter<crate::fog::FogCommand>, EventWriter<crate::weather::WeatherCommand>, EventWriter<crate::tunnel::TunnelCommand>),
    profiles: Res<ProfileCache>,
    star_q: Query<&StarId, With<StarRoot>>,
) {
    let now = time.elapsed_secs_f64();
    for ChatCommand(line) in events.read() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if !line.starts_with('/') {
            net.notify("Le chat demande le multijoueur : ouvrez le panneau F2. Tapez /aide pour les commandes.", now);
            continue;
        }
        let mut words = line.splitn(2, char::is_whitespace);
        let command = words.next().unwrap_or("").to_lowercase();
        let arg = words.next().unwrap_or("").trim();
        match command.as_str() {
            "/aide" | "/help" if !arg.is_empty() => match usage(&format!("/{}", arg.trim_start_matches('/'))) {
                Some(u) => net.notify(&u, now),
                None => net.notify(&format!("Pas d'aide pour \"{arg}\". Tapez /aide."), now),
            },
            "/aide" | "/help" => net.notify(&help(), now),
            // Tests : aller à un type d'étoile, de planète ou de lune (`test_cmd.rs`)
            "/aller" | "/go" => {
                go.send(crate::test_cmd::GoCommand(arg.to_string()));
            }
            // Statistiques de tous les astres d'une galaxie (`stats.rs`)
            "/stats" => {
                stats.send(crate::stats::StatsCommand(arg.to_string()));
            }
            // Horloge du monde (`world_clock.rs`)
            // Étude d'échelle (0.13 E1) : voxel k fois plus petit, au prochain atterrissage
            "/echelle" => {
                match arg.trim().parse::<u32>() {
                    Ok(k) if (1..=64).contains(&k) => {
                        crate::terrain::set_voxel_scale(k);
                        net.notify(&format!("Echelle x{k} : voxels {k} fois plus petits au prochain atterrissage (non sauvegarde)."), now);
                    }
                    _ => net.notify(&format!("/echelle <k> de 1 a 64 (actuelle : x{}).", crate::terrain::voxel_scale()), now),
                }
            }
            "/heure" | "/time" => {
                match arg.trim().replace(',', ".").replace(['h', 'H'], ".").trim_end_matches('.').parse::<f32>() {
                    Ok(h) if (0.0..24.0).contains(&h) => {
                        clock.send(crate::world_clock::ClockCommand::SetHour(h));
                    }
                    _ if arg.trim().is_empty() => {
                        clock.send(crate::world_clock::ClockCommand::Hour);
                    }
                    _ => net.notify("/heure [0-23.9] : sans argument, l'heure locale ; avec, regle l'heure (hote).", now),
                }
            }
            "/jour" => {
                clock.send(crate::world_clock::ClockCommand::SetHour(12.0));
            }
            "/nuit" => {
                clock.send(crate::world_clock::ClockCommand::SetHour(0.0));
            }
            "/vol" => {
                small.5.send(crate::test_cmd::DescendCommand);
            }
            "/essai" => {
                go.send(crate::test_cmd::GoCommand(format!("essai {arg}")));
            }
            "/temps" | "/speed" => {
                clock.send(crate::world_clock::ClockCommand::Speed(arg.to_string()));
            }
            // Voxels 3D : l'arche de test (`terrain.rs`)
            "/surplomb" => {
                overhang.send(crate::surface::OverhangCommand(None));
            }
            "/relief" => {
                overhang.send(crate::surface::OverhangCommand(Some(arg.to_string())));
            }
            "/geologie" => {
                overhang.send(crate::surface::OverhangCommand(Some(format!("geo:{arg}"))));
            }
            "/grotte" => {
                cave.send(crate::surface::CaveCommand);
            }
            "/meteo" => {
                small.8.send(crate::weather::WeatherCommand(arg.to_string()));
            }
            "/tunnel" => {
                small.9.send(crate::tunnel::TunnelCommand(arg.to_string()));
            }
            "/brouillard" => {
                small.7.send(crate::fog::FogCommand(arg.to_string()));
            }
            "/vent" => {
                let a = arg.trim().to_lowercase();
                settings.wind_stabilizer = match a.as_str() {
                    "oui" | "on" | "1" | "actif" => true,
                    "non" | "off" | "0" => false,
                    _ => !settings.wind_stabilizer,
                };
                settings.save();
                net.notify(if settings.wind_stabilizer { "Stabilisateur de vent actif : le vent ne vous deplace plus." } else { "Stabilisateur de vent coupe : le vent pousse le vaisseau." }, now);
            }
            "/bouclier" => {
                let a = arg.trim().to_lowercase();
                settings.heat_shield = match a.as_str() {
                    "oui" | "on" | "1" | "actif" => true,
                    "non" | "off" | "0" => false,
                    _ => !settings.heat_shield,
                };
                settings.save();
                net.notify(if settings.heat_shield { "Bouclier thermique actif : trop vite trop bas = surchauffe, puis degats." } else { "Bouclier thermique desactive : la rentree ne fait aucun degat." }, now);
            }
            "/nuage" => {
                small.6.send(crate::approche_fx::CloudCommand(arg.to_string()));
            }
            "/volume" => match arg.trim().trim_end_matches('%').parse::<f32>() {
                Ok(v) if (0.0..=100.0).contains(&v) => {
                    settings.sound_volume = v / 100.0;
                    settings.save();
                    net.notify(&format!("Volume du son : {v:.0} %."), now);
                }
                _ => net.notify(&format!("Volume du son : {:.0} %. /volume <0-100> pour le changer.", settings.sound_volume * 100.0), now),
            },
            "/mer" => {
                small.4.send(crate::surface::SeaCommand);
            }
            "/impact" => {
                impact.send(crate::meteors::ImpactCommand);
            }
            // Ceintures d'astéroïdes (`asteroids.rs`)
            "/ceinture" => {
                small.0.send(crate::asteroids::BeltCommand);
            }
            "/comete" => {
                small.1.send(crate::asteroids::CometCommand);
            }
            // Phénomènes du ciel (`sky.rs`)
            "/eclipse" => {
                small.2.send(crate::sky::EclipseCommand(arg.to_string()));
            }
            // Éditeur de modèles (0.12)
            "/editeur" => {
                use crate::editeur::format::ModelKind;
                let kind = match arg {
                    a if a.starts_with("vaisseau") => ModelKind::Vaisseau,
                    a if a.starts_with("autre") || a.starts_with("objet") => ModelKind::Autre,
                    _ => ModelKind::Personnage,
                };
                small.3.send(crate::editeur::OpenEditor(kind));
            }
            "/profil" | "/profile" => {
                let loaded = matches!(target.0, TargetKind::Star(id) if star_q.iter().any(|s| s.0 == id));
                net.notify(&export_profile(&settings, &profiles, &target.0, loaded), now);
            }
            "/graine" | "/seed" if !arg.is_empty() => match seed_code::decode(arg) {
                Some(seed) => net.notify(&format!("Le code {} est celui de la graine {seed}.", seed_code::encode(seed)), now),
                None => net.notify("Code de graine invalide (ex. K7Q2-M9XA).", now),
            },
            "/graine" | "/seed" => {
                let code = seed_code::encode(settings.world_seed);
                let copied = if copy_to_clipboard(&code) { " (copie dans le presse-papiers)" } else { "" };
                net.notify(&format!("Graine du monde : {code}{copied}."), now);
            }
            "/tp" | "/galaxie" => {
                if arg.is_empty() {
                    net.notify(&help(), now);
                    continue;
                }
                if plain(arg) == "liste" || plain(arg) == "list" {
                    net.notify(&list_kinds(&settings), now);
                    continue;
                }
                if travel.active() {
                    net.notify("Impossible de se teleporter pendant un voyage en trou de ver.", now);
                    continue;
                }
                let current = match target.0 {
                    TargetKind::DistantGalaxyCore(id) => id as usize,
                    _ => 0,
                };
                match find_galaxy(arg, &settings, current) {
                    Ok(id) => {
                        // Même cible que le clic sur le trou noir : le vaisseau saute jusque-là
                        target.0 = if id == 0 { TargetKind::GalacticCore } else { TargetKind::DistantGalaxyCore(id as u32) };
                        if let Ok(mut ctrl) = cam_q.get_single_mut() {
                            ctrl.zoom_goal = Some(crate::galaxy_view_distance(&target.0, &settings));
                        }
                        net.local.siege = None;
                        let what = settings.galaxies.get(id).map_or("", |g| g.kind.name());
                        net.notify(&format!("Teleportation au trou noir de la galaxie {id} ({what})."), now);
                    }
                    Err(e) => net.notify(&e, now),
                }
            }
            _ => match closest_command(&command) {
                Some(guess) => net.notify(&format!("Commande inconnue. Vouliez-vous dire {guess} ? (Tab : propositions, /aide)"), now),
                None => net.notify("Commande inconnue. Tapez /aide (Tab : propositions).", now),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_game_commands_stay_local() {
        assert!(is_local("/tp 3"));
        assert!(is_local("  /TP annulaire"));
        assert!(is_local("/aide"));
        assert!(is_local("/profil"));
        assert!(is_local("/graine"));
        assert!(is_local("/aller planete lave"));
        assert!(is_local("/stats tout"));
        assert!(!is_local("/g salut"));
        assert!(!is_local("bonjour /tp"));
    }

    #[test]
    fn tab_completes_commands_and_arguments() {
        let kinds = vec!["spirale barree".to_string(), "annulaire".to_string()];
        assert_eq!(suggestions("/st", &kinds, 21), vec!["/stats"]);
        assert!(suggestions("/", &kinds, 21).len() >= 6);
        assert_eq!(suggestions("/aller pl", &kinds, 21), vec!["/aller planete"]);
        assert_eq!(suggestions("/aller etoile gea", &kinds, 21), vec!["/aller etoile geante"]);
        assert!(suggestions("/aller planete ", &kinds, 21).contains(&"/aller planete lave".to_string()));
        assert_eq!(suggestions("/tp spi", &kinds, 21), vec!["/tp spirale barree"]);
        assert!(suggestions("/stats ", &kinds, 21).contains(&"/stats tout".to_string()));
        assert!(suggestions("bonjour", &kinds, 21).is_empty());
        assert_eq!(suggestions("/tunnel cr", &kinds, 21), vec!["/tunnel creuser"]);
        assert!(suggestions("/tunnel creuser ", &kinds, 21).contains(&"/tunnel creuser c".to_string()));
        assert_eq!(suggestions("/meteo c", &kinds, 21), vec!["/meteo clair"]);
        assert!(suggestions("/volume ", &kinds, 21).is_empty());
        assert!(usage("/stats 3").unwrap().starts_with("/stats [n | tout]"));
        assert!(usage("/go").unwrap().starts_with("/aller"));
        assert_eq!(closest_command("/stat"), Some("/stats"));
        assert_eq!(closest_command("/allr"), Some("/aller"));
        assert_eq!(closest_command("/zzzzzz"), None);
    }

    #[test]
    fn targets_become_world_bodies() {
        assert_eq!(target_body(&TargetKind::Planet(12_002), false), Some(BodyId::Planet { system: 12, index: 2 }));
        assert_eq!(target_body(&TargetKind::Moon(5_001, 1), false), Some(BodyId::Moon { system: 5, planet: 1, index: 1 }));
        assert_eq!(target_body(&TargetKind::Star(7_000), true), Some(BodyId::Star { system: 7, index: 0 }));
        assert_eq!(target_body(&TargetKind::Star(7_000), false), Some(BodyId::Star { system: 7_000, index: 0 }));
        assert_eq!(target_body(&TargetKind::GalacticCore, false), None);
    }

    #[test]
    fn every_target_kind_exports_a_profile() {
        let settings = GameSettings::default();
        // Un système dont la première planète a une lune
        let si = (0..).find(|&s| !settings.systems[s].planets()[0].moons.is_empty()).unwrap();
        let s = si as u32;
        let cache = ProfileCache::build(&settings, si);
        for id in [
            BodyId::Star { system: s, index: 0 },
            BodyId::Planet { system: s, index: 0 },
            BodyId::Moon { system: s, planet: 0, index: 0 },
            // Hors du système chargé : calculé à la demande
            BodyId::Planet { system: 4_000, index: 0 },
        ] {
            let profile = profile_of(&settings, &cache, id).unwrap_or_else(|| panic!("{}", id.key()));
            assert_eq!(profile.id(), id.key());
            // Les sections gardent leur ordre : identité d'abord
            let json = serde_json::to_string_pretty(&profile).unwrap();
            assert!(json.trim_start().starts_with("{
  \"id\""), "{json}");
        }
        assert!(profile_of(&settings, &cache, BodyId::Planet { system: s, index: 999 }).is_none());
        assert!(profile_of(&settings, &cache, BodyId::Planet { system: u32::MAX, index: 0 }).is_none());
    }

    #[test]
    fn galaxies_are_found_by_number_home_or_type() {
        let settings = GameSettings::default();
        assert_eq!(find_galaxy("0", &settings, 5), Ok(0));
        assert_eq!(find_galaxy("maison", &settings, 5), Ok(0));
        assert_eq!(find_galaxy("9999", &settings, 0), Ok(9999));
        assert!(find_galaxy("10000", &settings, 0).is_err());
        assert!(find_galaxy("zzz", &settings, 0).is_err());
        let id = find_galaxy("spirale barree", &settings, 0).unwrap();
        assert_eq!(settings.galaxies[id].kind, GalaxyKind::Barred);
        // Sans accent ou avec : pareil
        assert_eq!(find_galaxy("Spirale barrée", &settings, 0), Ok(id));
    }
}
