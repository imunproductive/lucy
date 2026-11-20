//! The collision caches `collision/<map>.col` (format 6): a map's static
//! collision as the character mover sees it (`tools/build_collision.py`,
//! `ow174/game/collision.py`), and `collision/<map>.rays.col`, the same
//! with every category, as the shots see it (`--ray-geometry`). The
//! server's reader is `orpheus_physics::collision::cache`.
//!
//! Where it comes from (the client's map chunk 0x11): each map has one chunk
//! of kind 0x11 (see [`crate::map_chunk`]). It holds the game's own physics
//! shapes:
//! - `u32 count[4], u64 offset[4]`: slot 2 convex polytopes, slot 3 triangle
//!   meshes;
//! - mesh (56 B each): `u64 nodes, verts, tris; u32 node count, vert count,
//!   tri count, depth; f32 area; u32 0, root mask, 0`; vertex `f32 x, y, z, 0`;
//!   triangle (32 B) `u32 v[3], i32 neighbour[3], u16 material, u16 surface,
//!   u16 mask, u16 0`; the BVH nodes are not needed here;
//! - polytope (88 B): `u64 verts, planes, half edges, faces; f32
//!   centroid[3]; u32 vert, face, edge count; f32 volume, area, 3 x f32; u16
//!   mask, u16 material, u32 flag, u32 0`; vertices `f32[3]`; faces `u8 first
//!   edge`; half edges `{s8 twin, u8 origin, u8 face, u8 next}`.
//!
//! A triangle's mask is the query categories it stops. The mover queries
//! with its team's proxy bit (0x08 team 0, 0x10 team 1), so only triangles
//! with one of them matter: about a sixth of a map's triangles (the movement
//! hull). A triangle whose surface has bit 0 is not ground (the client's
//! probe skips it).
//!
//! The chunk does not hold the props the client makes from the map's entity
//! placeables (fences, crates, clip boxes): their models' physics shapes are
//! added from [`crate::props`], each with the placeable's identifier when it
//! has one, since such a prop exists only in the game modes that list it.
//!
//! The cache keeps the shapes the way the client's physics engine has them:
//! each mesh's triangles in chunk order with their wing vertices, the
//! polytopes whole, the props as their models' shapes placed with the
//! placeable's translation, rotation and scale.
//!
//! The ray cache (Python's `--ray-geometry` side cache) keeps every shape of
//! every category, the props' static shapes too, and says so in its header
//! (`geometry`, `category_mask`, `prop_scope`). The shots and projectiles
//! query it with their ray profile's map mask: about six times the
//! movement cache's triangles, the visual detail the mover's hull leaves
//! out (refs report `WeaponRays.md`).

use std::io::Write;

use flate2::{Compression, write::ZlibEncoder};

use crate::{
    Install, Output, Result, asset_error, guid_name,
    map_chunk::{COLLISION, chunk_guid, read_chunk},
    props::{self, Hull, MOVER, Placed, Record, Shape},
};

const MAGIC: &[u8; 8] = b"OW174COL";
/// 2: props with their identifiers; 3: no client-only breakables; 4: the
/// engine's shapes; 5: only the props' static shapes, at their nodes; 6: the
/// shapes packed with zlib rather than Python's XZ, which the server unpacks
/// about ten times faster (the server still reads 5).
const VERSION: u32 = 6;
/// The Practice Range, the map the runtime pack ships a cache for.
const PRACTICE_RANGE: u64 = 0x0800_0000_0000_0688;
const PRACTICE_RANGE_NAME: &str = "Practice Range";
/// The ray cache's header marks (collision.py `RAY_GEOMETRY`,
/// `ALL_CATEGORIES`, `RAY_PROP_SCOPE`), which the server's reader checks.
const RAY_GEOMETRY: &str = "expanded-static-rays";
const ALL_CATEGORIES: u16 = 0xFFFF;
const RAY_PROP_SCOPE: &str = "indexed-static-models";

/// Which shapes a cache keeps.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Geometry {
    /// Those that stop the mover ([`MOVER`]).
    Movement,
    /// Every one ([`ALL_CATEGORIES`]).
    Rays,
}

impl Geometry {
    fn category(self) -> u16 {
        match self {
            Self::Movement => MOVER,
            Self::Rays => ALL_CATEGORIES,
        }
    }
}

/// The derivation: the Practice Range's cache, the only one the runtime
/// pack has; the server builds the others on demand with [`collision_cache`].
pub(crate) fn derive(install: &Install, _out: &std::path::Path) -> Result<Vec<Output>> {
    Ok(vec![collision_cache(
        install,
        PRACTICE_RANGE,
        PRACTICE_RANGE_NAME,
    )?])
}

/// The derivation of the Practice Range's ray cache; the server builds the
/// others on demand with [`ray_cache`].
pub(crate) fn derive_rays(install: &Install, _out: &std::path::Path) -> Result<Vec<Output>> {
    Ok(vec![ray_cache(
        install,
        PRACTICE_RANGE,
        PRACTICE_RANGE_NAME,
    )?])
}

/// One map's movement collision cache, `collision/<index>.col`, named in its
/// header (the server's map name).
pub fn collision_cache(install: &Install, map: u64, name: &str) -> Result<Output> {
    cache(install, map, name, Geometry::Movement)
}

/// One map's ray cache, `collision/<index>.rays.col`: every shape of the
/// chunk and the props' static shapes, whatever their masks (collision.py
/// `build(..., ray_geometry=True)`).
pub fn ray_cache(install: &Install, map: u64, name: &str) -> Result<Output> {
    cache(install, map, name, Geometry::Rays)
}

fn cache(install: &Install, map: u64, name: &str, geometry: Geometry) -> Result<Output> {
    let chunk = chunk_guid(map, COLLISION);
    let data = read_chunk(install, map, COLLISION)?
        .ok_or_else(|| asset_error(format!("no collision chunk for map {map:#x}")))?;
    let ckey = install
        .game()
        .asset(chunk)
        .map(|asset| {
            asset
                .ckey
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        })
        .expect("a chunk that reads has a manifest record");
    let category = geometry.category();
    let (meshes, hulls) = parse_chunk(&data, category)
        .map_err(|reason| asset_error(format!("{}: {reason}", guid_name(chunk))))?;
    let (models, placed) = props::map_shapes(install, map, category)
        .map_err(|error| asset_error(format!("the props of map {map:#x}: {error}")))?;
    let mut identifiers: Vec<u64> = placed
        .iter()
        .map(|placed| placed.1)
        .filter(|id| *id != 0)
        .collect();
    identifiers.sort_unstable();
    identifiers.dedup();
    let source = Source {
        map,
        chunk,
        ckey,
        props: placed.len(),
        geometry,
    };
    let suffix = match geometry {
        Geometry::Movement => "col",
        Geometry::Rays => "rays.col",
    };
    Ok(Output {
        path: format!("collision/{:04X}.{suffix}", map & 0xFFFF_FFFF),
        bytes: pack(
            name,
            &meshes,
            &hulls,
            &source,
            &models,
            &placed,
            &identifiers,
        )?,
    })
}

/// A mesh of the chunk: its kept vertices and triangles, vertex numbers into
/// the kept vertices.
type Mesh = (Vec<[f32; 3]>, Vec<Record>);

/// The shapes of a collision chunk that stop the category: (meshes,
/// polytopes). A mesh keeps only its triangles that stop it, in chunk order,
/// its vertices renumbered in the order those triangles name them (corners
/// and wings; a wing out of the mesh is -1).
fn parse_chunk(data: &[u8], category: u16) -> Result<(Vec<Mesh>, Vec<Hull>), String> {
    let u16_at = |at: usize| -> Result<u16, String> { Ok(u16::from_le_bytes(take(data, at)?)) };
    let u32_at = |at: usize| -> Result<u32, String> { Ok(u32::from_le_bytes(take(data, at)?)) };
    let u64_at = |at: usize| -> Result<usize, String> {
        usize::try_from(u64::from_le_bytes(take(data, at)?))
            .map_err(|_| "offset out of range".into())
    };
    let f32_at = |at: usize| -> Result<f32, String> { Ok(f32::from_le_bytes(take(data, at)?)) };
    let point = |at: usize| -> Result<[f32; 3], String> {
        Ok([f32_at(at)?, f32_at(at + 4)?, f32_at(at + 8)?])
    };
    let (hull_count, mesh_count) = (u32_at(8)? as usize, u32_at(12)? as usize);
    let (hull_table, mesh_table) = (u64_at(32)?, u64_at(40)?);
    let mut hulls = Vec::new();
    for number in 0..hull_count {
        let base = hull_table + 88 * number;
        let [verts, planes, edges, faces] = [0, 8, 16, 24].map(|at| u64_at(base + at));
        let (verts, planes, edges, faces) = (verts?, planes?, edges?, faces?);
        let centroid = point(base + 32)?;
        let vert_count = u32_at(base + 44)? as usize;
        let face_count = u32_at(base + 48)? as usize;
        let edge_count = u32_at(base + 52)? as usize;
        let mask = u16_at(base + 76)?;
        if mask & category == 0 {
            continue;
        }
        let points = (0..vert_count)
            .map(|n| point(verts + 12 * n))
            .collect::<Result<Vec<_>, _>>()?;
        let half = (0..edge_count)
            .map(|n| take::<4>(data, edges + 4 * n))
            .collect::<Result<Vec<_>, _>>()?;
        let face_list = data
            .get(faces..faces + face_count)
            .ok_or("polytope faces run past the chunk")?
            .to_vec();
        for (face, first) in face_list.iter().enumerate() {
            let mut edge = *first;
            for _ in 0..edge_count {
                let [_twin, _origin, owner, following] = *half
                    .get(edge as usize)
                    .ok_or("half edge out of the polytope")?;
                if owner as usize != face {
                    return Err("polytope face loop leaves its face".into());
                }
                edge = following;
                if edge == *first {
                    break;
                }
            }
        }
        let plane_list = (0..face_count)
            .map(|n| {
                let at = planes + 16 * n;
                Ok([
                    f32_at(at)?,
                    f32_at(at + 4)?,
                    f32_at(at + 8)?,
                    f32_at(at + 12)?,
                ])
            })
            .collect::<Result<Vec<_>, String>>()?;
        hulls.push(Hull {
            mask,
            centroid,
            points,
            planes: plane_list,
            faces: face_list,
            half,
        });
    }
    let mut meshes = Vec::new();
    for number in 0..mesh_count {
        let base = mesh_table + 56 * number;
        let verts = u64_at(base + 8)?;
        let tris = u64_at(base + 16)?;
        let vert_count = u32_at(base + 28)? as usize;
        let tri_count = u32_at(base + 32)? as usize;
        if verts + 16 * vert_count > data.len() || tris + 32 * tri_count > data.len() {
            return Err("mesh runs past the chunk".into());
        }
        let mut numbers: Vec<Option<i32>> = vec![None; vert_count];
        let mut order: Vec<usize> = Vec::new();
        let mut kept = Vec::new();
        for record in data[tris..tris + 32 * tri_count].chunks_exact(32) {
            let word = |k: usize| i32::from_le_bytes(record[4 * k..4 * k + 4].try_into().unwrap());
            let half = |k: usize| u16::from_le_bytes([record[2 * k], record[2 * k + 1]]);
            let (surface, mask) = (half(13), half(14));
            if mask & category == 0 {
                continue;
            }
            // The corners are unsigned, the wings signed; both index the mesh.
            let vertices = [
                u32::from_le_bytes(record[0..4].try_into().unwrap()) as i64,
                u32::from_le_bytes(record[4..8].try_into().unwrap()) as i64,
                u32::from_le_bytes(record[8..12].try_into().unwrap()) as i64,
                i64::from(word(3)),
                i64::from(word(4)),
                i64::from(word(5)),
            ];
            let renamed = vertices.map(|vertex| {
                let Ok(vertex) = usize::try_from(vertex) else {
                    return -1;
                };
                if vertex >= vert_count {
                    return -1;
                }
                *numbers[vertex].get_or_insert_with(|| {
                    order.push(vertex);
                    order.len() as i32 - 1
                })
            });
            if renamed[..3].iter().any(|vertex| *vertex < 0) {
                return Err("triangle corner out of the mesh".into());
            }
            kept.push((renamed, mask, surface));
        }
        let vertices = order
            .iter()
            .map(|vertex| point(verts + 16 * vertex))
            .collect::<Result<Vec<_>, _>>()?;
        meshes.push((vertices, kept));
    }
    Ok((meshes, hulls))
}

/// The triangles of a collision chunk's meshes whose mask is exactly `mask`,
/// as their corners, in chunk order.
pub(crate) fn mask_triangles(data: &[u8], mask: u16) -> Result<Vec<[[f32; 3]; 3]>, String> {
    let u16_at = |at: usize| -> Result<u16, String> { Ok(u16::from_le_bytes(take(data, at)?)) };
    let u32_at = |at: usize| -> Result<u32, String> { Ok(u32::from_le_bytes(take(data, at)?)) };
    let u64_at = |at: usize| -> Result<usize, String> {
        usize::try_from(u64::from_le_bytes(take(data, at)?))
            .map_err(|_| "offset out of range".into())
    };
    let f32_at = |at: usize| -> Result<f32, String> { Ok(f32::from_le_bytes(take(data, at)?)) };
    let (mesh_count, mesh_table) = (u32_at(12)? as usize, u64_at(40)?);
    let mut out = Vec::new();
    for number in 0..mesh_count {
        let base = mesh_table + 56 * number;
        let verts = u64_at(base + 8)?;
        let tris = u64_at(base + 16)?;
        let vert_count = u32_at(base + 28)? as usize;
        let tri_count = u32_at(base + 32)? as usize;
        for record in (0..tri_count).map(|k| tris + 32 * k) {
            if u16_at(record + 28)? != mask {
                continue;
            }
            let corner = |k: usize| -> Result<[f32; 3], String> {
                let vertex = u32_at(record + 4 * k)? as usize;
                if vertex >= vert_count {
                    return Err("triangle corner out of the mesh".into());
                }
                let at = verts + 16 * vertex;
                Ok([f32_at(at)?, f32_at(at + 4)?, f32_at(at + 8)?])
            };
            out.push([corner(0)?, corner(1)?, corner(2)?]);
        }
    }
    Ok(out)
}

fn take<const N: usize>(data: &[u8], at: usize) -> Result<[u8; N], String> {
    data.get(at..at + N)
        .map(|bytes| bytes.try_into().unwrap())
        .ok_or_else(|| "collision chunk is truncated".into())
}

/// Where a cache came from, in its header.
struct Source {
    map: u64,
    chunk: u64,
    ckey: String,
    props: usize,
    geometry: Geometry,
}

/// The cache file: magic, version, a JSON header and the zlib-packed shapes:
/// the chunk's meshes (vertices f32 x, y, z; triangles i32 corners and wings;
/// u16 masks; u16 surfaces), its polytopes, the props' models (their shapes)
/// and the placements (u32 model, u16 group: 0, or k when only the game
/// modes that list the header's k-th identifier have it; translation, scale,
/// rotation).
fn pack(
    name: &str,
    meshes: &[Mesh],
    hulls: &[Hull],
    source: &Source,
    models: &[Vec<Shape>],
    placed: &[Placed],
    identifiers: &[u64],
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for (vertices, records) in meshes {
        put_mesh(&mut out, vertices, records);
    }
    for hull in hulls {
        put_hull(&mut out, hull);
    }
    for shapes in models {
        out.extend((shapes.len() as u16).to_le_bytes());
        for shape in shapes {
            match shape {
                Shape::Sphere(mask, centre, radius) => {
                    out.push(0);
                    out.extend(mask.to_le_bytes());
                    floats(&mut out, centre.iter().chain([radius]));
                }
                Shape::Capsule(mask, a, b, radius) => {
                    out.push(1);
                    out.extend(mask.to_le_bytes());
                    floats(&mut out, a.iter().chain(b).chain([radius]));
                }
                Shape::Hull(hull) => {
                    out.push(2);
                    put_hull(&mut out, hull);
                }
                Shape::Mesh(mask, vertices, records) => {
                    out.push(3);
                    out.extend(mask.to_le_bytes());
                    out.extend((vertices.len() as u32).to_le_bytes());
                    out.extend((records.len() as u32).to_le_bytes());
                    put_mesh(&mut out, vertices, records);
                }
            }
        }
    }
    for (model, identifier, translation, scale, rotation) in placed {
        let group = match identifiers.iter().position(|id| id == identifier) {
            Some(index) if *identifier != 0 => index as u16 + 1,
            _ => 0,
        };
        out.extend((*model as u32).to_le_bytes());
        out.extend(group.to_le_bytes());
        floats(&mut out, translation.iter().chain(scale).chain(rotation));
    }
    let header = header(
        name,
        meshes,
        hulls,
        source,
        models.len(),
        placed.len(),
        identifiers,
    );
    // The fastest level packs these shapes as small as the default does.
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&out)?;
    let payload = encoder.finish()?;
    let mut blob = Vec::with_capacity(16 + header.len() + payload.len());
    blob.extend(MAGIC);
    blob.extend(VERSION.to_le_bytes());
    blob.extend((header.len() as u32).to_le_bytes());
    blob.extend(header.as_bytes());
    blob.extend(payload);
    Ok(blob)
}

fn floats<'a>(out: &mut Vec<u8>, values: impl Iterator<Item = &'a f32>) {
    for value in values {
        out.extend(value.to_le_bytes());
    }
}

fn put_mesh(out: &mut Vec<u8>, vertices: &[[f32; 3]], records: &[Record]) {
    floats(out, vertices.iter().flatten());
    for (corners, _, _) in records {
        for corner in corners {
            out.extend(corner.to_le_bytes());
        }
    }
    for (_, mask, _) in records {
        out.extend(mask.to_le_bytes());
    }
    for (_, _, surface) in records {
        out.extend(surface.to_le_bytes());
    }
}

fn put_hull(out: &mut Vec<u8>, hull: &Hull) {
    for count in [
        hull.mask as usize,
        hull.points.len(),
        hull.planes.len(),
        hull.half.len(),
    ] {
        out.extend((count as u16).to_le_bytes());
    }
    floats(out, hull.centroid.iter());
    floats(out, hull.points.iter().flatten());
    floats(out, hull.planes.iter().flatten());
    out.extend(&hull.faces);
    out.extend(hull.half.iter().flatten());
}

/// The JSON header as Python's `json.dumps(header, sort_keys=True)` writes
/// it: keys sorted, `", "` and `": "` between items, non-ASCII escaped.
fn header(
    name: &str,
    meshes: &[Mesh],
    hulls: &[Hull],
    source: &Source,
    models: usize,
    placements: usize,
    identifiers: &[u64],
) -> String {
    let mesh_counts: Vec<String> = meshes
        .iter()
        .map(|(vertices, records)| format!("[{}, {}]", vertices.len(), records.len()))
        .collect();
    let identifiers: Vec<String> = identifiers
        .iter()
        .map(|id| python_string(&format!("0x{id:016X}")))
        .collect();
    let triangles: usize = meshes.iter().map(|(_, records)| records.len()).sum();
    let mut fields = vec![
        ("chunk", python_string(&format!("0x{:016X}", source.chunk))),
        ("ckey", python_string(&source.ckey)),
        ("hulls", hulls.len().to_string()),
        ("identifiers", format!("[{}]", identifiers.join(", "))),
        ("map", python_string(&format!("0x{:016X}", source.map))),
        ("meshes", format!("[{}]", mesh_counts.join(", "))),
        ("models", models.to_string()),
        ("name", python_string(name)),
        ("placements", placements.to_string()),
        ("props", source.props.to_string()),
        ("triangles", triangles.to_string()),
    ];
    if source.geometry == Geometry::Rays {
        fields.extend([
            ("category_mask", ALL_CATEGORIES.to_string()),
            ("geometry", python_string(RAY_GEOMETRY)),
            ("prop_scope", python_string(RAY_PROP_SCOPE)),
        ]);
        fields.sort_unstable_by_key(|(key, _)| *key);
    }
    let items: Vec<String> = fields
        .iter()
        .map(|(key, value)| format!("\"{key}\": {value}"))
        .collect();
    format!("{{{}}}", items.join(", "))
}

/// A JSON string as Python's `json.dumps` writes it (`ensure_ascii`).
fn python_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            _ => {
                let mut units = [0; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod ported_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_escape_like_python_json() {
        assert_eq!(
            python_string("Estádio das Rãs"),
            r#""Est\u00e1dio das R\u00e3s""#
        );
        assert_eq!(python_string("a\"b\\c\u{1}"), r#""a\"b\\c\u0001""#);
        assert_eq!(python_string("😀"), r#""\ud83d\ude00""#);
    }
}
