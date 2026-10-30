//! Modèles NSBMD -> maillages et images RGBA (pose de repos), et animations de squelette NSBCA.
//!
//! Les textures d'un matériau sont cherchées par nom, comme le fait apicula (`connection.rs`) :
//! d'abord dans le fichier du modèle, puis dans les autres fichiers donnés (souvent un `.nsbtx`
//! à côté) ; la palette d'abord dans le fichier de la texture.

use crate::errors::Result;
use crate::nitro::{Animation, Container, Model, Name, Palette, Texture};
use crate::primitives::{DynamicState, PolyType, Primitives};
use crate::util::cur::Cur;
use crate::Rgba;

/// Lit un fichier Nitro 3D (BMD0, BTX0...).
pub fn read(bytes: &[u8]) -> Result<Container> {
    crate::nitro::container::read_container(Cur::new(bytes))
}

/// Un morceau de modèle dessiné avec un seul matériau.
pub struct Part {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    pub material: usize,
    /// Le morceau a des normales (éclairé sur la console) ; sinon couleurs de sommets.
    pub lit: bool,
}

pub struct MaterialOut {
    pub name: String,
    pub texture: Option<Rgba>,
    /// Nom de la texture voulue (même introuvable).
    pub texture_name: Option<String>,
    pub repeat_s: bool,
    pub repeat_t: bool,
    pub mirror_s: bool,
    pub mirror_t: bool,
    pub alpha: f32,
    pub cull_back: bool,
}

pub struct ModelOut {
    pub name: String,
    pub parts: Vec<Part>,
    pub materials: Vec<MaterialOut>,
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub vertex_count: usize,
    pub triangle_count: usize,
    /// Textures ou palettes introuvables, etc.
    pub warnings: Vec<String>,
}

/// Toutes les textures et palettes utilisables, avec le fichier d'où elles viennent
/// (0 = le fichier du modèle).
#[derive(Default)]
pub struct TextureLib<'a> {
    textures: Vec<(&'a Texture, usize)>,
    palettes: Vec<(&'a Palette, usize)>,
}

impl<'a> TextureLib<'a> {
    pub fn new(own: &'a Container, others: &'a [Container]) -> Self {
        let mut lib = TextureLib::default();
        for (file, cont) in std::iter::once(own).chain(others).enumerate() {
            lib.textures.extend(cont.textures.iter().map(|t| (t, file)));
            lib.palettes.extend(cont.palettes.iter().map(|p| (p, file)));
        }
        lib
    }

    fn texture(&self, name: &Name, has_palette: bool) -> Option<(&'a Texture, usize)> {
        let mut candidates = self.textures.iter().filter(|(t, _)| t.name == *name && t.params.format().desc().requires_palette == has_palette);
        let first = candidates.next().copied()?;
        // Les candidats sont dans l'ordre des fichiers : le fichier du modèle d'abord
        Some(first)
    }

    fn palette(&self, name: &Name, texture_file: usize) -> Option<&'a Palette> {
        let named = || self.palettes.iter().filter(|(p, _)| p.name == *name);
        named().find(|(_, f)| *f == texture_file).or_else(|| named().next()).map(|(p, _)| *p)
    }
}

/// Tous les modèles d'un fichier, avec leurs textures.
pub fn build_models(own: &Container, others: &[Container]) -> Vec<ModelOut> {
    let lib = TextureLib::new(own, others);
    own.models.iter().map(|m| build_model(m, &lib)).collect()
}

pub fn build_model(model: &Model, lib: &TextureLib) -> ModelOut {
    let objects: Vec<_> = model.objects.iter().map(|o| o.matrix).collect();
    let uv_mats: Vec<_> = model.materials.iter().map(|m| m.texture_mat).collect();
    let prims = Primitives::build(model, PolyType::Tris, DynamicState { objects: &objects, uv_mats: &uv_mats });

    let mut warnings = Vec::new();
    let materials = model
        .materials
        .iter()
        .map(|mat| {
            let mut texture = None;
            if let Some(tex_name) = &mat.texture_name {
                let has_palette = mat.palette_name.is_some();
                match lib.texture(tex_name, has_palette) {
                    None => warnings.push(format!("texture introuvable : {tex_name}")),
                    Some((tex, file)) => {
                        let palette = mat.palette_name.as_ref().and_then(|p| lib.palette(p, file));
                        if has_palette && palette.is_none() {
                            warnings.push(format!("palette introuvable : {}", mat.palette_name.as_ref().unwrap()));
                        } else {
                            match crate::nds::decode_texture(tex, palette) {
                                Ok(rgba) => {
                                    let (width, height) = tex.params.dim();
                                    texture = Some(Rgba { width, height, data: rgba.0 });
                                }
                                Err(e) => warnings.push(format!("texture {tex_name} : {e}")),
                            }
                        }
                    }
                }
            }
            MaterialOut {
                name: mat.name.to_string(),
                texture,
                texture_name: mat.texture_name.map(|n| n.to_string()),
                repeat_s: mat.params.repeat_s(),
                repeat_t: mat.params.repeat_t(),
                mirror_s: mat.params.mirror_s(),
                mirror_t: mat.params.mirror_t(),
                alpha: mat.alpha,
                cull_back: mat.cull_backface,
            }
        })
        .collect();

    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    let mut parts = Vec::new();
    let mut triangle_count = 0;
    for call in &prims.draw_calls {
        let start = call.vertex_range.start as usize;
        let verts = &prims.vertices[start..call.vertex_range.end as usize];
        if verts.is_empty() || call.index_range.is_empty() {
            continue;
        }
        for v in verts {
            for i in 0..3 {
                min[i] = min[i].min(v.position[i]);
                max[i] = max[i].max(v.position[i]);
            }
        }
        let indices: Vec<u32> = prims.indices[call.index_range.clone()].iter().map(|&i| i as u32 - start as u32).collect();
        triangle_count += indices.len() / 3;
        parts.push(Part {
            positions: verts.iter().map(|v| v.position).collect(),
            normals: verts.iter().map(|v| v.normal).collect(),
            // apicula retourne v (OpenGL, image chargée à l'envers) : on revient au haut de l'image en 0
            uvs: verts.iter().map(|v| [v.texcoord[0], 1.0 - v.texcoord[1]]).collect(),
            colors: verts.iter().map(|v| [v.color[0], v.color[1], v.color[2], 1.0]).collect(),
            indices,
            material: call.mat_id as usize,
            lit: call.used_normals,
        });
    }
    if parts.is_empty() {
        min = [0.0; 3];
        max = [0.0; 3];
    }
    warnings.dedup();
    ModelOut { name: model.name.to_string(), parts, materials, min, max, vertex_count: prims.vertices.len(), triangle_count, warnings }
}

/// Une animation prête à jouer : pour chaque image, positions et normales de chaque morceau (les mêmes
/// morceaux, dans le même ordre, que `build_model`).
pub struct AnimOut {
    pub name: String,
    pub frames: Vec<Vec<(Vec<[f32; 3]>, Vec<[f32; 3]>)>>,
    /// Images (1/60 s) entre deux images gardées : les très longues animations sont réduites.
    pub step: u16,
    pub min: [f32; 3],
    pub max: [f32; 3],
}

const MAX_ANIM_FRAMES: u16 = 240;

/// `None` si l'animation a plus d'os que le modèle. S'il en a moins, les derniers os du modèle restent
/// en place (les os sont liés par leur n°).
pub fn build_animation(model: &Model, animation: &Animation) -> Option<AnimOut> {
    if animation.objects_curves.len() > model.objects.len() || animation.num_frames == 0 {
        return None;
    }
    let uv_mats: Vec<_> = model.materials.iter().map(|m| m.texture_mat).collect();
    let step = animation.num_frames.div_ceil(MAX_ANIM_FRAMES).max(1);
    let (mut min, mut max) = ([f32::MAX; 3], [f32::MIN; 3]);
    let mut frames = Vec::new();
    for frame in (0..animation.num_frames).step_by(step as usize) {
        let moved = animation.objects_curves.iter().map(|c| c.sample_at(frame));
        let objects: Vec<_> = moved.chain(model.objects.iter().skip(animation.objects_curves.len()).map(|o| o.matrix)).collect();
        let prims = Primitives::build(model, PolyType::Tris, DynamicState { objects: &objects, uv_mats: &uv_mats });
        let mut parts = Vec::new();
        for call in &prims.draw_calls {
            let verts = &prims.vertices[call.vertex_range.start as usize..call.vertex_range.end as usize];
            if verts.is_empty() || call.index_range.is_empty() {
                continue;
            }
            for v in verts {
                for i in 0..3 {
                    min[i] = min[i].min(v.position[i]);
                    max[i] = max[i].max(v.position[i]);
                }
            }
            parts.push((verts.iter().map(|v| v.position).collect(), verts.iter().map(|v| v.normal).collect()));
        }
        frames.push(parts);
    }
    Some(AnimOut { name: animation.name.to_string(), frames, step, min, max })
}

/// Toutes les textures d'un fichier (NSBTX ou NSBMD), chacune avec la palette de même nom ou la
/// première venue, pour les montrer à plat.
pub fn texture_sheet(cont: &Container) -> Vec<(String, Result<Rgba>)> {
    cont.textures
        .iter()
        .map(|tex| {
            let needs = tex.params.format().desc().requires_palette;
            let palette = if needs {
                let base = tex.name.to_string();
                cont.palettes.iter().find(|p| p.name.to_string().trim_end_matches("_pl") == base).or(cont.palettes.first())
            } else {
                None
            };
            let image = crate::nds::decode_texture(tex, palette).map(|rgba| {
                let (width, height) = tex.params.dim();
                Rgba { width, height, data: rgba.0 }
            });
            (tex.name.to_string(), image)
        })
        .collect()
}
