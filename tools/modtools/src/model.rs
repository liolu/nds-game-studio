//! Modèles 3D <-> OBJ pour Blender et Godot. L'export écrit un objet par morceau, des triangles. Au retour, les positions et
//! les coordonnées de texture sont reprises coin par coin : le nombre de triangles de chaque morceau ne doit pas changer,
//! car le jeu suppose ses listes de sommets et ses os. Les morceaux qui ont bougé deviennent de nouvelles listes de commandes
//! de la DS, placées à la fin du bloc des modèles ; le reste du fichier ne change pas.

use std::collections::HashMap;
use std::fmt::Write as _;

use cgmath::{vec4, Matrix4, One, Point3, SquareMatrix, Transform, Vector4, Zero};
use nds_nitro::model3d::ModelOut;
use nds_nitro::nds::gpu_cmds::{CmdParser, GpuCmd};
use nds_nitro::nitro::render_cmds::Op;
use nds_nitro::nitro::Model;

use crate::container::{rd16, rd32, wr32};

/// Le nom d'un objet dans l'OBJ : le numéro seul sert au retour, Blender trie par nom et garde celui-ci.
fn object_name(part: usize, material: &str) -> String {
    let clean: String = material.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
    format!("part_{part:03}_{clean}")
}

/// Le fichier `.obj`, et le `.mtl` qui va avec (`textures` = chemin de l'image de chaque matériau).
pub fn write_obj(model: &ModelOut, mtl_name: &str, textures: &[Option<String>]) -> (String, String) {
    let mut obj = format!("# {}\nmtllib {mtl_name}\n", model.name);
    let (mut base_v, mut base_vt, mut base_vn) = (1, 1, 1);
    for (p, part) in model.parts.iter().enumerate() {
        let material = model.materials.get(part.material).map_or("default", |m| m.name.as_str());
        let _ = writeln!(obj, "o {}\nusemtl {}", object_name(p, material), material.replace(' ', "_"));
        for v in &part.positions {
            let _ = writeln!(obj, "v {:.6} {:.6} {:.6}", v[0], v[1], v[2]);
        }
        for t in &part.uvs {
            // Un matériau sans texture divise par une taille nulle
            let finite = |v: f32| if v.is_finite() { v } else { 0.0 };
            let _ = writeln!(obj, "vt {:.6} {:.6}", finite(t[0]), finite(1.0 - t[1]));
        }
        if part.lit {
            for n in &part.normals {
                let _ = writeln!(obj, "vn {:.6} {:.6} {:.6}", n[0], n[1], n[2]);
            }
        }
        for face in part.indices.chunks_exact(3) {
            obj.push('f');
            for &i in face {
                let (v, vt) = (base_v + i, base_vt + i);
                if part.lit {
                    let _ = write!(obj, " {v}/{vt}/{}", base_vn + i);
                } else {
                    let _ = write!(obj, " {v}/{vt}");
                }
            }
            obj.push('\n');
        }
        base_v += part.positions.len() as u32;
        base_vt += part.uvs.len() as u32;
        if part.lit {
            base_vn += part.normals.len() as u32;
        }
    }
    let mut mtl = String::new();
    for (m, material) in model.materials.iter().enumerate() {
        let _ = writeln!(mtl, "newmtl {}\nKd 1 1 1\nd {:.3}", material.name.replace(' ', "_"), material.alpha);
        if let Some(Some(path)) = textures.get(m) {
            let _ = writeln!(mtl, "map_Kd {path}");
        }
        mtl.push('\n');
    }
    (obj, mtl)
}

/// Un objet d'un OBJ lu : ses triangles, chaque coin étant (position, coordonnée de texture).
struct ObjPart {
    number: usize,
    corners: Vec<([f32; 3], Option<[f32; 2]>)>,
}

fn parse_obj(text: &str) -> Result<Vec<ObjPart>, String> {
    let (mut positions, mut uvs) = (Vec::<[f32; 3]>::new(), Vec::<[f32; 2]>::new());
    let mut parts: Vec<ObjPart> = Vec::new();
    let floats = |rest: &mut std::str::SplitWhitespace, n: usize| -> Result<Vec<f32>, String> { (0..n).map(|_| rest.next().and_then(|v| v.parse().ok()).ok_or_else(|| "nombre illisible".to_string())).collect() };
    for line in text.lines() {
        let mut words = line.split_whitespace();
        match words.next() {
            Some("v") => {
                let v = floats(&mut words, 3)?;
                positions.push([v[0], v[1], v[2]]);
            }
            Some("vt") => {
                let v = floats(&mut words, 2)?;
                uvs.push([v[0], v[1]]);
            }
            Some("o" | "g") => {
                let name = words.next().unwrap_or("");
                let number = name.split("part_").nth(1).and_then(|rest| rest.get(..3)).and_then(|n| n.parse().ok());
                let number = number.ok_or_else(|| format!("objet {name} : le nom part_NNN doit rester"))?;
                if parts.last().is_none_or(|p: &ObjPart| p.number != number) {
                    parts.push(ObjPart { number, corners: Vec::new() });
                }
            }
            Some("f") => {
                let part = parts.last_mut().ok_or("une face hors de tout objet")?;
                let corners: Vec<&str> = words.collect();
                if corners.len() != 3 {
                    return Err("seuls les triangles reviennent : exporter avec Triangulate".into());
                }
                for corner in corners {
                    let mut numbers = corner.split('/');
                    let index = |word: Option<&str>, len: usize| -> Option<usize> {
                        let n: i64 = word?.parse().ok()?;
                        if n < 0 { (len as i64 + n).try_into().ok() } else { (n - 1).try_into().ok() }
                    };
                    let v = index(numbers.next(), positions.len()).and_then(|i| positions.get(i)).ok_or("sommet inconnu")?;
                    let t = index(numbers.next(), uvs.len()).and_then(|i| uvs.get(i)).copied();
                    part.corners.push((*v, t));
                }
            }
            _ => {}
        }
    }
    Ok(parts)
}

/// Une commande de la DS telle qu'elle est écrite.
struct RawCmd {
    opcode: u8,
    params: Vec<u32>,
}

fn param_count(opcode: u8) -> Result<usize, String> {
    const SIZES: [i8; 66] = [0, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 1, 0, 1, 1, 1, 0, 16, 12, 16, 12, 9, 3, 3, -1, -1, -1, 1, 1, 1, 2, 1, 1, 1, 1, 1, 1, 1, 1, -1, -1, -1, -1, 1, 1, 1, 1, 1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 1, 0];
    match SIZES.get(opcode as usize) {
        Some(n) if *n >= 0 => Ok(*n as usize),
        _ => Err(format!("commande {opcode:#x} inconnue")),
    }
}

fn read_cmds(bytes: &[u8]) -> Result<Vec<RawCmd>, String> {
    let mut cmds = Vec::new();
    let mut at = 0;
    while at + 4 <= bytes.len() {
        let opcodes = [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]];
        at += 4;
        for opcode in opcodes {
            let count = param_count(opcode)?;
            let params = (0..count).map(|i| rd32(bytes, at + i * 4) as u32).collect();
            at += count * 4;
            cmds.push(RawCmd { opcode, params });
        }
    }
    Ok(cmds)
}

fn write_cmds(cmds: &[RawCmd]) -> Vec<u8> {
    let mut out = Vec::new();
    for group in cmds.chunks(4) {
        for i in 0..4 {
            out.push(group.get(i).map_or(0, |c| c.opcode));
        }
        for cmd in group {
            for param in &cmd.params {
                out.extend(param.to_le_bytes());
            }
        }
    }
    out
}

/// Ce qu'il faut pour écrire les sommets d'un morceau : la matrice courante au moment où le morceau est dessiné.
struct Draw {
    piece: usize,
    material: usize,
    matrix: Matrix4<f64>,
    stack: Vec<Matrix4<f64>>,
}

/// Rejoue les opérations du modèle, comme `Primitives::build`.
fn draws(model: &Model) -> Vec<Draw> {
    let mut stack = vec![Matrix4::<f64>::one(); 32];
    let mut matrix = Matrix4::<f64>::one();
    let mut material = 0;
    let mut out = Vec::new();
    for op in &model.render_ops {
        match op {
            Op::LoadMatrix { stack_pos } => matrix = stack[*stack_pos as usize],
            Op::StoreMatrix { stack_pos } => stack[*stack_pos as usize] = matrix,
            Op::MulObject { object_idx } => matrix = matrix * model.objects[*object_idx as usize].matrix,
            Op::Skin { terms } => {
                let mut blended = Matrix4::<f64>::zero();
                for term in terms.iter() {
                    blended += term.weight as f64 * stack[term.stack_pos as usize] * model.inv_binds[term.inv_bind_idx as usize];
                }
                matrix = blended;
            }
            Op::ScaleUp => matrix = matrix * Matrix4::from_scale(model.up_scale),
            Op::ScaleDown => matrix = matrix * Matrix4::from_scale(model.down_scale),
            Op::BindMaterial { material_idx } => material = *material_idx as usize,
            Op::Draw { piece_idx } => out.push(Draw { piece: *piece_idx as usize, material, matrix, stack: stack.clone() }),
        }
        // Une liste de commandes peut changer la matrice : au morceau suivant, `build` repart de ce qu'elle a laissé
        if let Op::Draw { piece_idx } = op {
            for cmd in CmdParser::new(&model.pieces[*piece_idx as usize].gpu_commands).flatten() {
                match cmd {
                    GpuCmd::Restore { idx } => matrix = stack[idx as usize],
                    GpuCmd::Scale { scale } => matrix = matrix * Matrix4::from_nonuniform_scale(scale.0, scale.1, scale.2),
                    _ => {}
                }
            }
        }
    }
    out
}

fn to_fixed(value: f64, fraction: i32, warned: &mut bool) -> u32 {
    let scaled = (value * 2f64.powi(fraction)).round();
    if !(-32768.0..=32767.0).contains(&scaled) {
        *warned = true;
    }
    (scaled.clamp(-32768.0, 32767.0) as i32 as u32) & 0xFFFF
}

const VTX_16: u8 = 0x23;
const TEXCOORD: u8 = 0x22;

/// L'OBJ garde 6 décimales de nombres en simple précision : en dessous de cet écart, la valeur n'a pas changé.
fn moved(old: f32, new: f32) -> bool {
    (old - new).abs() > 2e-5 + old.abs() * 2e-6
}

/// La nouvelle liste de commandes d'un morceau. Tous ses sommets passent en position absolue (`VTX_16`), car les autres formes
/// dépendent du sommet d'avant. `new` donne, pour chaque sommet, ce qui a changé. `None` si rien n'a bougé.
#[allow(clippy::type_complexity)]
fn rewrite_piece(model: &Model, draw: &Draw, new: &[(Option<[f32; 3]>, Option<[f32; 2]>)], warned: &mut bool) -> Result<Option<Vec<u8>>, String> {
    let bytes = &model.pieces[draw.piece].gpu_commands;
    let material = &model.materials[draw.material];
    let (tex_w, tex_h) = (material.width as f64, material.height as f64);
    let inverse_uv = material.texture_mat.invert().unwrap_or_else(Matrix4::one);
    let raw = read_cmds(bytes)?;
    let mut parser = CmdParser::new(bytes);
    let mut matrix = draw.matrix;
    let mut next = 0;
    let mut changed = false;
    let mut out = Vec::with_capacity(raw.len());
    let mut current_uv: Option<[f64; 2]> = None;
    for cmd in raw {
        let parsed = parser.next().ok_or("liste de commandes coupee")?.map_err(|e| e.to_string())?;
        match parsed {
            GpuCmd::Restore { idx } => matrix = draw.stack[idx as usize],
            GpuCmd::Scale { scale } => matrix = matrix * Matrix4::from_nonuniform_scale(scale.0, scale.1, scale.2),
            GpuCmd::TexCoord { texcoord } => current_uv = Some([texcoord.x, texcoord.y]),
            _ => {}
        }
        if let GpuCmd::Vertex { position } = parsed {
            let (wanted, uv) = new.get(next).copied().unwrap_or((None, None));
            next += 1;
            if let (Some(uv), true) = (uv, current_uv.is_some() && tex_w > 0.0 && tex_h > 0.0) {
                let texcoord: Vector4<f64> = inverse_uv * vec4(uv[0] as f64 * tex_w, uv[1] as f64 * tex_h, 0.0, 1.0);
                changed = true;
                let (s, t) = (to_fixed(texcoord.x, 4, warned), to_fixed(texcoord.y, 4, warned));
                out.push(RawCmd { opcode: TEXCOORD, params: vec![s | (t << 16)] });
            }
            let local = wanted
                .map(|w| {
                    changed = true;
                    let inverse = matrix.invert().unwrap_or_else(Matrix4::one);
                    inverse.transform_point(Point3::new(w[0] as f64, w[1] as f64, w[2] as f64))
                })
                .unwrap_or(position);
            let (x, y, z) = (to_fixed(local.x, 12, warned), to_fixed(local.y, 12, warned), to_fixed(local.z, 12, warned));
            out.push(RawCmd { opcode: VTX_16, params: vec![x | (y << 16), z] });
            continue;
        }
        if cmd.opcode != 0 {
            out.push(cmd);
        }
    }
    Ok(changed.then(|| write_cmds(&out)))
}

/// Début des données d'un bloc d'informations (`info_block`) et nombre d'éléments.
fn info_block(file: &[u8], at: usize) -> (usize, usize) {
    let count = file[at + 1] as usize;
    (at + 12 + 4 * count + 4, count)
}

/// Le fichier du modèle avec les positions et les coordonnées de texture de l'OBJ. Rend aussi `true` si un sommet sort de ce que
/// la DS sait écrire (plus ou moins 8 unités, avant l'échelle du modèle).
pub fn import_obj(file: &[u8], model: &Model, index: usize, built: &ModelOut, text: &str) -> Result<(Vec<u8>, bool), String> {
    let parts = parse_obj(text)?;
    if parts.len() != built.parts.len() {
        return Err(format!("{} objets dans l'OBJ, {} attendus", parts.len(), built.parts.len()));
    }
    let all_draws = draws(model);
    // Les mêmes morceaux que `build_model`, qui laisse de côté les vides
    let mut counted = Vec::new();
    for draw in &all_draws {
        let vertices = CmdParser::new(&model.pieces[draw.piece].gpu_commands).flatten().filter(|c| matches!(c, GpuCmd::Vertex { .. })).count();
        if vertices > 0 {
            counted.push((draw, vertices));
        }
    }
    let mut new_lists: HashMap<usize, Vec<u8>> = HashMap::new();
    let mut warned = false;
    let mut part_at = 0;
    for (draw, vertices) in counted {
        let Some(part) = built.parts.get(part_at) else { break };
        if part.positions.len() != vertices {
            continue;
        }
        let obj = parts.iter().find(|p| p.number == part_at).ok_or_else(|| format!("objet part_{part_at:03} absent"))?;
        part_at += 1;
        if obj.corners.len() != part.indices.len() {
            return Err(format!("part_{:03} : {} triangles, {} attendus. Bouger les sommets oui, en ajouter non.", obj.number, obj.corners.len() / 3, part.indices.len() / 3));
        }
        let mut new = vec![(None, None); vertices];
        for (corner, &vertex) in obj.corners.iter().zip(&part.indices) {
            let vertex = vertex as usize;
            let (old_position, old_uv) = (part.positions[vertex], part.uvs[vertex]);
            if (0..3).any(|i| moved(old_position[i], corner.0[i])) {
                new[vertex].0 = Some(corner.0);
            }
            // Comme à l'export, une valeur impossible (matériau sans texture) vaut 0
            let finite = |v: f32| if v.is_finite() { v } else { 0.0 };
            if let Some(t) = corner.1.map(|t| [t[0], 1.0 - t[1]]) {
                if moved(finite(old_uv[0]), t[0]) || moved(finite(old_uv[1]), t[1]) {
                    new[vertex].1 = Some(t);
                }
            }
        }
        // Un morceau dessiné plusieurs fois n'a qu'une liste : le premier dessin décide
        if new_lists.contains_key(&draw.piece) {
            continue;
        }
        if let Some(list) = rewrite_piece(model, draw, &new, &mut warned)? {
            new_lists.insert(draw.piece, list);
        }
    }
    Ok((attach_lists(file, index, &new_lists)?, warned))
}

/// Ajoute les nouvelles listes à la fin du bloc MDL0 et y fait pointer les morceaux. Les blocs suivants (TEX0) sont décalés.
fn attach_lists(file: &[u8], index: usize, lists: &HashMap<usize, Vec<u8>>) -> Result<Vec<u8>, String> {
    if lists.is_empty() {
        return Ok(file.to_vec());
    }
    let sections = rd16(file, 14);
    let mdl = (0..sections).map(|i| rd32(file, 16 + i * 4)).find(|at| file.get(*at..*at + 4) == Some(b"MDL0")).ok_or("bloc MDL0 absent")?;
    let mdl_end = mdl + rd32(file, mdl + 4);
    let mut added = Vec::new();
    let mut out = file.to_vec();
    let (models_at, model_count) = info_block(file, mdl + 8);
    if index >= model_count {
        return Err(format!("modele {index} absent"));
    }
    let model_at = mdl + rd32(file, models_at + index * 4);
    let pieces_table = model_at + rd32(file, model_at + 12);
    let (pieces_at, piece_count) = info_block(file, pieces_table);
    let mut ordered: Vec<_> = lists.iter().collect();
    ordered.sort_by_key(|(piece, _)| **piece);
    for (piece, list) in ordered {
        if *piece >= piece_count {
            return Err(format!("morceau {piece} absent"));
        }
        let piece_at = pieces_table + rd32(file, pieces_at + piece * 4);
        wr32(&mut out, piece_at + 8, mdl_end + added.len() - piece_at);
        wr32(&mut out, piece_at + 12, list.len());
        added.extend(list);
    }
    let size = rd32(file, mdl + 4) + added.len();
    wr32(&mut out, mdl + 4, size);
    let shift = added.len();
    out.splice(mdl_end..mdl_end, added);
    for i in 0..sections {
        let at = rd32(file, 16 + i * 4);
        if at >= mdl_end {
            wr32(&mut out, 16 + i * 4, at + shift);
        }
    }
    let total = out.len();
    wr32(&mut out, 8, total);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nds_nitro::model3d;

    /// Le premier NSBMD lisible d'une rom de test, si `NDSGS_TEST_MODEL` en donne un.
    fn real_model() -> Option<Vec<u8>> {
        std::fs::read(std::env::var_os("NDSGS_TEST_MODEL")?).ok()
    }

    #[test]
    fn commands_are_grouped_by_four() {
        let cmds = vec![RawCmd { opcode: 0x40, params: vec![1] }, RawCmd { opcode: VTX_16, params: vec![2, 3] }, RawCmd { opcode: 0x41, params: vec![] }];
        let bytes = write_cmds(&cmds);
        assert_eq!(bytes[..4], [0x40, VTX_16, 0x41, 0]);
        assert_eq!(bytes.len(), 4 + 12);
        let back = read_cmds(&bytes).unwrap();
        assert_eq!(back.len(), 4);
        assert_eq!(back[1].params, [2, 3]);
    }

    #[test]
    fn objects_keep_their_number() {
        let text = "o part_002_mat\nv 0 0 0\nv 1 0 0\nv 0 1 0\nvt 0.5 0.5\nf 1/1 2/1 3/1\n";
        let parts = parse_obj(text).unwrap();
        assert_eq!(parts[0].number, 2);
        assert_eq!(parts[0].corners[1].0, [1.0, 0.0, 0.0]);
        assert!(parse_obj("o autre\nv 0 0 0\n").is_err());
        assert!(parse_obj("o part_000\nv 0 0 0\nf 1 1 1 1\n").is_err());
    }

    #[test]
    fn a_real_model_comes_back_moved() {
        let Some(file) = real_model() else { return };
        let cont = model3d::read(&file).unwrap();
        let built = model3d::build_models(&cont, &[]);
        let (obj, _) = write_obj(&built[0], "m.mtl", &[]);
        // Sans changement, le fichier reste identique à l'octet près
        let (same, _) = import_obj(&file, &cont.models[0], 0, &built[0], &obj).unwrap();
        assert!(same == file, "aucun morceau ne doit bouger");
        // Tout monte d'une unité
        let moved: String = obj.lines().map(|l| match l.strip_prefix("v ") {
            Some(rest) => {
                let v: Vec<f32> = rest.split(' ').map(|x| x.parse().unwrap()).collect();
                format!("v {} {} {}\n", v[0], v[1] + 1.0, v[2])
            }
            None => format!("{l}\n"),
        }).collect();
        let (out, _) = import_obj(&file, &cont.models[0], 0, &built[0], &moved).unwrap();
        let back = model3d::read(&out).unwrap();
        let again = model3d::build_models(&back, &[]);
        assert_eq!(back.textures.len(), cont.textures.len(), "les textures sont decalees, pas perdues");
        for (a, b) in built[0].parts.iter().zip(&again[0].parts) {
            assert_eq!(a.positions.len(), b.positions.len());
            for (p, q) in a.positions.iter().zip(&b.positions) {
                assert!((p[1] + 1.0 - q[1]).abs() < 0.01 && (p[0] - q[0]).abs() < 0.01, "{p:?} -> {q:?}");
            }
        }
    }
}
