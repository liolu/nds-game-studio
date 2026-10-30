//! Maillages du moteur maison (Rayman Raving Rabbids 2, Battle of Giants, Star Wars III...) : un `GEOM` est
//! une simple liste de commandes de la puce graphique de la DS (taille, puis commandes), sans squelette.

use cgmath::{EuclideanSpace, InnerSpace};

use crate::errors::Result;
use crate::model3d::{MaterialOut, ModelOut, Part};
use crate::nds::gpu_cmds::{CmdParser, GpuCmd};
use crate::Rgba;

/// Les triangles d'un groupe de sommets, selon son type : 0 triangles, 1 quadrilatères, 2 bande de
/// triangles, 3 bande de quadrilatères.
fn triangles(prim_type: u32, first: u32, end: u32, indices: &mut Vec<u32>) {
    let count = end - first;
    match prim_type {
        0 => indices.extend(first..first + count / 3 * 3),
        1 => {
            for q in 0..count / 4 {
                let a = first + q * 4;
                indices.extend([a, a + 1, a + 2, a, a + 2, a + 3]);
            }
        }
        2 => {
            for t in 0..count.saturating_sub(2) {
                let a = first + t;
                if t % 2 == 0 {
                    indices.extend([a, a + 1, a + 2]);
                } else {
                    indices.extend([a + 1, a, a + 2]);
                }
            }
        }
        3 => {
            for q in 0..count.saturating_sub(2) / 2 {
                let a = first + q * 2;
                indices.extend([a, a + 1, a + 3, a, a + 3, a + 2]);
            }
        }
        _ => {}
    }
}

/// Le modèle d'un `GEOM`, avec sa texture si on l'a.
pub fn read_geometry(file: &[u8], texture: Option<Rgba>, name: &str) -> Result<ModelOut> {
    if file.get(0..4) != Some(b"MOEG") {
        bail!("pas un fichier GEOM");
    }
    let Some(size) = file.get(4..8).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize) else { bail!("GEOM trop court") };
    read_commands(file.get(8..8 + size).unwrap_or_else(|| file.get(8..).unwrap_or(&[])), texture, name)
}

/// Un maillage du monde de Tom and Jerry (`.z` décompressé) : échelle en 20.12, 2 u16, boîte (6 i16), nombre de morceaux,
/// leurs positions depuis cette table. Chaque morceau : hauteur et profondeur de son centre (2 i32), un u32, sa boîte
/// (6 i16), taille, commandes GX.
pub fn read_world_mesh(file: &[u8], name: &str, mut texture_of: impl FnMut(usize) -> Option<Rgba>) -> Option<ModelOut> {
    const MAX_PARTS: usize = 4096;
    const TABLE_AT: usize = 0x18;
    let u32_at = |at: usize| file.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]));
    const MAX_SCALE: u32 = 0x100000;
    let scale = u32_at(0)?;
    if scale == 0 || scale > MAX_SCALE || scale % 0x1000 != 0 {
        return None;
    }
    let scale = scale as f32 / 4096.0;
    let count = u32_at(0x14)? as usize;
    if count == 0 || count > MAX_PARTS {
        return None;
    }
    let mut models = Vec::with_capacity(count);
    for i in 0..count {
        let base = TABLE_AT + u32_at(TABLE_AT + i * 4)? as usize;
        let center = [0, u32_at(base)? as i32, u32_at(base + 4)? as i32].map(|v| v as f32 / 4096.0);
        let size = u32_at(base + 0x18)? as usize;
        let Ok(mut model) = read_commands(file.get(base + 0x1C..base + 0x1C + size)?, texture_of(i), name) else { continue };
        let place = |at: f32, axis: usize| (at + center[axis]) * scale;
        for part in &mut model.parts {
            for position in &mut part.positions {
                *position = [place(position[0], 0), place(position[1], 1), place(position[2], 2)];
            }
        }
        for axis in 0..3 {
            model.min[axis] = place(model.min[axis], axis);
            model.max[axis] = place(model.max[axis], axis);
        }
        models.push(model);
    }
    merge(name, models)
}

/// Le modèle que dessine une liste de commandes GX.
fn read_commands(cmds: &[u8], texture: Option<Rgba>, name: &str) -> Result<ModelOut> {
    let (tex_w, tex_h) = texture.as_ref().map_or((1.0, 1.0), |t| (t.width.max(1) as f64, t.height.max(1) as f64));

    let mut part = Part { positions: Vec::new(), normals: Vec::new(), uvs: Vec::new(), colors: Vec::new(), indices: Vec::new(), material: 0, lit: false };
    let mut scale = (1.0, 1.0, 1.0);
    let (mut uv, mut normal, mut color) = ([0.0f32; 2], [0.0f32, 0.0, 0.0], [1.0f32, 1.0, 1.0, 1.0]);
    let (mut prim, mut first) = (0, 0u32);
    let (mut min, mut max) = ([f32::MAX; 3], [f32::MIN; 3]);
    for cmd in CmdParser::new(cmds) {
        let Ok(cmd) = cmd else { break };
        match cmd {
            GpuCmd::Scale { scale: (x, y, z) } => scale = (scale.0 * x, scale.1 * y, scale.2 * z),
            GpuCmd::Begin { prim_type } => {
                triangles(prim, first, part.positions.len() as u32, &mut part.indices);
                prim = prim_type;
                first = part.positions.len() as u32;
            }
            GpuCmd::End => {
                triangles(prim, first, part.positions.len() as u32, &mut part.indices);
                first = part.positions.len() as u32;
            }
            GpuCmd::TexCoord { texcoord } => uv = [(texcoord.x / tex_w) as f32, (texcoord.y / tex_h) as f32],
            GpuCmd::Color { color: c } => {
                color = [c.x, c.y, c.z, 1.0];
                normal = [0.0; 3];
            }
            GpuCmd::Normal { normal: n } => {
                let n = n.normalize();
                normal = [n.x as f32, n.y as f32, n.z as f32];
                color = [1.0; 4];
                part.lit = true;
            }
            GpuCmd::Vertex { position } => {
                let p = position.to_vec();
                let at = [(p.x * scale.0) as f32, (p.y * scale.1) as f32, (p.z * scale.2) as f32];
                for i in 0..3 {
                    min[i] = min[i].min(at[i]);
                    max[i] = max[i].max(at[i]);
                }
                part.positions.push(at);
                part.normals.push(normal);
                part.uvs.push(uv);
                part.colors.push(color);
            }
            _ => {}
        }
    }
    triangles(prim, first, part.positions.len() as u32, &mut part.indices);
    if part.indices.is_empty() {
        bail!("GEOM sans triangles");
    }
    let (vertex_count, triangle_count) = (part.positions.len(), part.indices.len() / 3);
    let material = MaterialOut { name: name.to_string(), texture, texture_name: None, repeat_s: true, repeat_t: true, mirror_s: false, mirror_t: false, alpha: 1.0, cull_back: false };
    Ok(ModelOut { name: name.to_string(), parts: vec![part], materials: vec![material], min, max, vertex_count, triangle_count, warnings: Vec::new() })
}

/// Réunit les morceaux d'un même modèle.
pub fn merge(name: &str, models: Vec<ModelOut>) -> Option<ModelOut> {
    let mut models = models.into_iter();
    let mut all = models.next()?;
    all.name = name.to_string();
    for model in models {
        let offset = all.materials.len();
        all.parts.extend(model.parts.into_iter().map(|mut part| {
            part.material += offset;
            part
        }));
        all.materials.extend(model.materials);
        for i in 0..3 {
            all.min[i] = all.min[i].min(model.min[i]);
            all.max[i] = all.max[i].max(model.max[i]);
        }
        all.vertex_count += model.vertex_count;
        all.triangle_count += model.triangle_count;
    }
    Some(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_the_parts_of_a_world_mesh() {
        // Un triangle (BEGIN, 3 VTX_16) : centre à y = 1, échelle 2
        let cmds = [[0x40u8, 0x23, 0x23, 0x23].as_slice(), &0u32.to_le_bytes(), &[0, 0, 0, 0, 0, 0, 0, 0], &[0, 0x10, 0, 0, 0, 0, 0, 0], &[0, 0, 0, 0x10, 0, 0, 0, 0]].concat();
        let mut file = vec![0u8; 0x18];
        file[..4].copy_from_slice(&0x2000u32.to_le_bytes());
        file[0x14] = 1;
        file.extend(4u32.to_le_bytes());
        file.extend(0x1000i32.to_le_bytes());
        file.extend([0; 8 + 12]);
        file.extend((cmds.len() as u32).to_le_bytes());
        file.extend(&cmds);
        let model = read_world_mesh(&file, "x", |_| None).unwrap();
        assert_eq!(model.parts[0].positions[0], [0.0, 2.0, 0.0]);
        assert_eq!(model.parts[0].positions[1], [2.0, 2.0, 0.0]);
        assert_eq!(model.max[1], 4.0);
    }

    #[test]
    fn assembles_triangles_from_each_group_type() {
        let mut out = Vec::new();
        triangles(1, 0, 4, &mut out);
        assert_eq!(out, [0, 1, 2, 0, 2, 3]);
        out.clear();
        triangles(2, 0, 4, &mut out);
        assert_eq!(out, [0, 1, 2, 2, 1, 3]);
        out.clear();
        triangles(3, 0, 4, &mut out);
        assert_eq!(out, [0, 1, 3, 0, 3, 2]);
    }

    #[test]
    fn refuses_other_files() {
        assert!(read_geometry(b"pas ca", None, "x").is_err());
    }
}
