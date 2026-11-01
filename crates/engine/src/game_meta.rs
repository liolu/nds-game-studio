//! Ce que la rom ne dit pas (développeur, année, genres), lu dans la base de GameTDB (`dstdb.zip`). On n'en garde que
//! `game_meta.json` dans le dossier des données, redemandé après 30 jours. Suit l'option des jaquettes : rien n'est
//! téléchargé tant qu'elle est coupée, et ce qui est déjà là reste lu.

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::i18n::{self, Lang};

const URL: &str = "https://www.gametdb.com/dstdb.zip";
const TIMEOUT_SECS: u64 = 120;
const MAX_ARCHIVE: u64 = 64 * 1024 * 1024;
const MAX_XML: u64 = 256 * 1024 * 1024;
const REFRESH_EVERY: Duration = Duration::from_secs(30 * 24 * 3600);
/// Année de sortie de la DS : la base la met aussi par défaut (un sixième de ses dates), elle ne dit donc rien.
const PLACEHOLDER_YEAR: u16 = 2004;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GameMeta {
    #[serde(default)]
    pub developer: String,
    #[serde(default)]
    pub publisher: String,
    #[serde(default)]
    pub year: Option<u16>,
    #[serde(default)]
    pub genres: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MetaDb {
    games: BTreeMap<String, GameMeta>,
    /// Genre anglais -> français.
    genres_fr: HashMap<String, String>,
}

fn unescape(text: &str) -> String {
    text.trim().replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// Le texte de `<name>texte</name>` (la base écrit `<name/>` quand il est vide).
fn element<'a>(block: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let start = block.find(&open)? + open.len();
    let end = block[start..].find("</")?;
    Some(&block[start..start + end])
}

fn attribute<'a>(block: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("{name}=\"");
    let start = block.find(&open)? + open.len();
    let end = block[start..].find('"')?;
    Some(&block[start..start + end])
}

fn parse(xml: &str) -> MetaDb {
    let mut db = MetaDb::default();
    // Chaque genre : <maingenre name="x"> ou <subgenre name="x">, puis ses <loc lang="..">
    if let Some(end) = xml.find("</genres>") {
        for block in xml[..end].split("genre name=\"").skip(1) {
            let Some((name, rest)) = block.split_once('"') else { continue };
            // S'arrête au genre suivant : un genre sans français ne prend pas celui d'un autre
            if let Some(french) = rest.split_once("<loc lang=\"FR\">").and_then(|(_, r)| r.split_once("</loc>")) {
                db.genres_fr.insert(name.to_string(), unescape(french.0));
            }
        }
    }
    for block in xml.split("<game name=\"").skip(1) {
        let Some(code) = element(block, "id") else { continue };
        let genres = element(block, "genre").map_or_else(Vec::new, |g| g.split(',').map(unescape).filter(|g| !g.is_empty()).collect());
        let meta = GameMeta {
            developer: element(block, "developer").map(unescape).unwrap_or_default(),
            publisher: element(block, "publisher").map(unescape).unwrap_or_default(),
            year: block.find("<date ").and_then(|at| attribute(&block[at..], "year")).and_then(|y| y.parse().ok()),
            genres,
        };
        db.games.insert(code.to_string(), meta);
    }
    db
}

impl MetaDb {
    /// Ce qu'on sait de ce jeu : sa région d'abord, puis les autres versions du même jeu (mêmes 3 premières lettres)
    /// pour ce qui manque. L'année est la première de toutes les versions : une réédition n'est pas la sortie du jeu.
    pub fn lookup(&self, code: &str) -> GameMeta {
        let mut meta = GameMeta::default();
        let prefix: String = code.chars().take(3).collect();
        if prefix.is_empty() {
            return meta;
        }
        let others = self.games.range(prefix.clone()..).take_while(|(c, _)| c.starts_with(&prefix)).filter(|(c, _)| *c != code).map(|(_, m)| m);
        for found in self.games.get(code).into_iter().chain(others) {
            if meta.developer.is_empty() {
                meta.developer.clone_from(&found.developer);
            }
            if meta.publisher.is_empty() {
                meta.publisher.clone_from(&found.publisher);
            }
            if let Some(year) = found.year.filter(|y| *y != PLACEHOLDER_YEAR) {
                meta.year = Some(meta.year.map_or(year, |known| known.min(year)));
            }
            if meta.genres.is_empty() {
                meta.genres.clone_from(&found.genres);
            }
        }
        meta
    }

    /// Les genres d'un jeu dans la langue choisie.
    pub fn genre_names(&self, meta: &GameMeta) -> Vec<String> {
        meta.genres.iter().map(|genre| if i18n::lang() == Lang::Fr { self.genres_fr.get(genre).unwrap_or(genre).clone() } else { genre.clone() }).collect()
    }
}

fn path() -> PathBuf {
    crate::save::data_dir().join("game_meta.json")
}

fn download() -> Result<MetaDb, String> {
    let tls = ureq::tls::TlsConfig::builder().provider(ureq::tls::TlsProvider::NativeTls).build();
    let config = ureq::Agent::config_builder().tls_config(tls).timeout_global(Some(Duration::from_secs(TIMEOUT_SECS))).build();
    let agent = ureq::Agent::new_with_config(config);
    let mut response = agent.get(URL).call().map_err(|e| e.to_string())?;
    let archive = response.body_mut().with_config().limit(MAX_ARCHIVE).read_to_vec().map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive)).map_err(|e| e.to_string())?;
    let file = zip.by_name("dstdb.xml").map_err(|e| e.to_string())?;
    let mut xml = String::new();
    file.take(MAX_XML).read_to_string(&mut xml).map_err(|e| e.to_string())?;
    let db = parse(&xml);
    if db.games.is_empty() { Err("dstdb.xml sans jeux".to_string()) } else { Ok(db) }
}

/// Dans un fil à part : la base déjà là, puis une plus récente si `fetch` et qu'elle a vieilli. Le launcher prend chaque
/// résultat quand il arrive.
pub fn start(fetch: bool) -> Arc<Mutex<Option<MetaDb>>> {
    let result: Arc<Mutex<Option<MetaDb>>> = Arc::default();
    let out = result.clone();
    std::thread::spawn(move || {
        let path = path();
        let saved = std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice::<MetaDb>(&bytes).ok());
        let fresh = saved.is_some() && std::fs::metadata(&path).and_then(|m| m.modified()).ok().and_then(|at| SystemTime::now().duration_since(at).ok()).is_some_and(|age| age < REFRESH_EVERY);
        if saved.is_some() {
            *out.lock().unwrap() = saved;
        }
        if !fetch || fresh {
            return;
        }
        match download() {
            Ok(db) => {
                if let Ok(text) = serde_json::to_string(&db) {
                    if let Err(e) = crate::save::write_atomic(&path, &text) {
                        bevy::log::warn!("game_meta.json non enregistre : {e}");
                    }
                }
                bevy::log::info!("infos des jeux : {} jeux lus sur GameTDB", db.games.len());
                *out.lock().unwrap() = Some(db);
            }
            Err(e) => bevy::log::warn!("infos des jeux non telechargees : {e}"),
        }
    });
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<datafile>
	<genres>
		<maingenre name="theme">
			<loc lang="FR">thème</loc>
			<subgenre name="horror">
				<loc lang="FI">kauhu</loc>
			</subgenre>
			<subgenre name="sci-fi">
				<loc lang="FR">science-fiction</loc>
			</subgenre>
		</maingenre>
	</genres>
	<game name="Le jeu (USA)">
		<id>AL7E</id>
		<locale lang="EN"><title>Le jeu</title><synopsis/></locale>
		<developer>Amaze &amp; Co</developer>
		<publisher>LucasArts</publisher>
		<date year="2004" month="" day=""/>
		<genre>action,adventure</genre>
	</game>
	<game name="Le jeu (Europe)">
		<id>AL7P</id>
		<developer/>
		<publisher>Autre</publisher>
		<date year="2006" month="9" day="1"/>
		<genre>puzzle</genre>
	</game>
	<game name="Sans rien">
		<id>ZZZJ</id>
		<developer/>
		<publisher/>
		<date year="" month="" day=""/>
		<genre/>
	</game>
</datafile>"#;

    #[test]
    fn lit_la_base_et_complete_avec_les_autres_regions() {
        let db = parse(XML);
        assert_eq!(db.games.len(), 3);
        let usa = db.lookup("AL7E");
        assert_eq!(usa.developer, "Amaze & Co");
        assert_eq!(usa.publisher, "LucasArts");
        assert_eq!(usa.genres, ["action", "adventure"]);
        assert_eq!(usa.year, Some(2006), "2004 ne dit rien : l'annee vient de l'Europe");
        let europe = db.lookup("AL7P");
        assert_eq!(europe.publisher, "Autre");
        assert_eq!(europe.developer, "Amaze & Co");
        assert_eq!(db.lookup("AL7J").publisher, "LucasArts", "region absente de la base");
        assert_eq!(db.lookup("ZZZJ"), GameMeta::default());
        assert_eq!(db.lookup("QQQE"), GameMeta::default());
    }

    #[test]
    fn un_genre_sans_francais_ne_prend_pas_celui_d_un_autre() {
        let db = parse(XML);
        assert_eq!(db.genres_fr["theme"], "thème");
        assert_eq!(db.genres_fr["sci-fi"], "science-fiction");
        assert!(!db.genres_fr.contains_key("horror"));
    }

    #[test]
    fn le_fichier_garde_tout() {
        let db = parse(XML);
        let text = serde_json::to_string(&db).unwrap();
        assert_eq!(serde_json::from_str::<MetaDb>(&text).unwrap(), db);
    }
}
