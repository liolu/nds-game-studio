//! Paramètres propres à un jeu pour lire ses fichiers.
//!
//! Deux jeux peuvent partager un format et différer sur un détail : ce qui est commun reste dans le code
//! (signature, extension, devinettes), ce qui n'est vrai que pour un jeu s'écrit ici, jamais comme une
//! nouvelle devinette qui changerait les autres jeux. La table `game_params.tsv` est remplie par
//! `--example asset_params`, qui essaie sur chaque jeu tous les décodeurs existants.
//!
//! Une ligne : `code du jeu`, `motif`, `sorte`, `commentaire`. Le motif vaut `*.ext` pour tout le jeu ou
//! `dossier/*.ext` pour un seul dossier (une archive compte comme un dossier) ; le plus précis gagne.
//! La sorte `Other` refuse une devinette fausse. Après la sorte, des options séparées par des espaces disent
//! comment montrer ces fichiers : `palette=dossier/nom.ext` (la palette du jeu à prendre), `bpp=4` ou `bpp=8`,
//! `bank=n` (la banque de palette d'un écran, quelle que soit celle qu'il demande), `width=n` (largeur en tuiles).

use nds_nitro::Kind;

const TABLE: &str = include_str!("game_params.tsv");

/// Les sortes qu'une ligne peut donner, et leur nom dans la table.
pub const KINDS: &[(&str, Kind)] = &[
    ("PaletteTiles", Kind::PaletteTiles),
    ("RomImage", Kind::RomImage),
    ("ZImage", Kind::ZImage),
    ("BsTexture", Kind::BsTexture),
    ("Bitmap", Kind::Bitmap),
    ("AnmFile", Kind::AnmFile),
    ("RawTiles", Kind::RawTiles),
    ("RawPalette", Kind::RawPalette),
    ("RawScreen", Kind::RawScreen),
    ("RawTexture", Kind::RawTexture),
    ("TileFont", Kind::TileFont),
    ("LzSprites", Kind::LzSprites),
    ("Compressed", Kind::Compressed),
    ("PlainText", Kind::PlainText),
    ("Other", Kind::Other),
];

pub fn kind_named(name: &str) -> Option<Kind> {
    KINDS.iter().find(|(n, _)| *n == name).map(|(_, kind)| *kind)
}

pub fn kind_name(kind: Kind) -> Option<&'static str> {
    KINDS.iter().find(|(_, k)| *k == kind).map(|(name, _)| *name)
}

/// Comment montrer les fichiers d'une ligne.
#[derive(Clone, Default, Debug, PartialEq)]
pub struct Options {
    pub palette: Option<String>,
    pub bpp8: Option<bool>,
    pub bank: Option<u16>,
    pub width: Option<u32>,
}

/// La sorte et les options d'une case `sorte` de la table (`RawTiles bpp=4`).
fn parse_kind(field: &str) -> Option<(Kind, Options)> {
    let mut words = field.split_whitespace();
    let kind = kind_named(words.next()?)?;
    let mut options = Options::default();
    for word in words {
        match word.split_once('=')? {
            ("palette", path) => options.palette = Some(path.to_string()),
            ("bpp", "4") => options.bpp8 = Some(false),
            ("bpp", "8") => options.bpp8 = Some(true),
            ("bank", n) => options.bank = Some(n.parse().ok()?),
            ("width", n) => options.width = Some(n.parse().ok()?),
            _ => return None,
        }
    }
    Some((kind, options))
}

/// Les lignes d'un jeu.
#[derive(Default)]
pub struct GameParams {
    rules: Vec<(String, Kind, Options)>,
}

impl GameParams {
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// La sorte écrite pour ce fichier, sinon celle qu'il a déjà. Une ligne ne donne une sorte qu'aux fichiers
    /// sans sorte (`*.bin` ne touche pas un `.bin` reconnu par sa signature) ; `Other` les refuse tous.
    pub fn kind_for(&self, dir: &str, format: &str, kind: Kind) -> Kind {
        match self.rule(dir, format).map(|(k, _)| *k) {
            Some(Kind::Other) => Kind::Other,
            Some(written) if kind == Kind::Other => written,
            _ => kind,
        }
    }

    /// Les options écrites pour ce fichier.
    pub fn options_for(&self, dir: &str, format: &str) -> Options {
        self.rule(dir, format).map(|(_, o)| o.clone()).unwrap_or_default()
    }

    fn rule(&self, dir: &str, format: &str) -> Option<(&Kind, &Options)> {
        let (exact, whole_game) = (format!("{dir}/*.{format}"), format!("*.{format}"));
        let find = |pattern: &str| self.rules.iter().find(|(p, _, _)| p == pattern).map(|(_, k, o)| (k, o));
        find(&exact).or_else(|| find(&whole_game))
    }
}

/// Les paramètres du jeu `code`, lus dans `table`.
fn parse(table: &str, code: &str) -> GameParams {
    let mut rules = Vec::new();
    for line in table.lines().filter(|l| !l.starts_with('#')) {
        let mut parts = line.split('\t');
        if let (Some(c), Some(pattern), Some((kind, options))) = (parts.next(), parts.next(), parts.next().and_then(parse_kind)) {
            if c == code {
                rules.push((pattern.to_string(), kind, options));
            }
        }
    }
    GameParams { rules }
}

pub fn for_game(code: &str) -> GameParams {
    parse(TABLE, code)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# commentaire\nAAAA\t*.bin\tRawTiles\tun jeu\nAAAA\tfile/pal/*.bin\tRawPalette\tun dossier\nBBBB\t*.bin\tOther\tune devinette fausse\nCCCC\t*.dat\tInconnue\tignoree\n";

    #[test]
    fn un_jeu_ne_voit_que_ses_lignes() {
        let params = parse(SAMPLE, "AAAA");
        assert_eq!(params.kind_for("file", "bin", Kind::Other), Kind::RawTiles);
        assert_eq!(parse(SAMPLE, "BBBB").kind_for("file", "bin", Kind::RawTiles), Kind::Other);
        assert_eq!(parse(SAMPLE, "DDDD").kind_for("file", "bin", Kind::Audio), Kind::Audio);
    }

    #[test]
    fn le_dossier_gagne_sur_le_jeu_entier() {
        let params = parse(SAMPLE, "AAAA");
        assert_eq!(params.kind_for("file/pal", "bin", Kind::Other), Kind::RawPalette);
        assert_eq!(params.kind_for("file", "dat", Kind::Audio), Kind::Audio);
    }

    #[test]
    fn un_fichier_reconnu_garde_sa_sorte() {
        assert_eq!(parse(SAMPLE, "AAAA").kind_for("file", "bin", Kind::Palette), Kind::Palette);
    }

    #[test]
    fn une_sorte_inconnue_est_ignoree() {
        assert!(parse(SAMPLE, "CCCC").is_empty());
    }

    #[test]
    fn la_table_du_depot_se_lit() {
        for line in TABLE.lines().filter(|l| !l.starts_with('#') && !l.is_empty()) {
            let parts: Vec<&str> = line.split('\t').collect();
            assert!(parts.len() >= 3 && parse_kind(parts[2]).is_some(), "{line}");
        }
    }

    #[test]
    fn les_options_se_lisent() {
        let params = parse("AAAA	radar/*.icg	RawTiles palette=radar/a.ncl bpp=8 width=8	x
AAAA	*.sc	RawScreen bank=0	x
", "AAAA");
        assert_eq!(params.kind_for("radar", "icg", Kind::Other), Kind::RawTiles);
        assert_eq!(params.options_for("radar", "icg"), Options { palette: Some("radar/a.ncl".into()), bpp8: Some(true), bank: None, width: Some(8) });
        assert_eq!(params.options_for("seq", "sc").bank, Some(0));
        assert!(parse_kind("RawTiles bpp=5").is_none());
    }
}
