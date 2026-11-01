//! L'export et l'import d'un jeu entier : ce qui relie les convertisseurs aux fichiers de la rom.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use nds_nitro::{adx, decompress_any, g2d, gx_mesh, model3d, narc, packs, romarc, sadl, sdat, Kind, Rgba};
use serde::{Deserialize, Serialize};

use crate::asset_scan::{self, Decoded, Entry};
use crate::container::{self, rd32, wr32};
use crate::i18n::t;
use crate::{compress, images, media, model, sound, text};

const FILES: &str = "fichiers";
const PACKS: &str = "paquets";
const PACK_STATE: &str = "paquets.json";
const DROP_INS: &str = "remplacements";
const MANIFEST: &str = "manifest.json";
pub const MODDED_ROM: &str = "jeu-modifie.nds";
const REPORT: &str = "rapport.json";
const README: &str = "LISEZMOI.txt";
/// Un fichier a des milliers de cellules parfois : au-delà, on garde le fichier au format du jeu.
const MAX_CELLS: usize = 2000;
/// Un son plus court reste en WAV seul : une banque peut en avoir des centaines.
const MP3_FROM_SECONDS: f32 = 2.0;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Recipe {
    /// Le fichier tel qu'il est dans le jeu : il revient tel quel.
    Raw,
    Palette { count: usize },
    Sheet { palette: Option<String>, width: u32, bank: usize },
    Screen { tiles: String, palette: Option<String> },
    Cell { tiles: String, palette: Option<String>, index: usize },
    Texture { index: usize, palette: Option<usize> },
    Model { index: usize, textures: Option<String> },
    Sound,
    BankSound { index: usize },
    Messages,
    Texel { palette: Option<String> },
    BsTexture,
    PaletteTiles { width: u32 },
    /// Une vidéo VX, MODS ou Bink, réécrite depuis ce que FFmpeg sait lire.
    Video,
    /// Un `.z` de Tom and Jerry : image, fond ou texture.
    ZImage,
    ZPalette,
    Font,
    Bitmap,
    EngineBitmap { palette: Option<String> },
    LangText,
    /// Un maillage en commandes GX : `GEOM`, `MESH`, ou `.z` du monde.
    GxModel,
    /// Pour regarder ou réutiliser : le chemin du retour n'existe pas encore pour ce format.
    ExportOnly,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Item {
    file: String,
    hash: String,
    source: String,
    recipe: Recipe,
}

#[derive(Serialize, Deserialize, Default)]
struct Manifest {
    code: String,
    items: Vec<Item>,
    /// Tout le jeu a été exporté, et pas un seul fichier à la demande.
    #[serde(default)]
    complete: bool,
}

/// Ce que le launcher montre après un import.
#[derive(Serialize, Default)]
struct Report {
    applied: Vec<String>,
    errors: Vec<String>,
    warnings: Vec<String>,
    export_only: Vec<String>,
    built: bool,
}

/// Empreinte d'un fichier : savoir s'il a changé depuis l'export.
fn fingerprint(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash = (hash ^ *byte as u64).wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn game_code(rom: &[u8]) -> String {
    String::from_utf8_lossy(rom.get(0x0C..0x10).unwrap_or(b"????")).into_owned()
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&temp, path).map_err(|e| e.to_string())
}

fn load_manifest(dir: &Path) -> Manifest {
    std::fs::read(dir.join(MANIFEST)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

/// Un chemin de fichier sûr sous Windows, avec les archives en dossiers.
fn clean_path(path: &str) -> String {
    path.split('/')
        .map(|part| {
            let clean: String = part.chars().map(|c| if c.is_control() || r#"<>:"\|?*"#.contains(c) { '_' } else { c }).collect();
            let clean = clean.trim_end_matches(['.', ' ']).to_string();
            if clean.is_empty() { "_".to_string() } else { clean }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Là où va un fichier du jeu dans `fichiers/`, sans extension ajoutée.
fn base_name(entry: &Entry) -> String {
    let member = entry.path.rsplit_once('#').map(|(_, n)| n);
    match (&entry.member_name, member) {
        (Some(name), _) => clean_path(&format!("{}/{}", entry.dir, name)),
        (None, Some(n)) => clean_path(&format!("{}/{:0>4}.{}", entry.dir, n, romarc::extension(&asset_scan::read_entry(entry).unwrap_or_default()))),
        _ => clean_path(&entry.path),
    }
}

fn relative(from_dir: &str, to: &str) -> String {
    let (from, to): (Vec<&str>, Vec<&str>) = (from_dir.split('/').filter(|p| !p.is_empty()).collect(), to.split('/').collect());
    let shared = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts = vec![".."; from.len() - shared];
    parts.extend(&to[shared..]);
    parts.join("/")
}

fn png_bytes(image: &Rgba) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out).write_image(&image.data, image.width, image.height, image::ExtendedColorType::Rgba8).map_err(|e| e.to_string())?;
    Ok(out)
}

fn read_png(path: &Path) -> Result<Rgba, String> {
    let image = image::open(path).map_err(|e| e.to_string())?.into_rgba8();
    Ok(Rgba { width: image.width(), height: image.height(), data: image.into_raw() })
}

/// Une image à la taille que le jeu attend : une autre taille est mise à l'échelle, et c'est signalé.
fn read_png_sized(path: &Path, width: u32, height: u32, file: &str, warnings: &mut Vec<String>) -> Result<Rgba, String> {
    let picture = read_png(path)?;
    if picture.width == width && picture.height == height {
        return Ok(picture);
    }
    warnings.push(t!("{file}: image of {}x{} resized to {width}x{height}, the size the game expects", "{file} : image de {}x{} mise a {width}x{height}, la taille que le jeu attend", picture.width, picture.height));
    let source = image::RgbaImage::from_raw(picture.width, picture.height, picture.data).ok_or("image illisible")?;
    // En réduisant, un bon filtre ; en agrandissant, des pixels nets
    let filter = if width < source.width() { image::imageops::FilterType::Lanczos3 } else { image::imageops::FilterType::Nearest };
    let resized = image::imageops::resize(&source, width, height, filter);
    Ok(Rgba { width, height, data: resized.into_raw() })
}

/// Le contenu de la rom vu par les recettes : les fichiers d'origine, et ceux qui ont déjà changé pendant cet import.
struct Game {
    entries: Vec<Entry>,
    by_path: HashMap<String, usize>,
    edited: BTreeMap<usize, Vec<u8>>,
}

impl Game {
    fn new(rom: &Path) -> Self {
        let entries = asset_scan::scan(rom);
        let by_path = entries.iter().enumerate().map(|(i, e)| (e.path.clone(), i)).collect();
        Game { entries, by_path, edited: BTreeMap::new() }
    }

    fn at(&self, path: &str) -> Result<usize, String> {
        self.by_path.get(path).copied().ok_or_else(|| format!("{path} : plus dans la rom"))
    }

    fn current(&self, index: usize) -> Result<Vec<u8>, String> {
        match self.edited.get(&index) {
            Some(bytes) => Ok(bytes.clone()),
            None => asset_scan::read_entry(&self.entries[index]),
        }
    }

    /// Certains formats sont compressés sans le dire : ces lecteurs décompressent eux-mêmes.
    fn plain(&self, index: usize) -> Result<Vec<u8>, String> {
        let bytes = self.current(index)?;
        Ok(decompress_any(&bytes).unwrap_or(bytes))
    }

    fn set_plain(&mut self, index: usize, new: Vec<u8>) -> Result<(), String> {
        let original = asset_scan::read_entry(&self.entries[index])?;
        let packed = if decompress_any(&original).is_some() { compress::same_method(&original, &new)? } else { new };
        self.edited.insert(index, packed);
        Ok(())
    }

    /// Un fichier sous toutes ses compressions.
    fn unpacked(&self, index: usize) -> Result<Vec<u8>, String> {
        Ok(asset_scan::fully_unpacked(self.current(index)?))
    }

    /// Reprend toutes les compressions du fichier d'origine.
    fn set_unpacked(&mut self, index: usize, new: &[u8]) -> Result<(), String> {
        let packed = compress::same_layers(&asset_scan::read_entry(&self.entries[index])?, new)?;
        self.edited.insert(index, packed);
        Ok(())
    }

    fn palette(&self, path: Option<&str>, tiles: Option<&g2d::Tiles>) -> Result<g2d::Palette, String> {
        let Some(path) = path else {
            // Comme l'aperçu sans palette : des gris
            let bpp8 = tiles.is_some_and(|t| t.bpp8);
            return Ok(g2d::Palette { colors: (0..256).map(|i| if bpp8 { [i as u8, i as u8, i as u8, 255] } else { let v = (i % 16 * 17) as u8; [v, v, v, 255] }).collect() });
        };
        let index = self.at(path)?;
        let bytes = self.plain(index)?;
        if self.entries[index].kind == Kind::RawPalette { Ok(g2d::raw_palette(&bytes)) } else { g2d::read_palette(&bytes).map_err(|e| e.to_string()) }
    }

    fn tiles(&self, path: &str, palette: Option<&g2d::Palette>) -> Result<g2d::Tiles, String> {
        let index = self.at(path)?;
        let bytes = self.plain(index)?;
        if self.entries[index].kind == Kind::RawTiles { Ok(g2d::raw_tiles(&bytes, palette)) } else { g2d::read_tiles(&bytes).map_err(|e| e.to_string()) }
    }

    fn store_tiles(&mut self, path: &str, tiles: &g2d::Tiles) -> Result<(), String> {
        let index = self.at(path)?;
        let bytes = self.plain(index)?;
        let out = images::write_tiles(&bytes, tiles)?;
        self.set_plain(index, out)
    }
}

fn path_of(game_entries: &[Entry], list: &[usize], at: usize) -> Option<String> {
    list.get(at).map(|&i| game_entries[i].path.clone())
}

/// La palette d'une image ` PMB`, comme l'aperçu : sans palette, des gris sur 256 niveaux.
fn bitmap_palette(game: &Game, path: Option<&str>) -> Result<g2d::Palette, String> {
    match path {
        Some(_) => game.palette(path, None),
        None => Ok(g2d::Palette { colors: (0..=255).map(|i| [i, i, i, 255]).collect() }),
    }
}

// --------------------------------------------------------------------------------------------
//  Export
// --------------------------------------------------------------------------------------------

struct Writer<'a> {
    root: PathBuf,
    previous: HashMap<String, &'a Item>,
    items: Vec<Item>,
    kept: usize,
}

impl Writer<'_> {
    /// Écrit un fichier exporté, sauf s'il a été modifié depuis l'export d'avant : le travail de l'utilisateur passe avant.
    fn put(&mut self, relative: &str, source: &Entry, recipe: Recipe, bytes: impl FnOnce() -> Result<Vec<u8>, String>) -> Result<(), String> {
        let path = self.root.join(relative);
        if let Some(old) = self.previous.get(relative) {
            if let Ok(on_disk) = std::fs::read(&path) {
                let item = (*old).clone();
                if fingerprint(&on_disk) != item.hash {
                    self.kept += 1;
                }
                self.items.push(item);
                return Ok(());
            }
        } else if path.exists() {
            // Pas exporté par cet outil : un fichier posé à la main n'est pas écrasé
            self.kept += 1;
            return Ok(());
        }
        let bytes = bytes()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
        self.items.push(Item { file: relative.to_string(), hash: fingerprint(&bytes), source: source.path.clone(), recipe });
        Ok(())
    }

    fn image(&mut self, relative: String, source: &Entry, recipe: Recipe, image: impl FnOnce() -> Result<Rgba, String>) -> Result<(), String> {
        self.put(&relative, source, recipe, || png_bytes(&image()?))
    }
}

/// Le dossier des textures d'un fichier 3D.
fn textures_dir(base: &str) -> String {
    format!("{base}.textures")
}

fn clean_name(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' { c } else { '_' }).collect()
}

/// Le nom de fichier de chaque texture, unique dans son dossier.
fn texture_files(cont: &nds_nitro::nitro::Container) -> Vec<String> {
    let mut seen = HashSet::new();
    cont.textures
        .iter()
        .enumerate()
        .map(|(i, tex)| {
            let name = clean_name(&tex.name.to_string());
            if seen.insert(name.clone()) { format!("{name}.png") } else { format!("{name}_{i}.png") }
        })
        .collect()
}

fn export_textures(writer: &mut Writer, entry: &Entry, base: &str, cont: &nds_nitro::nitro::Container) -> Result<(), String> {
    let files = texture_files(cont);
    // La palette qu'un matériau associe à la texture, sinon celle de même nom, sinon la première
    let mut pairs: HashMap<String, String> = HashMap::new();
    for material in cont.models.iter().flat_map(|m| &m.materials) {
        if let (Some(texture), Some(palette)) = (&material.texture_name, &material.palette_name) {
            pairs.entry(texture.to_string()).or_insert(palette.to_string());
        }
    }
    for (index, tex) in cont.textures.iter().enumerate() {
        let name = tex.name.to_string();
        let needs = tex.params.format().desc().requires_palette;
        let wanted = pairs.get(&name).cloned().unwrap_or_else(|| name.clone());
        let palette = needs.then(|| cont.palettes.iter().position(|p| p.name.to_string() == wanted || p.name.to_string().trim_end_matches("_pl") == name).unwrap_or(0)).filter(|p| *p < cont.palettes.len());
        let relative = format!("{}/{}", textures_dir(base), files[index]);
        writer.image(relative, entry, Recipe::Texture { index, palette }, || {
            let (width, height) = tex.params.dim();
            let rgba = nds_nitro::nds::decode_texture(tex, palette.map(|p| &cont.palettes[p])).map_err(|e| e.to_string())?;
            Ok(Rgba { width, height, data: rgba.0 })
        })?;
    }
    Ok(())
}

fn export_model(writer: &mut Writer, game: &Game, sel: usize, base: &str, bytes: &[u8]) -> Result<(), String> {
    let entry = &game.entries[sel];
    let own = model3d::read(bytes).map_err(|e| e.to_string())?;
    export_textures(writer, entry, base, &own)?;
    let c = asset_scan::choices_for(&game.entries, sel);
    let companion = c.texture.checked_sub(1).and_then(|n| c.textures.get(n)).copied();
    let others = match companion {
        Some(at) => vec![model3d::read(&asset_scan::read_entry(&game.entries[at])?).map_err(|e| e.to_string())?],
        None => Vec::new(),
    };
    let companion_path = companion.map(|at| game.entries[at].path.clone());
    let directory = base.rsplit_once('/').map_or("", |(d, _)| d);
    let models = model3d::build_models(&own, &others);
    for (index, built) in models.iter().enumerate() {
        let name = if models.len() == 1 { base.to_string() } else { format!("{base}.{}", clean_name(&built.name)) };
        let mtl = format!("{name}.mtl");
        let mtl_short = mtl.rsplit('/').next().unwrap_or(&mtl).to_string();
        // Une texture du fichier, ou du fichier de textures qui va avec : sa propre image, celle qu'on modifie
        let find = |cont: &nds_nitro::nitro::Container, dir: &str, wanted: &str| {
            let at = cont.textures.iter().position(|t| t.name.to_string() == wanted)?;
            Some(relative(directory, &format!("{}/{}", textures_dir(dir), texture_files(cont)[at])))
        };
        let other_base = companion.map(|at| base_name(&game.entries[at]));
        let textures: Vec<Option<String>> = built.materials.iter().map(|m| {
            let wanted = m.texture_name.as_deref()?;
            find(&own, base, wanted).or_else(|| find(others.first()?, other_base.as_deref()?, wanted))
        }).collect();
        let (obj, material_file) = model::write_obj(built, &mtl_short, &textures);
        writer.put(&format!("{name}.obj"), entry, Recipe::Model { index, textures: companion_path.clone() }, || Ok(obj.into_bytes()))?;
        writer.put(&mtl, entry, Recipe::ExportOnly, || Ok(material_file.into_bytes()))?;
    }
    Ok(())
}

/// Les images des matériaux d'un modèle, pour le regarder : la vraie texture est un fichier à part. Rend le chemin de chacune pour le `.mtl`.
fn export_materials(writer: &mut Writer, entry: &Entry, base: &str, built: &model3d::ModelOut) -> Result<Vec<Option<String>>, String> {
    let mut textures = Vec::new();
    for (m, material) in built.materials.iter().enumerate() {
        let picture = material.texture.as_ref().map(|image| (format!("{base}.apercu.{m}.png"), image));
        textures.push(picture.as_ref().map(|(path, _)| path.rsplit('/').next().unwrap_or(path).to_string()));
        if let Some((path, image)) = picture {
            writer.image(path, entry, Recipe::ExportOnly, || Ok(image.clone()))?;
        }
    }
    Ok(textures)
}

fn export_entry(writer: &mut Writer, game: &Game, sel: usize) -> Result<(), String> {
    let entry = &game.entries[sel];
    let base = base_name(entry);
    let bytes = asset_scan::read_entry(entry)?;
    let palette_path = |c: &asset_scan::Choices| path_of(&game.entries, &c.palettes, c.palette);
    let raw = |writer: &mut Writer, recipe: Recipe| writer.put(&base, entry, recipe, || Ok(bytes.clone()));
    match entry.kind {
        Kind::Palette | Kind::RawPalette => {
            let inner = decompress_any(&bytes).unwrap_or_else(|| bytes.clone());
            let palette = if entry.kind == Kind::RawPalette { g2d::raw_palette(&inner) } else { g2d::read_palette(&inner).map_err(|e| e.to_string())? };
            writer.image(format!("{base}.palette.png"), entry, Recipe::Palette { count: palette.colors.len() }, || Ok(images::palette_image(&palette)))
        }
        Kind::Tiles | Kind::RawTiles => {
            let c = asset_scan::choices_for(&game.entries, sel);
            let palette_file = palette_path(&c);
            let palette_probe = palette_file.as_deref().map(|p| game.palette(Some(p), None)).transpose()?;
            let tiles = game.tiles(&entry.path, palette_probe.as_ref())?;
            let palette = game.palette(palette_file.as_deref(), Some(&tiles))?;
            let width = tiles.default_width();
            writer.image(format!("{base}.png"), entry, Recipe::Sheet { palette: palette_file, width, bank: 0 }, || Ok(g2d::render_tiles(&tiles, &palette, 0, width)))
        }
        Kind::Screen | Kind::RawScreen => {
            let c = asset_scan::choices_for(&game.entries, sel);
            let Some(tiles_file) = path_of(&game.entries, &c.tiles, c.tile) else { return raw(writer, Recipe::Raw) };
            let palette_file = palette_path(&c);
            let palette_probe = palette_file.as_deref().map(|p| game.palette(Some(p), None)).transpose()?;
            let tiles = game.tiles(&tiles_file, palette_probe.as_ref())?;
            let palette = game.palette(palette_file.as_deref(), Some(&tiles))?;
            let inner = decompress_any(&bytes).unwrap_or_else(|| bytes.clone());
            let screen = if entry.kind == Kind::RawScreen { g2d::raw_screen(&inner) } else { g2d::read_screen(&inner).map_err(|e| e.to_string())? };
            writer.image(format!("{base}.png"), entry, Recipe::Screen { tiles: tiles_file, palette: palette_file }, || Ok(g2d::render_screen(&screen, &tiles, &palette)))
        }
        Kind::Cells => {
            let c = asset_scan::choices_for(&game.entries, sel);
            let cells = g2d::read_cells(&bytes).map_err(|e| e.to_string())?;
            let Some(tiles_file) = path_of(&game.entries, &c.tiles, c.tile) else { return raw(writer, Recipe::Raw) };
            if cells.cells.len() > MAX_CELLS {
                return raw(writer, Recipe::Raw);
            }
            let palette_file = palette_path(&c);
            let tiles = game.tiles(&tiles_file, None)?;
            let palette = game.palette(palette_file.as_deref(), Some(&tiles))?;
            for index in 0..cells.cells.len() {
                let recipe = Recipe::Cell { tiles: tiles_file.clone(), palette: palette_file.clone(), index };
                writer.image(format!("{base}.cells/{index:04}.png"), entry, recipe, || Ok(g2d::render_cell(&cells, index, &tiles, &palette)))?;
            }
            Ok(())
        }
        Kind::Textures => export_textures(writer, entry, &base, &model3d::read(&bytes).map_err(|e| e.to_string())?),
        Kind::Model => export_model(writer, game, sel, &base, &bytes),
        Kind::Audio => {
            let sound = read_sound_file(&bytes)?;
            let wav = sound.wav();
            writer.put(&format!("{base}.wav"), entry, Recipe::Sound, || Ok(wav.clone()))?;
            if media::available() && sound.seconds() >= MP3_FROM_SECONDS {
                let mp3 = format!("{base}.mp3");
                let target = writer.root.join(&mp3);
                writer.put(&mp3, entry, Recipe::Sound, || {
                    if let Some(parent) = target.parent() {
                        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                    }
                    media::wav_to_mp3(&wav, &target)?;
                    std::fs::read(&target).map_err(|e| e.to_string())
                })?;
            }
            Ok(())
        }
        Kind::AudioBank => {
            for index in 0..sdat::swar_count(&bytes).map_err(|e| e.to_string())? {
                let wave = sdat::read_swar(&bytes, index).map_err(|e| e.to_string())?;
                writer.put(&format!("{base}.sounds/{index:03}.wav"), entry, Recipe::BankSound { index }, || Ok(wave.wav()))?;
            }
            Ok(())
        }
        Kind::Messages if bytes.starts_with(b"MESG") => match text::bmg_to_text(&bytes) {
            Some(lines) => writer.put(&format!("{base}.txt"), entry, Recipe::Messages, || Ok(lines.into_bytes())),
            None => raw(writer, Recipe::Raw),
        },
        Kind::Video => {
            raw(writer, Recipe::Raw)?;
            if !media::available() {
                return Ok(());
            }
            let target = writer.root.join(format!("{base}.video"));
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let recipe = Recipe::Video;
            let short = format!("{base}.mp4");
            if writer.previous.contains_key(&short) {
                writer.put(&short, entry, recipe, || Ok(Vec::new()))?;
                return Ok(());
            }
            for made in media::video_to_files(bytes.clone(), &target)? {
                let bytes = std::fs::read(&made).map_err(|e| e.to_string())?;
                let name = made.strip_prefix(&writer.root).map_or(short.clone(), |p| p.to_string_lossy().replace('\\', "/"));
                writer.items.push(Item { file: name, hash: fingerprint(&bytes), source: entry.path.clone(), recipe: recipe.clone() });
            }
            Ok(())
        }
        Kind::Texel => {
            let c = asset_scan::choices_for(&game.entries, sel);
            let palette_file = palette_path(&c);
            let palette = game.palette(palette_file.as_deref(), None)?;
            let texel = g2d::read_texel(&bytes).map_err(|e| e.to_string())?;
            writer.image(format!("{base}.png"), entry, Recipe::Texel { palette: palette_file }, || g2d::render_texel(&texel, &palette).map_err(|e| e.to_string()))
        }
        Kind::BsTexture => {
            let inner = decompress_any(&bytes).unwrap_or_else(|| bytes.clone());
            let (texel, palette) = g2d::read_bstga(&inner).map_err(|e| e.to_string())?;
            writer.image(format!("{base}.png"), entry, Recipe::BsTexture, || g2d::render_texel(&texel, &palette).map_err(|e| e.to_string()))
        }
        Kind::PaletteTiles => {
            let inner = decompress_any(&bytes).unwrap_or_else(|| bytes.clone());
            let (palette, tiles) = g2d::read_palette_tiles(&inner).ok_or("planche illisible")?;
            let width = if tiles.count() % 40 == 0 && tiles.count() % 32 != 0 { 40 } else { 32 };
            writer.image(format!("{base}.png"), entry, Recipe::PaletteTiles { width }, || Ok(g2d::render_tiles(&tiles, &palette, 0, width)))
        }
        Kind::Messages if bytes.starts_with(b"GNAL") => match text::gnal_to_text(&bytes) {
            Some(lines) => writer.put(&format!("{base}.txt"), entry, Recipe::LangText, || Ok(lines.into_bytes())),
            None => raw(writer, Recipe::Raw),
        },
        Kind::ZImage | Kind::Font | Kind::Bitmap | Kind::EngineBitmap | Kind::RawTexture | Kind::TextureRef | Kind::JCells | Kind::JLayout | Kind::EngineGeometry | Kind::EngineMesh | Kind::Messages => {
            let mut c = asset_scan::choices_for(&game.entries, sel);
            match (entry.kind, asset_scan::decode(&game.entries, sel, &mut c)) {
                // Sous `mesh/`, ce qui ressemble à une image est une donnée de collision
                (Kind::ZImage, Ok(Decoded::Image { rgba, .. })) if !entry.path.contains("/mesh/") => {
                    if let Some(palette) = images::z_palette(&asset_scan::fully_unpacked(bytes.clone())) {
                        writer.image(format!("{base}.palette.png"), entry, Recipe::ZPalette, || Ok(images::palette_image(&palette)))?;
                    }
                    writer.image(format!("{base}.png"), entry, Recipe::ZImage, || Ok(rgba))
                }
                (Kind::Font, Ok(Decoded::Image { rgba, .. })) => writer.image(format!("{base}.png"), entry, Recipe::Font, || Ok(rgba)),
                (Kind::Bitmap, Ok(Decoded::Image { rgba, .. })) => writer.image(format!("{base}.png"), entry, Recipe::Bitmap, || Ok(rgba)),
                (Kind::EngineBitmap, Ok(Decoded::Image { rgba, .. })) => writer.image(format!("{base}.png"), entry, Recipe::EngineBitmap { palette: palette_path(&c) }, || Ok(rgba)),
                (Kind::ZImage | Kind::EngineGeometry | Kind::EngineMesh, Ok(Decoded::Model { model: built, .. })) => {
                    let (obj, mtl) = (format!("{base}.obj"), format!("{base}.mtl"));
                    let mtl_short = mtl.rsplit('/').next().unwrap_or(&mtl).to_string();
                    let textures = export_materials(writer, entry, &base, &built)?;
                    let (text, material_file) = model::write_obj(&built, &mtl_short, &textures);
                    writer.put(&obj, entry, Recipe::GxModel, || Ok(text.into_bytes()))?;
                    writer.put(&mtl, entry, Recipe::ExportOnly, || Ok(material_file.into_bytes()))
                }
                // Sans chemin du retour pour l'instant : l'aperçu, et le fichier du jeu à côté
                (_, decoded) => {
                    raw(writer, Recipe::Raw)?;
                    match decoded {
                        Ok(Decoded::Image { rgba, .. }) => writer.image(format!("{base}.apercu.png"), entry, Recipe::ExportOnly, || Ok(rgba)),
                        Ok(Decoded::Model { model: built, .. }) => {
                            let mtl = format!("{base}.apercu.mtl");
                            let mtl_short = mtl.rsplit('/').next().unwrap_or(&mtl).to_string();
                            let textures = export_materials(writer, entry, &base, &built)?;
                            let (obj, material_file) = model::write_obj(&built, &mtl_short, &textures);
                            writer.put(&format!("{base}.apercu.obj"), entry, Recipe::ExportOnly, || Ok(obj.into_bytes()))?;
                            writer.put(&mtl, entry, Recipe::ExportOnly, || Ok(material_file.into_bytes()))
                        }
                        Ok(Decoded::Text { lines, .. }) => writer.put(&format!("{base}.apercu.txt"), entry, Recipe::ExportOnly, || Ok(lines.join("\r\n").into_bytes())),
                        _ => Ok(()),
                    }
                }
            }
        }
        _ => raw(writer, Recipe::Raw),
    }
}

fn read_sound_file(bytes: &[u8]) -> Result<sadl::Sound, String> {
    match bytes.get(0..4) {
        Some(b"STRM") => sdat::read_strm(bytes),
        Some(b"SWAV") => sdat::read_swav(bytes),
        _ if adx::is_adx(bytes) => adx::decode(bytes),
        _ => sadl::decode(bytes),
    }
    .map_err(|e| e.to_string())
}

/// Le fichier d'explications, dans la langue du launcher.
fn readme() -> String {
    t!(
        "MODDING THIS GAME\r\n\r\nEdit the files in the '{FILES}' folder, then press 'Apply my changes' in the launcher.\r\nYour rom is never modified: the launcher builds a copy, and one button brings the originals back.\r\n\r\nImages .png        GIMP: any size, brought back to the game's size. Colors are the game's; a very different texture gets its own when nothing shares them. Edit the .palette.png file for others.\r\nFonts .png         GIMP: keep the size. Only the transparency of each pixel counts, not its color.\r\nModels .obj        Blender, Godot: moving vertices and UVs works; keep the same triangles and the part_NNN names.\r\nSounds .wav .mp3   Audacity: any length for music and sound effects; sadl and ADX keep their length.\r\nTexts .txt         one line per message; keep the {{1A:...}} commands and the \\n.\r\nVideos .mp4 .ogv   they come back: same size and speed are restored, the size the game allows is kept, images may lose detail.\r\n.apercu files      to watch and reuse: edit the file they come from instead.\r\nOther files        the game's own format: they come back as they are.\r\n\r\nSharing: zip your changed files with the same paths as in '{FILES}'. Anyone can add the .zip or your repository's link in the Modding tab. A file nds-mod.json gives its name: {{\"format\": 1, \"id\": \"my-mod\", \"name\": \"My mod\", \"version\": \"1.0\", \"author\": \"...\", \"games\": [\"AL7E\"]}}.\r\n\r\nYou can also put an exported file in '{DROP_INS}' at the same path as in '{FILES}'. Expert: a file in the game's format placed there at its path in the rom is used as it is.\r\n",
        "MODDER CE JEU\r\n\r\nModifiez les fichiers du dossier '{FILES}', puis cliquez sur 'Appliquer mes changements' dans le launcher.\r\nVotre rom n'est jamais modifiee : le launcher fabrique une copie, et un bouton remet les originaux.\r\n\r\nImages .png        GIMP : toute taille, ramenee a celle du jeu. Les couleurs sont celles du jeu ; une texture tres differente recoit les siennes si rien ne les partage. Sinon, modifier le fichier .palette.png.\r\nPolices .png       GIMP : garder la taille. Seule la transparence de chaque pixel compte, pas sa couleur.\r\nModeles .obj       Blender, Godot : bouger les sommets et les UV marche ; garder les memes triangles et les noms part_NNN.\r\nSons .wav .mp3     Audacity : toute duree pour la musique et les effets ; sadl et ADX gardent leur duree.\r\nTextes .txt        une ligne par message ; garder les commandes {{1A:...}} et les \\n.\r\nVideos .mp4 .ogv   elles reviennent : taille et cadence remises, taille permise par le jeu respectee, l'image peut perdre des details.\r\nFichiers .apercu   pour regarder et reutiliser : modifier plutot le fichier dont ils viennent.\r\nAutres fichiers    le format du jeu : ils reviennent tels quels.\r\n\r\nPartager : zippez vos fichiers modifies avec les memes chemins que dans '{FILES}'. N'importe qui peut ajouter le .zip ou le lien de votre depot dans l'onglet Moding. Un fichier nds-mod.json donne son nom : {{\"format\": 1, \"id\": \"mon-mod\", \"name\": \"Mon mod\", \"version\": \"1.0\", \"author\": \"...\", \"games\": [\"AL7E\"]}}.\r\n\r\nVous pouvez aussi mettre un fichier exporte dans '{DROP_INS}', au meme chemin que dans '{FILES}'. Expert : un fichier au format du jeu pose la a son chemin dans la rom est utilise tel quel.\r\n"
    )
}

pub fn export(rom: &Path, dir: &Path, only: Option<&str>) -> Result<(), String> {
    let code = game_code(&std::fs::read(rom).map_err(|e| e.to_string())?[..0x10.min(std::fs::metadata(rom).map_err(|e| e.to_string())?.len() as usize)]);
    let root = dir.join(FILES);
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir.join(DROP_INS)).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(README), readme()).map_err(|e| e.to_string())?;
    if !media::available() {
        println!("{}", t!("FFmpeg not found: no MP3 or MP4 this time, everything else works.", "FFmpeg absent : ni MP3 ni MP4 cette fois, tout le reste marche."));
    }
    println!("{}", t!("Reading the game...", "Lecture du jeu..."));
    let game = Game::new(rom);
    if game.entries.is_empty() {
        return Err(t!("no file found in this rom", "aucun fichier trouve dans cette rom"));
    }
    // Une archive est exportée par ses fichiers
    let parents: HashSet<&str> = game.entries.iter().filter(|e| e.member.is_some() || e.rom_index.is_some()).map(|e| e.dir.as_str()).collect();
    let is_rom_bin = |e: &Entry| e.rom_index.is_none() && e.path.rsplit('/').next().is_some_and(|n| n.eq_ignore_ascii_case("rom.bin")) && parents.contains("rom.bin");
    let old = load_manifest(dir);
    let mut writer = Writer { root, previous: old.items.iter().map(|i| (i.file.clone(), i)).collect(), items: Vec::new(), kept: 0 };
    let (total, mut failed) = (game.entries.len(), 0);
    let mut shown = std::time::Instant::now();
    for sel in 0..total {
        let entry = &game.entries[sel];
        if parents.contains(entry.path.as_str()) || is_rom_bin(entry) || only.is_some_and(|text| !entry.path.contains(text)) {
            continue;
        }
        if shown.elapsed().as_millis() > 500 {
            shown = std::time::Instant::now();
            println!("{}", t!("Export {} / {total}: {}", "Export {} / {total} : {}", sel + 1, entry.path));
        }
        let result = catch_unwind(AssertUnwindSafe(|| export_entry(&mut writer, &game, sel))).unwrap_or_else(|_| Err("fichier inattendu".into()));
        if let Err(e) = result {
            failed += 1;
            // Illisible : on garde le fichier du jeu, il reste modifiable à la main
            let saved = asset_scan::read_entry(entry).and_then(|bytes| writer.put(&base_name(entry), entry, Recipe::Raw, || Ok(bytes)));
            println!("{}: {e}{}", entry.path, if saved.is_ok() { "" } else { " (!)" });
        }
    }
    // Un export limité garde ce qui a déjà été exporté ailleurs
    let seen: HashSet<String> = writer.items.iter().map(|i| i.file.clone()).collect();
    let mut items = writer.items;
    items.extend(old.items.iter().filter(|i| !seen.contains(&i.file)).cloned());
    let manifest = Manifest { code, items, complete: old.complete || only.is_none() };
    write_atomic(&dir.join(MANIFEST), &serde_json::to_vec(&manifest).map_err(|e| e.to_string())?)?;
    println!("{}", t!("Done: {} files in {FILES}, {} of yours kept, {failed} kept in the game's format.", "Termine : {} fichiers dans {FILES}, {} des votres gardes, {failed} gardes au format du jeu.", manifest.items.len(), writer.kept));
    Ok(())
}

// --------------------------------------------------------------------------------------------
//  Import
// --------------------------------------------------------------------------------------------

fn apply(game: &mut Game, item: &Item, path: &Path, shared: &HashMap<(String, Option<usize>), usize>, warnings: &mut Vec<String>) -> Result<(), String> {
    let source = game.at(&item.source)?;
    match &item.recipe {
        Recipe::Raw => game.edited.insert(source, std::fs::read(path).map_err(|e| e.to_string())?).map(|_| ()).unwrap_or(()),
        Recipe::Palette { count } => {
            let colors = images::colors_from_image(&read_png(path)?, *count)?;
            let out = images::write_palette(&game.plain(source)?, &colors)?;
            game.set_plain(source, out)?;
        }
        Recipe::Sheet { palette, width, bank } => {
            let probe = palette.as_deref().map(|p| game.palette(Some(p), None)).transpose()?;
            let mut tiles = game.tiles(&item.source, probe.as_ref())?;
            let palette = game.palette(palette.as_deref(), Some(&tiles))?;
            let rows = (tiles.count() as u32).div_ceil((*width).max(1)).max(1);
            let picture = read_png_sized(path, *width * 8, rows * 8, &item.file, warnings)?;
            images::sheet_from_image(&mut tiles, &palette, *bank, *width, &picture)?;
            game.store_tiles(&item.source, &tiles)?;
        }
        Recipe::Screen { tiles: tiles_file, palette } => {
            let probe = palette.as_deref().map(|p| game.palette(Some(p), None)).transpose()?;
            let mut tiles = game.tiles(tiles_file, probe.as_ref())?;
            let palette = game.palette(palette.as_deref(), Some(&tiles))?;
            let bytes = game.plain(source)?;
            let mut screen = if game.entries[source].kind == Kind::RawScreen { g2d::raw_screen(&bytes) } else { g2d::read_screen(&bytes).map_err(|e| e.to_string())? };
            let picture = read_png_sized(path, screen.width, screen.height, &item.file, warnings)?;
            let added = images::screen_from_image(&mut screen, &mut tiles, &palette, &picture)?;
            if added > 0 {
                warnings.push(t!("{}: {added} new tiles: check in game that they fit in video memory", "{} : {added} nouvelles tuiles : verifier en jeu qu'elles tiennent en memoire video", item.file));
            }
            game.store_tiles(tiles_file, &tiles)?;
            let out = images::write_screen(&bytes, &screen.entries)?;
            game.set_plain(source, out)?;
        }
        Recipe::Cell { tiles: tiles_file, palette, index } => {
            let cells = g2d::read_cells(&game.current(source)?).map_err(|e| e.to_string())?;
            let mut tiles = game.tiles(tiles_file, None)?;
            let palette = game.palette(palette.as_deref(), Some(&tiles))?;
            let shown = g2d::render_cell(&cells, *index, &tiles, &palette);
            let picture = read_png_sized(path, shown.width, shown.height, &item.file, warnings)?;
            images::cell_from_image(&cells, *index, &mut tiles, &palette, &picture)?;
            game.store_tiles(tiles_file, &tiles)?;
        }
        Recipe::Texture { index, palette } => {
            // Sa palette n'est à elle que si aucune autre texture du fichier ne s'en sert, et si son fichier ne sert pas de textures à un modèle ailleurs
            let own_palette = shared.get(&(item.source.clone(), *palette)).is_none_or(|n| *n <= 1) && palette.is_some();
            let bytes = game.current(source)?;
            let cont = model3d::read(&bytes).map_err(|e| e.to_string())?;
            let texture = cont.textures.get(*index).ok_or("texture absente")?;
            let (width, height) = texture.params.dim();
            let picture = read_png_sized(path, width, height, &item.file, warnings)?;
            let chosen = palette.and_then(|p| cont.palettes.get(p));
            // Jusqu'à la palette suivante, ou la fin du bloc
            let room = chosen.map_or(0, |own| cont.palettes.iter().map(|p| p.off as usize).filter(|off| *off > own.off as usize).min().unwrap_or(own.pal_block.len()).saturating_sub(own.off as usize));
            let (out, approximate) = images::write_texture(&bytes, texture, chosen, room, &picture, own_palette)?;
            if approximate {
                warnings.push(t!("{}: this texture shares its colors or is compressed: the closest colors of the game were used", "{} : cette texture partage ses couleurs ou est compressee : les couleurs les plus proches du jeu ont ete prises", item.file));
            }
            game.edited.insert(source, out);
        }
        Recipe::Model { index, textures } => {
            let bytes = game.current(source)?;
            let own = model3d::read(&bytes).map_err(|e| e.to_string())?;
            let others = match textures {
                Some(file) => vec![model3d::read(&game.current(game.at(file)?)?).map_err(|e| e.to_string())?],
                None => Vec::new(),
            };
            let built = model3d::build_models(&own, &others);
            let target = own.models.get(*index).ok_or("modele absent")?;
            let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
            let (out, too_far) = model::import_obj(&bytes, target, *index, built.get(*index).ok_or("modele absent")?, &text)?;
            if too_far {
                warnings.push(t!("{}: some vertices are too far from the center for the DS and were brought back", "{} : des sommets sont trop loin du centre pour la DS et ont ete ramenes", item.file));
            }
            game.edited.insert(source, out);
        }
        Recipe::Sound => {
            let bytes = game.current(source)?;
            let original = read_sound_file(&bytes)?;
            let samples = media::read_sound(path, original.rate, original.channels)?;
            let out = match bytes.get(0..4) {
                Some(b"STRM") => sound::write_strm(&bytes, &samples)?,
                Some(b"SWAV") => sound::write_swav(&bytes, &samples)?,
                _ if adx::is_adx(&bytes) => sound::write_adx(&bytes, &samples)?,
                _ => sound::write_sadl(&bytes, &samples)?,
            };
            if !bytes.starts_with(b"STRM") && !bytes.starts_with(b"SWAV") && samples.len() != original.samples.len() {
                warnings.push(t!("{}: this format keeps its length: cut or completed with silence", "{} : ce format garde sa duree : coupe ou complete par du silence", item.file));
            }
            game.edited.insert(source, out);
        }
        Recipe::BankSound { index } => {
            let bytes = game.current(source)?;
            let original = sdat::read_swar(&bytes, *index).map_err(|e| e.to_string())?;
            let samples = media::read_sound(path, original.rate, original.channels)?;
            game.edited.insert(source, sound::write_swar(&bytes, *index, &samples)?);
        }
        Recipe::Messages => {
            let lines = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
            let (out, lossy) = text::text_to_bmg(&game.current(source)?, lines.trim_start_matches('\u{FEFF}'))?;
            if lossy {
                warnings.push(t!("{}: some characters do not exist in this game's encoding", "{} : des caracteres n'existent pas dans l'encodage de ce jeu", item.file));
            }
            game.edited.insert(source, out);
        }
        Recipe::Texel { palette } => {
            let bytes = game.current(source)?;
            let texel = g2d::read_texel(&bytes).map_err(|e| e.to_string())?;
            let colors = game.palette(palette.as_deref(), None)?;
            let picture = read_png_sized(path, texel.width, texel.height, &item.file, warnings)?;
            let pixels = images::texel_from_image(&texel, &colors, &picture)?;
            let mut out = bytes.clone();
            let size = rd32(&bytes, 4).min(pixels.len());
            out[8..8 + size].copy_from_slice(&pixels[..size]);
            game.edited.insert(source, out);
        }
        Recipe::BsTexture => {
            let bytes = game.plain(source)?;
            let (texel, palette) = g2d::read_bstga(&bytes).map_err(|e| e.to_string())?;
            let picture = read_png_sized(path, texel.width, texel.height, &item.file, warnings)?;
            let pixels = images::texel_from_image(&texel, &palette, &picture)?;
            let (at, size) = (rd32(&bytes, 20), rd32(&bytes, 24).min(pixels.len()));
            let mut out = bytes.clone();
            out[at..at + size].copy_from_slice(&pixels[..size]);
            game.set_plain(source, out)?;
        }
        Recipe::PaletteTiles { width } => {
            let bytes = game.plain(source)?;
            let (palette, mut tiles) = g2d::read_palette_tiles(&bytes).ok_or("planche illisible")?;
            let rows = (tiles.count() as u32).div_ceil((*width).max(1)).max(1);
            let picture = read_png_sized(path, *width * 8, rows * 8, &item.file, warnings)?;
            images::sheet_from_image(&mut tiles, &palette, 0, *width, &picture)?;
            let header = if tiles.bpp8 { 512 } else { 32 };
            let mut out = bytes[..header].to_vec();
            out.extend(images::write_tiles(b"", &tiles)?);
            game.set_plain(source, out)?;
        }
        Recipe::ZImage => {
            let (out, added) = images::z_from_image(&game.unpacked(source)?, &read_png(path)?)?;
            if added > 0 {
                warnings.push(t!("{}: {added} new tiles: check in game that they fit in video memory", "{} : {added} nouvelles tuiles : verifier en jeu qu'elles tiennent en memoire video", item.file));
            }
            game.set_unpacked(source, &out)?;
        }
        Recipe::ZPalette => {
            let bytes = game.unpacked(source)?;
            let count = images::z_palette(&bytes).ok_or("palette illisible")?.colors.len();
            let colors = images::colors_from_image(&read_png(path)?, count)?;
            game.set_unpacked(source, &images::z_with_palette(&bytes, &colors)?)?;
        }
        Recipe::Font => {
            let out = images::font_from_image(&game.current(source)?, &read_png(path)?)?;
            game.edited.insert(source, out);
        }
        Recipe::Bitmap => {
            let out = images::bmp_from_image(&game.current(source)?, &read_png(path)?)?;
            game.edited.insert(source, out);
        }
        Recipe::EngineBitmap { palette } => {
            let bytes = game.current(source)?;
            let colors = bitmap_palette(game, palette.as_deref())?;
            let before = g2d::render_engine_bitmap(&bytes, &colors).map_err(|e| e.to_string())?;
            let pixels = decompress_any(bytes.get(8..).ok_or("image coupee")?).ok_or("image PMB non compressee")?;
            let new = images::bitmap_pixels(&pixels, &before, &colors, &read_png(path)?)?;
            game.edited.insert(source, bitmap_with(&bytes, &new)?);
        }
        Recipe::LangText => {
            let lines = std::fs::read(path).map_err(|e| e.to_string())?;
            let lines = String::from_utf8_lossy(&lines);
            let (out, lossy) = text::text_to_gnal(&game.current(source)?, lines.trim_start_matches('\u{FEFF}'))?;
            if lossy {
                warnings.push(t!("{}: some characters do not exist in this game's encoding", "{} : des caracteres n'existent pas dans l'encodage de ce jeu", item.file));
            }
            game.edited.insert(source, out);
        }
        Recipe::GxModel => {
            if apply_gx_model(game, source, path)? {
                warnings.push(t!("{}: some vertices are too far from the center for the DS and were brought back", "{} : des sommets sont trop loin du centre pour la DS et ont ete ramenes", item.file));
            }
        }
        Recipe::Video => {
            let (out, written) = media::video_to_game(&game.current(source)?, path)?;
            if written.worst_level >= 5 {
                warnings.push(t!("{}: some images were simplified to fit the size the game allows", "{} : des images ont ete simplifiees pour tenir dans la taille permise par le jeu", item.file));
            }
            game.edited.insert(source, out);
        }
        Recipe::ExportOnly => {}
    }
    Ok(())
}

/// Une image ` PMB` avec ces pixels. Après ses données compressées (leur taille est à 4), le fichier nomme sa palette : ça reste.
fn bitmap_with(file: &[u8], pixels: &[u8]) -> Result<Vec<u8>, String> {
    let end = (8 + rd32(file, 4)).min(file.len());
    let new = compress::same_method(file.get(8..end).ok_or("image coupee")?, pixels)?;
    let mut out = file[..8].to_vec();
    wr32(&mut out, 4, new.len());
    out.extend(new);
    out.extend(&file[end..]);
    Ok(out)
}

/// La taille d'une texture comme la voit `read_commands` : 1 sans texture, les coordonnées restent alors en pixels.
fn texture_size(built: &model3d::ModelOut, part: &model3d::Part) -> (f64, f64) {
    built.materials.get(part.material).and_then(|m| m.texture.as_ref()).map_or((1.0, 1.0), |t| (t.width.max(1) as f64, t.height.max(1) as f64))
}

/// Un `GEOM` avec cette nouvelle liste de commandes.
fn geometry_with(file: &[u8], list: &[u8]) -> Vec<u8> {
    let mut out = file[..4].to_vec();
    out.extend((list.len() as u32).to_le_bytes());
    out.extend(list);
    out.extend(file.get(8 + rd32(file, 4)..).unwrap_or(&[]));
    out
}

/// Un `GEOM`, un `MESH` (ses morceaux sont des `GEOM`) ou un maillage du monde en `.z`, avec les sommets de l'OBJ. `true` si des sommets
/// sortent de ce que la DS sait écrire.
fn apply_gx_model(game: &mut Game, source: usize, path: &Path) -> Result<bool, String> {
    let mut choices = asset_scan::choices_for(&game.entries, source);
    let Decoded::Model { model: built, .. } = asset_scan::decode(&game.entries, source, &mut choices)? else { return Err("modele illisible".into()) };
    let changes = model::gx_changes(&built, &std::fs::read_to_string(path).map_err(|e| e.to_string())?)?;
    let same = |at: [f32; 3]| at.map(f64::from);
    let mut warned = false;
    match game.entries[source].kind {
        Kind::EngineGeometry | Kind::EngineMesh => {
            let files = if game.entries[source].kind == Kind::EngineMesh { asset_scan::mesh_geometries(&game.entries, source) } else { vec![source] };
            if files.len() != built.parts.len() {
                return Err(format!("{} morceaux, {} fichiers de geometrie", built.parts.len(), files.len()));
            }
            for ((file, part), new) in files.into_iter().zip(&built.parts).zip(&changes) {
                let bytes = game.current(file)?;
                let list = bytes.get(8..8 + rd32(&bytes, 4)).unwrap_or_else(|| bytes.get(8..).unwrap_or(&[]));
                if let Some(list) = model::rewrite_gx(list, new, texture_size(&built, part), same, &mut warned)? {
                    game.edited.insert(file, geometry_with(&bytes, &list));
                }
            }
        }
        _ => {
            let mut bytes = game.unpacked(source)?;
            let (scale, parts) = gx_mesh::world_parts(&bytes).ok_or("maillage illisible")?;
            let drawn: Vec<(usize, &gx_mesh::WorldPart)> = parts.iter().enumerate().filter(|(_, p)| gx_mesh::draws_triangles(&bytes[p.list..p.list + p.size])).collect();
            if drawn.len() != built.parts.len() {
                return Err(format!("{} morceaux, {} attendus", drawn.len(), built.parts.len()));
            }
            // Du dernier au premier : une liste qui grandit ne décale que ce qui la suit
            let mut lists = Vec::new();
            for (((index, piece), part), new) in drawn.iter().zip(&built.parts).zip(&changes) {
                let to_local = |at: [f32; 3]| [0, 1, 2].map(|axis| (at[axis] / scale - piece.center[axis]) as f64);
                if let Some(list) = model::rewrite_gx(&bytes[piece.list..piece.list + piece.size], new, texture_size(&built, part), to_local, &mut warned)? {
                    lists.push((*index, list));
                }
            }
            for (index, list) in lists.into_iter().rev() {
                let piece = &parts[index];
                let growth = list.len() as i64 - piece.size as i64;
                wr32(&mut bytes, piece.base + 0x18, list.len());
                bytes.splice(piece.list..piece.list + piece.size, list);
                for other in 0..parts.len() {
                    let at = gx_mesh::WORLD_TABLE_AT + other * 4;
                    if parts[other].base > piece.base {
                        let moved = (rd32(&bytes, at) as i64 + growth) as usize;
                        wr32(&mut bytes, at, moved);
                    }
                }
            }
            game.set_unpacked(source, &bytes)?;
        }
    }
    Ok(warned)
}

/// Les fichiers de la rom, une fois chaque fichier modifié remis dans ce qui le contient.
fn pack(game: &Game, rom: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut whole = BTreeMap::new();
    let mut in_rom_bin = HashMap::new();
    let mut members: HashMap<String, Vec<usize>> = HashMap::new();
    for (&index, bytes) in &game.edited {
        let entry = &game.entries[index];
        if let Some(n) = entry.rom_index {
            in_rom_bin.insert(n as usize, bytes.clone());
        } else if entry.member.is_some() {
            members.entry(entry.dir.clone()).or_default().push(index);
        } else {
            let raw = &rom[entry.span.0..entry.span.1];
            whole.insert(entry.path.clone(), compress::like(raw, bytes)?);
        }
    }
    if !in_rom_bin.is_empty() {
        let file = game.entries.iter().find(|e| e.rom_index.is_none() && e.path.rsplit('/').next().is_some_and(|n| n.eq_ignore_ascii_case("rom.bin"))).ok_or("rom.bin introuvable")?;
        whole.insert(file.path.clone(), container::rebuild_rom_bin(&rom[file.span.0..file.span.1], &in_rom_bin)?);
    }
    for (parent_path, changed) in members {
        let parent = &game.entries[game.at(&parent_path)?];
        let raw = &rom[parent.span.0..parent.span.1];
        let inner = nds_nitro::unpack(raw.to_vec());
        // Chaque morceau reprend la compression qu'il avait dans l'archive
        let mut by_start = HashMap::new();
        let mut by_number = HashMap::new();
        let mut by_name = BTreeMap::new();
        for index in changed {
            let entry = &game.entries[index];
            let (start, end) = entry.member.ok_or("archive inattendue")?;
            let bytes = compress::like(inner.get(start..end).ok_or("archive changee")?, &game.edited[&index])?;
            let number: usize = entry.path.rsplit_once('#').and_then(|(_, n)| n.parse().ok()).ok_or("archive inattendue")?;
            by_number.insert(number, bytes.clone());
            if let Some(name) = &entry.member_name {
                by_name.insert(name.clone(), bytes.clone());
            }
            by_start.insert(start, (bytes, end - start));
        }
        let plain_starts = || by_start.iter().map(|(start, (bytes, _))| (*start, bytes.clone())).collect::<HashMap<_, _>>();
        // Une archive qui range des vidéos : sa table peut être un fichier à côté d'elle
        let table_path = packs::companion(&parent_path);
        let table = table_path.as_ref().and_then(|path| game.entries.iter().find(|e| e.path.eq_ignore_ascii_case(path))).map(|e| &rom[e.span.0..e.span.1]);
        if !packs::videos(&inner, table).is_empty() {
            let (out, new_table) = packs::rebuild(&inner, table, &plain_starts()).ok_or_else(|| format!("{parent_path} : archive inattendue"))?;
            if let (Some(path), Some(new_table)) = (table_path, new_table) {
                whole.insert(path, new_table);
            }
            whole.insert(parent_path, out);
            continue;
        }
        let new_inner = match parent.kind {
            Kind::Archive if narc::read(&inner).is_ok() => container::rebuild_narc(&inner, &by_number)?,
            Kind::SoundArchive => container::rebuild_sdat(&inner, &plain_starts())?,
            Kind::NestedRom => container::patch_rom(&inner, &by_name)?,
            _ => match container::rebuild_table(&inner, &plain_starts()) {
                Some(out) => out,
                None => {
                    // Fichiers collés sans table : seule une taille égale ou plus petite garde les autres en place
                    let mut out = inner.clone();
                    for (start, (bytes, size)) in &by_start {
                        if bytes.len() > *size {
                            return Err(format!("{parent_path} : un fichier colle ne peut pas grandir"));
                        }
                        out[*start..*start + bytes.len()].copy_from_slice(bytes);
                    }
                    out
                }
            },
        };
        whole.insert(parent_path, compress::like(raw, &new_inner)?);
    }
    Ok(whole)
}

/// Les fichiers posés à la main, au format du jeu.
fn drop_ins(root: &Path, at: &Path, out: &mut BTreeMap<String, Vec<u8>>) -> Result<(), String> {
    for entry in std::fs::read_dir(at).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            drop_ins(root, &path, out)?;
        } else if let Ok(short) = path.strip_prefix(root) {
            out.insert(short.to_string_lossy().replace('\\', "/"), std::fs::read(&path).map_err(|e| e.to_string())?);
        }
    }
    Ok(())
}

/// Le fichier exporté que désigne un chemin venu d'un mod : le même, ou le même sous un dossier en plus (une archive a souvent un dossier autour).
fn exported_path<'a>(path: &'a str, exported: &HashSet<&str>) -> Option<&'a str> {
    let mut rest = path;
    loop {
        if exported.contains(rest) {
            return Some(rest);
        }
        rest = rest.split_once('/')?.1;
    }
}

/// Les dossiers des mods actifs, dans l'ordre : le dernier passe avant les autres.
fn enabled_packs(dir: &Path) -> Vec<PathBuf> {
    #[derive(Deserialize)]
    struct Entry {
        id: String,
        enabled: bool,
    }
    #[derive(Deserialize)]
    struct State {
        packs: Vec<Entry>,
    }
    let Some(state) = std::fs::read(dir.join(PACK_STATE)).ok().and_then(|bytes| serde_json::from_slice::<State>(&bytes).ok()) else { return Vec::new() };
    state.packs.into_iter().filter(|p| p.enabled).map(|p| dir.join(PACKS).join(p.id)).filter(|p| p.is_dir()).collect()
}

pub fn import(rom_path: &Path, dir: &Path) -> Result<(), String> {
    let packs = enabled_packs(dir);
    // Un mod donne ses fichiers par leur chemin à l'export : il faut donc l'export entier
    if !packs.is_empty() && !load_manifest(dir).complete {
        println!("{}", t!("First export of the game, to place the mods...", "Premier export du jeu, pour placer les mods..."));
        export(rom_path, dir, None)?;
    }
    let manifest = load_manifest(dir);
    let rom = std::fs::read(rom_path).map_err(|e| e.to_string())?;
    if !manifest.code.is_empty() && manifest.code != game_code(&rom) {
        return Err(t!("this mod is for {}, this rom is {}", "ce mod est pour {}, cette rom est {}", manifest.code, game_code(&rom)));
    }
    let root = dir.join(FILES);
    // Du plus ancien au plus récent : d'un WAV et d'un MP3 du même son, le dernier modifié gagne
    let exported: HashSet<&str> = manifest.items.iter().map(|i| i.file.as_str()).collect();
    // Ce que les mods remplacent parmi les fichiers exportés, et le reste : au format du jeu, ou inconnu
    let mut from_packs: HashMap<String, PathBuf> = HashMap::new();
    let mut pack_natives = BTreeMap::new();
    let mut report = Report::default();
    for pack in &packs {
        let name = pack.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
        for sub in [FILES, DROP_INS] {
            let folder = pack.join(sub);
            let mut listed = BTreeMap::new();
            drop_ins(&folder, &folder, &mut listed)?;
            for (path, bytes) in listed {
                match exported_path(&path, &exported) {
                    Some(file) => {
                        from_packs.insert(file.to_string(), folder.join(&path));
                    }
                    None if sub == DROP_INS => {
                        pack_natives.insert(path, bytes);
                    }
                    None => report.errors.push(t!("Mod {name}: {path} matches no file of this game", "Mod {name} : {path} ne correspond a aucun fichier de ce jeu")),
                }
            }
        }
    }
    let mut changed: Vec<(std::time::SystemTime, &Item, PathBuf)> = Vec::new();
    let drops = dir.join(DROP_INS);
    for item in &manifest.items {
        // Un fichier exporté posé dans `remplacements`, au même chemin, passe avant celui de `fichiers`
        let dropped = drops.join(&item.file);
        let path = if dropped.is_file() { dropped } else { root.join(&item.file) };
        // Vos changements passent avant les mods, et le dernier mod avant les autres
        let personal = std::fs::read(&path).is_ok_and(|bytes| fingerprint(&bytes) != item.hash);
        let path = match from_packs.get(&item.file).filter(|_| !personal) {
            Some(pack_file) => pack_file.clone(),
            None => path,
        };
        let Ok(bytes) = std::fs::read(&path) else { continue };
        // Essai de toute la chaine : tout est reimporte sans avoir change, et la rom produite doit se reexporter a l'identique
        // Une vidéo se réencode avec des pertes : hors de cet essai, qui veut une rom identique
        let test_all = std::env::var_os("NDSGS_MOD_TEST_ALL").is_some() && !matches!(item.recipe, Recipe::Video);
        if fingerprint(&bytes) != item.hash || test_all {
            let time = std::fs::metadata(&path).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
            changed.push((time, item, path));
        }
    }
    changed.sort_by_key(|(time, _, _)| *time);
    // Les palettes d'abord : une image et sa palette modifiées ensemble vont avec la nouvelle palette
    changed.sort_by_key(|(_, item, _)| !matches!(item.recipe, Recipe::Palette { .. } | Recipe::ZPalette));
    let mut shared: HashMap<(String, Option<usize>), usize> = HashMap::new();
    for item in &manifest.items {
        if let Recipe::Texture { palette, .. } = item.recipe {
            *shared.entry((item.source.clone(), palette)).or_default() += 1;
        }
    }
    println!("{}", t!("{} changed files.", "{} fichiers modifies.", changed.len()));
    let mut game = Game::new(rom_path);
    for (_, item, path) in &changed {
        if matches!(item.recipe, Recipe::ExportOnly) {
            report.export_only.push(item.file.clone());
            continue;
        }
        println!("{}", t!("Import: {}", "Import : {}", item.file));
        let mut warnings = Vec::new();
        let result = catch_unwind(AssertUnwindSafe(|| apply(&mut game, item, path, &shared, &mut warnings))).unwrap_or_else(|_| Err("fichier inattendu".into()));
        report.warnings.extend(warnings);
        match result {
            Ok(()) => report.applied.push(item.file.clone()),
            Err(e) => report.errors.push(format!("{} : {e}", item.file)),
        }
    }
    let mut files = pack(&game, &rom).inspect_err(|e| report.errors.push(e.clone())).unwrap_or_default();
    let mut hand_placed = pack_natives;
    drop_ins(&drops, &drops, &mut hand_placed)?;
    // Les fichiers exportés posés là sont déjà traités ; le reste doit être un fichier de la rom, au format du jeu
    hand_placed.retain(|path, _| {
        let known = exported.contains(path.as_str());
        let in_rom = game.entries.iter().any(|e| e.member.is_none() && e.rom_index.is_none() && e.path.eq_ignore_ascii_case(path));
        if !known && !in_rom {
            report.errors.push(t!("{DROP_INS}/{path}: unknown file. Put it at the same path as in '{FILES}', or at its path in the rom.", "{DROP_INS}/{path} : fichier inconnu. Le mettre au meme chemin que dans '{FILES}', ou a son chemin dans la rom."));
        }
        !known && in_rom
    });
    report.applied.extend(hand_placed.keys().map(|k| format!("{DROP_INS}/{k}")));
    files.extend(hand_placed);
    let target = dir.join(MODDED_ROM);
    if files.is_empty() {
        // La copie vient de cet outil et se refait quand on veut
        let _ = std::fs::remove_file(&target);
        println!("{}", t!("Nothing to change: the game stays original.", "Rien a changer : le jeu reste d'origine."));
    } else {
        println!("{}", t!("Building the modded copy of the rom...", "Fabrication de la copie modifiee de la rom..."));
        let out = container::patch_rom(&rom, &files)?;
        write_atomic(&target, &out)?;
        report.built = true;
        println!("{}", t!("Done: {} files changed in the copy, your rom is untouched.", "Termine : {} fichiers changes dans la copie, votre rom n'a pas bouge.", files.len()));
    }
    for line in report.errors.iter().chain(&report.warnings) {
        println!("{line}");
    }
    for file in &report.export_only {
        println!("{}", t!("{file}: cannot go back into the game yet.", "{file} : ne peut pas encore revenir dans le jeu."));
    }
    write_atomic(&dir.join(REPORT), &serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Essai sur une vraie rom : `NDSGS_TEST_ROM`, et `NDSGS_TEST_MODEL` le fichier du modèle (`rom.bin#3416`).
    #[test]
    fn a_real_model_squeezed_in_its_obj_comes_back_squeezed() {
        let (Some(rom), Some(path)) = (std::env::var_os("NDSGS_TEST_ROM"), std::env::var("NDSGS_TEST_MODEL").ok()) else { return };
        let game = Game::new(Path::new(&rom));
        let file = asset_scan::read_entry(&game.entries[game.at(&path).unwrap()]).unwrap();
        let cont = model3d::read(&file).unwrap();
        let built = model3d::build_models(&cont, &[]);
        let (obj, _) = model::write_obj(&built[0], "m.mtl", &[]);
        let (same, far) = model::import_obj(&file, &cont.models[0], 0, &built[0], &obj).unwrap();
        assert!(same == file && !far, "sans changement : identique");
        // Autour de son centre : un modèle peut être loin de l'origine, et la DS ne va pas au-delà de 8 unités autour de son repère
        let center = (built[0].min[0] + built[0].max[0]) / 2.0;
        let squeezed: String = obj.lines().map(|line| match line.strip_prefix("v ") {
            Some(rest) => {
                let v: Vec<f32> = rest.split(' ').map(|x| x.parse().unwrap()).collect();
                format!("v {:.6} {} {}\n", center + (v[0] - center) * 0.5, v[1], v[2])
            }
            None => format!("{line}\n"),
        }).collect();
        let (out, far) = model::import_obj(&file, &cont.models[0], 0, &built[0], &squeezed).unwrap();
        let back = model3d::read(&out).unwrap();
        let again = model3d::build_models(&back, &[]);
        let scale = cont.models[0].up_scale;
        let mut worst = 0f32;
        for (a, b) in built[0].parts.iter().zip(&again[0].parts) {
            for (p, q) in a.positions.iter().zip(&b.positions) {
                worst = worst.max((center + (p[0] - center) * 0.5 - q[0]).abs()).max((p[1] - q[1]).abs()).max((p[2] - q[2]).abs());
            }
        }
        println!("up {scale} parts {} far {far} worst {worst} box {:?} {:?}", built[0].parts.len(), built[0].min, built[0].max);
        assert!(worst < 0.01 * scale as f32 + 0.01, "ecart {worst}");
    }

    #[test]
    fn a_pmb_keeps_the_name_of_its_palette() {
        let stream = compress::lz10(&[1u8; 256]).unwrap();
        let mut file = b" PMB".to_vec();
        file.extend((stream.len() as u32).to_le_bytes());
        file.extend(&stream);
        file.extend(b"   pal0");
        let out = bitmap_with(&file, &[2u8; 256]).unwrap();
        assert!(out.ends_with(b"   pal0"));
        let size = rd32(&out, 4);
        assert_eq!(decompress_any(&out[8..8 + size]).unwrap(), [2u8; 256]);
        assert_eq!(&out[8 + size..], b"   pal0");
    }

    #[test]
    fn a_mods_path_is_found_even_under_an_extra_folder() {
        let exported = HashSet::from(["rom.bin/1.png", "a/b.wav"]);
        assert_eq!(exported_path("rom.bin/1.png", &exported), Some("rom.bin/1.png"));
        assert_eq!(exported_path("mon mod/rom.bin/1.png", &exported), Some("rom.bin/1.png"));
        assert_eq!(exported_path("autre/1.png", &exported), None);
    }

    #[test]
    fn paths_are_safe_on_windows() {
        assert_eq!(clean_path("a:b/c?.png"), "a_b/c_.png");
        assert_eq!(clean_path("dir./x "), "dir/x");
    }

    #[test]
    fn relative_paths_go_up_then_down() {
        assert_eq!(relative("a/b", "a/c/d.png"), "../c/d.png");
        assert_eq!(relative("", "x/y.png"), "x/y.png");
        assert_eq!(relative("a", "a/y.png"), "y.png");
    }
}
