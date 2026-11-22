//! The physics props of a map (`ow174/game/props.py`): entity placeables
//! the client makes itself whose models have physics shapes that stop the
//! character mover. They are not in the map's collision chunk (0x11), which
//! holds the map's static geometry with its model groups baked in (checked on
//! the Practice Range: 905 model-group instances with mover shapes are in the
//! chunk to 2 cm, the entity props are not).
//!
//! What they are (data, 1.74): fences and railings (Practice Range 2600: 137
//! of them), desks and chairs (04AB, 0572, 04D1 next to the Practice Range
//! spawn), clip boxes (Eichenwalde 1612, only in the free-for-all modes) ...
//! The client makes every entity placeable that has a client id (body +104)
//! itself, with its definition's STUModelComponent model, and keeps one with
//! an identifier (body +8) only when the game mode lists that identifier
//! (STUGameMode m_A43573F4, the rule of the pending list 0x7FF78969EFD0;
//! Eichenwalde's clip boxes carry the free-for-all one). Placeables without a
//! client id are the server's (spawn doors, the payload); we do not make
//! them, so the client does not collide with them either. Client-only
//! breakables (the STUModelComponent's m_breakableConfig with m_clientOnly =
//! 1: gas bottles, small boxes, 119 definitions) are left out: they are
//! dynamic bodies, which the mover does not meet (below).
//!
//! A model's physics shapes are its STUModel m_CB4D298D list (MSTU chunk of
//! the 00C model, an STUv2 blob): {shape, holder}. The holder's inline
//! STUDominoFlags (STU_933AD0A9) is the shape's mask, packed as Python packs
//! it (bits of fields 0..12: 0, 8, 12, 7, 6, 5, 2, 9, 1, 3, 4, 10, 11; a field
//! left out keeps its default, every field on but the sixth: 0x1FDF), which
//! every mesh triangle also carries. Measured on the install's model meshes
//! (refs report `WeaponRays.md`), the client puts field 0 at bit 2, field 6 at
//! bit 0 and field 5 at bit 13; the cache keeps Python's packing, which gives
//! the same answer to every query the server makes (the mover's bits 3 and 4,
//! the shots' `1FDF` and `1F9F` hold bits 0 and 2 both and neither 5 nor 13).
//! A shape stops the mover when its
//! mask has a team proxy bit (0x08, 0x10), as the static triangles do, and
//! when the client makes it a static shape: the mover's filter (category
//! 0x08/0x10, include 0x1481, 0x7FF789B463F0) meets only shapes of category 1,
//! 0x80, 0x400 or 0x1000 (pair filter 0x7FF789D07730). The client's builders
//! pick a model's parts by the list entry's motion type (m_89AABA94) and give
//! each part a category from its holder (STUs with their class defaults, the
//! field offsets as the builders read them):
//! - 0, STU_897BADC9 (0x7FF78A9FD400): made when m_983EB565 != 0 (default 3);
//!   0x800 with m_8BE69838 (default 0), else 1 with m_B4D8D9BB (default 1),
//!   else 0x100
//! - 2, STU_E62A2EFC (0x7FF78A9CC460): made when m_E5FB1A8B != 0 (default 3);
//!   0x80 (or 0x1000) with m_B4D8D9BB (default 1), else 0x100
//! - 4, STU_82C2BA2C (0x7FF78AA31B80): made when m_8C9E3EE1 != 0 (default 3);
//!   1 (or 0x1000) with m_B4D8D9BB (default 0), else 0x100 (a dynamic body:
//!   office chairs on wheels, bottles, cones)
//!
//! Motion type 1 goes to another list (0x7FF78AA3CDE0, not the physics
//! world), type 3 entries have no shape. Checked against the running client's
//! own world: every prop shape of the Practice Range (762) has the category
//! this rule gives. The client-only breakables are dynamic bodies too.
//!
//! A list entry names the model node it hangs on (m_30F925F4): bones
//! STU_CB79EE74 and hardpoints STU_BE9DDFDF {m_EDF0511C id, m_FF592924
//! parent, m_7DC1550F translation, m_AF9D3A0C rotation}. The client puts a
//! static shape in its body's frame with the node's place in the model
//! applied to its geometry (the desk 04AB's hull hangs 1.62 m up on node
//! 0x8B: the client's hull is there, the placeable's frame its body). Shapes:
//! triangle meshes (with wing vertices as in chunk 0x11), convex polytopes
//! (planes, centroid, half edges as in chunk 0x11), capsules and spheres; the
//! physics engine takes them as they are, in the placeable's frame
//! (rotation, translation) times its scale.
//!
//! Which definitions are props and which model each one has: see
//! [`prop_model`].

use std::collections::HashMap;

use orpheus_stu::{Asset, Value as Stu};

use crate::{
    Install, Result, asset_error, guid_name,
    map_chunk::{ENTITY, entities, read_chunk},
};

/// STUModelComponent.
const MODEL_COMPONENT: u32 = 0x1918_35F9;
/// The team proxy categories.
pub(crate) const MOVER: u16 = 0x18;
const CHUNK_MAGIC: u32 = 0xF123_456F;
const CHUNK_TAG: u16 = 0xC65C;
/// STUDominoFlags fields in order and the mask bit each one sets (packed as
/// in the model meshes' triangles).
const FLAG_FIELDS: [u32; 13] = [
    0xED81_9119,
    0x3485_8FD7,
    0x1E5E_EC85,
    0x7E48_C526,
    0xE552_CFAA,
    0xB975_96D2,
    0x27CD_5BF8,
    0x61BB_5D6E,
    0xDEF2_CFF6,
    0xA4F2_490B,
    0xD7E9_C136,
    0x13F1_77D2,
    0xD608_21FE,
];
const FLAG_BITS: [u32; 13] = [0, 8, 12, 7, 6, 5, 2, 9, 1, 3, 4, 10, 11];
const DEFAULT_MASK: u16 = 0x1FDF;
const SHAPES: u32 = 0xCB4D_298D;
const SHAPE: u32 = 0xB7C8_314A;
const HOLDER: u32 = 0x9B04_24EB;
const FLAGS: u32 = 0xCBCB_2AB9;
const MESH: u32 = 0x537E_56E4;
const CONVEX: u32 = 0xB380_0E70;
const CAPSULE: u32 = 0x4C8D_9F5E;
const SPHERE: u32 = 0x4614_D0E5;
const VERTICES: u32 = 0x88FC_ECD7;
const TRIANGLES: u32 = 0x0D50_490E;
const HALF_EDGES: u32 = 0x036F_F5AD;
const FACES: u32 = 0xDEBE_73A9;
const POINT_A: u32 = 0x10EF_DE90;
const POINT_B: u32 = 0xC320_C90B;
const CENTRE: u32 = 0xA8D1_2F28;
const RADIUS: u32 = 0xED61_D926;
const PLANES: u32 = 0x0674_FD43;
const CENTROID: u32 = 0x7D1F_C63E;
/// The list entry's motion type and node number.
const MOTION: u32 = 0x89AA_BA94;
const NODE: u32 = 0x30F9_25F4;
/// Bones and hardpoints of the model.
const NODE_CLASSES: [u32; 2] = [0xCB79_EE74, 0xBE9D_DFDF];
const NODE_ID: u32 = 0xEDF0_511C;
const PARENT: u32 = 0xFF59_2924;
const TRANSLATION: u32 = 0x7DC1_550F;
const ROTATION: u32 = 0xAF9D_3A0C;
const STATIC: u32 = 0xB4D8_D9BB;
const SENSOR: u32 = 0x8BE6_9838;
/// By motion type: the holder class its builder reads, the holder's enum
/// that makes the part (0: none, default 3), m_B4D8D9BB's default in that
/// class.
const BUILDERS: [(u8, u32, u32, u8); 3] = [
    (0, 0x897B_ADC9, 0x983E_B565, 1),
    (2, 0xE62A_2EFC, 0xE5FB_1A8B, 1),
    (4, 0x82C2_BA2C, 0x8C9E_3EE1, 0),
];

/// An STUDominoFlags packed into a mask, each field given by `flag` (its hash
/// to its value, `None` when the asset leaves it at its default).
fn pack_flags(flag: impl Fn(u32) -> Option<bool>) -> u16 {
    let mut mask = DEFAULT_MASK;
    for (field, bit) in FLAG_FIELDS.into_iter().zip(FLAG_BITS) {
        match flag(field) {
            Some(true) => mask |= 1 << bit,
            Some(false) => mask &= !(1 << bit),
            None => {}
        }
    }
    mask
}

/// The bit of each STUDominoFlags field in the client's own packing
/// (`0x7FF78A6B8FC0`; measured on the model meshes, refs report
/// `WeaponRays.md`): [`FLAG_BITS`] with fields 0, 5 and 6 at bits 2, 13
/// and 0.
const CLIENT_FLAG_BITS: [u32; 13] = [2, 8, 12, 7, 6, 13, 0, 9, 1, 3, 4, 10, 11];

/// An STUDominoFlags packed as the client packs it, each field given by
/// `flag` (its hash to its value).
pub(crate) fn pack_client_flags(flag: impl Fn(u32) -> bool) -> u16 {
    FLAG_FIELDS
        .into_iter()
        .zip(CLIENT_FLAG_BITS)
        .filter(|&(field, _)| flag(field))
        .fold(0, |mask, (_, bit)| mask | 1 << bit)
}

/// A triangle record of chunk 0x11 and of model meshes: corners, wing
/// vertices (the far corner of the neighbour across each edge, -1 none),
/// mask, surface.
pub(crate) type Record = ([i32; 6], u16, u16);

/// A convex polytope: mask, centroid, vertices, planes, faces (each its
/// first half edge), half edges {s8 twin, u8 origin, u8 face, u8 next}.
#[derive(Clone, Debug)]
pub(crate) struct Hull {
    pub mask: u16,
    pub centroid: [f32; 3],
    pub points: Vec<[f32; 3]>,
    pub planes: Vec<[f32; 4]>,
    pub faces: Vec<u8>,
    pub half: Vec<[u8; 4]>,
}

/// A model's physics shape in model space.
#[derive(Clone, Debug)]
pub(crate) enum Shape {
    /// A triangle mesh: the holder's mask, every vertex, the triangles that
    /// stop the category (each with its own mask).
    Mesh(u16, Vec<[f32; 3]>, Vec<Record>),
    Hull(Hull),
    Capsule(u16, [f32; 3], [f32; 3], f32),
    Sphere(u16, [f32; 3], f32),
}

/// A placed prop: its model's number, its identifier (0 or the one the
/// game mode must list), translation, scale, rotation.
pub(crate) type Placed = (usize, u64, [f32; 3], [f32; 3], [f32; 4]);

/// A map's props: (models: each model's shapes that stop `category`,
/// placements in placeable order). A placement is kept only when its model
/// has shapes; every model a prop names is listed, with or without shapes.
/// Which models are props doesn't depend on the category (Python's
/// `map_shapes` takes them from one catalogue for the movement and the ray
/// caches); `category` must hold the mover's bits ([`MOVER`]).
pub(crate) fn map_shapes(
    install: &Install,
    map: u64,
    category: u16,
) -> Result<(Vec<Vec<Shape>>, Vec<Placed>)> {
    let Some(data) = read_chunk(install, map, ENTITY)? else {
        return Ok((Vec::new(), Vec::new()));
    };
    let mut models: Vec<Vec<Shape>> = Vec::new();
    // Model GUID -> its number, `None` when it isn't a prop's.
    let mut index: HashMap<u64, Option<usize>> = HashMap::new();
    let mut roots: HashMap<u64, Option<u64>> = HashMap::new();
    let mut placed = Vec::new();
    for entity in entities(&data)? {
        if entity.client_id == 0 {
            continue;
        }
        let model = match roots.get(&entity.definition) {
            Some(model) => *model,
            None => {
                let model = root_model(install, entity.definition);
                roots.insert(entity.definition, model);
                model
            }
        };
        let Some(model) = model else { continue };
        let number = match index.get(&model) {
            Some(number) => *number,
            None => {
                let read = read_model(&install.raw(model)?, category)
                    .map_err(|reason| asset_error(format!("{}: {reason}", guid_name(model))))?;
                let number = read.catalogued.then(|| {
                    models.push(read.shapes);
                    models.len() - 1
                });
                index.insert(model, number);
                number
            }
        };
        if let Some(number) = number.filter(|number| !models[*number].is_empty()) {
            placed.push((
                number,
                entity.identifier,
                entity.translation,
                entity.scale,
                entity.rotation,
            ));
        }
    }
    Ok((models, placed))
}

/// The model a prop's definition makes: its first STUModelComponent that
/// isn't a client-only breakable; `None` when it has none or the install
/// can't decode it.
///
/// Which of these models are props is the rule of the prop catalogue the
/// Python reference read (`data/collision_props_174.json`, made by a scratch
/// tool that isn't in the repository), reconstructed from the catalogue and
/// checked against all its 1094 definitions on its 89 maps: a model is a
/// prop's when it has a static shape that stops the mover (what the cache
/// keeps), or a list entry of a motion type other than 2 with a polytope,
/// capsule or sphere (any mask), or a mesh triangle with a team proxy bit
/// (the catalogue's older test, [`Model::catalogued`]). The catalogue lists
/// such models even without static shapes; the cache keeps them, empty.
///
/// The catalogue lacks the models stored inside bundles (its tool read only
/// models stored as their own files): 17 definitions placed on its maps.
/// They are props here; on the Practice Range these are 049A to 049F, six
/// models without static shapes (see the collision test).
fn root_model(install: &Install, definition: u64) -> Option<u64> {
    let asset = install.stu(definition).ok()?;
    model_components(&asset)
        .into_iter()
        .find(|(_, client_only)| !client_only)
        .map(|(model, _)| model)
}

/// One static physics shape of a server placeable's model: the category
/// the client files it under ([`Structured::static_category`]), the model
/// node it hangs on, and the shape in model space.
pub(crate) struct ModelPart {
    pub category: u16,
    pub node: Option<u32>,
    pub shape: Shape,
}

/// A definition's model ([`root_model`]) and its static physics shapes that
/// stop `category`, in model space, as a prop's ([`map_shapes`]); none when
/// it has no model. The server-owned placeables use it (`crate::map_areas`).
pub(crate) fn definition_shapes(
    install: &Install,
    definition: u64,
    category: u16,
) -> Result<Option<(u64, Vec<ModelPart>)>> {
    let Some(model) = root_model(install, definition) else {
        return Ok(None);
    };
    let read = read_model(&install.raw(model)?, category)
        .map_err(|reason| asset_error(format!("{}: {reason}", guid_name(model))))?;
    let parts = read
        .categories
        .into_iter()
        .zip(read.nodes)
        .zip(read.shapes)
        .map(|((category, node), shape)| ModelPart {
            category,
            node,
            shape,
        })
        .collect();
    Ok(Some((model, parts)))
}

/// The entity's STUModelComponent models and whether each is a client-only
/// breakable (m_breakableConfig m_clientOnly = 1), in component map order.
pub(crate) fn model_components(asset: &Asset) -> Vec<(u64, bool)> {
    let Some(Stu::Map(entries)) = asset.root().and_then(|root| root.get("m_componentMap")) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|(_, index)| asset.instances.get((*index)?)?.as_ref())
        .filter(|object| object.class == MODEL_COMPONENT)
        .filter_map(|object| {
            let Some(Stu::Guid(model)) = object.get("m_model") else {
                return None;
            };
            let client_only = object
                .get("m_breakableConfig")
                .and_then(|config| asset.resolve(config))
                .and_then(|config| config.get("m_clientOnly"))
                .is_some_and(|value| matches!(value, Stu::Int(1) | Stu::UInt(1)));
            (*model != 0).then_some((*model, client_only))
        })
        .collect()
}

// ---- the model file

/// Python-style slicing: `data[start:end]`, clamped to the data.
pub(crate) fn slice(data: &[u8], start: usize, end: usize) -> &[u8] {
    let end = end.min(data.len());
    &data[start.min(end)..end]
}

fn read<const N: usize>(data: &[u8], at: usize) -> Result<[u8; N], String> {
    data.get(at..at + N)
        .map(|bytes| bytes.try_into().unwrap())
        .ok_or_else(|| "truncated structured data".to_owned())
}

fn i32_at(data: &[u8], at: usize) -> Result<i32, String> {
    read::<4>(data, at).map(i32::from_le_bytes)
}

fn u32_at(data: &[u8], at: usize) -> Result<u32, String> {
    read::<4>(data, at).map(u32::from_le_bytes)
}

pub(crate) fn i64_at(data: &[u8], at: usize) -> Result<i64, String> {
    read::<8>(data, at).map(i64::from_le_bytes)
}

fn f32_at(data: &[u8], at: usize) -> Result<f32, String> {
    read::<4>(data, at).map(f32::from_le_bytes)
}

/// The payload of the first top-level chunk of a tag in a chunked file
/// (teChunkedData: 00C models), such as `MSTU`.
pub(crate) fn chunk<'a>(data: &'a [u8], name: &[u8; 4]) -> Result<Option<&'a [u8]>, String> {
    if data.len() < 16 || u32_at(data, 0)? != CHUNK_MAGIC {
        return Err("not a chunked file".into());
    }
    let size = u32_at(data, 8)? as usize;
    let mut position = 16;
    while position + 16 <= size.min(data.len()) {
        let ident = u32_at(data, position)?;
        let serialized = i32_at(data, position + 4)?;
        let length = i32_at(data, position + 8)?;
        let tag = u16::from_le_bytes(read::<2>(data, position + 12)?);
        if tag != CHUNK_TAG {
            break;
        }
        let start = position + 16;
        if ident.to_be_bytes() == *name {
            let end = (start as i64 + i64::from(length)).max(0) as usize;
            return Ok(Some(slice(data, start, end)));
        }
        let step = 16 + i64::from(serialized.max(length));
        position = usize::try_from(position as i64 + step).map_err(|_| "bad chunk size")?;
    }
    Ok(None)
}

/// A field's place in the body: (start, size).
type Place = (usize, usize);
/// An instance's fields: hash -> place, a later field of a hash replacing an
/// earlier one.
type Fields = Vec<(u32, Place)>;

fn place(fields: &Fields, field: u32) -> Option<Place> {
    fields
        .iter()
        .rev()
        .find(|(hash, _)| *hash == field)
        .map(|(_, place)| *place)
}

/// An STUv2 blob, as much of it as the shapes need: each instance's type
/// hash and where its fields are. Header: bags {i32 count, i32 offset} of
/// instance infos (16 B: hash, ..., size), inline types and field bags (each
/// a bag of {hash, size}), then i32 dynamic data size and offset, i32 data
/// offset. An instance in the data: i32 field bag, then each field of the bag
/// (a size of 0 means an i32 size first). Embedded instances are i32
/// indexes, arrays an i32 offset into the dynamic data {i32 count, u32, i64
/// start}.
struct Structured<'a> {
    hashes: Vec<u32>,
    bags: Vec<Vec<(u32, i32)>>,
    dynamic: &'a [u8],
    body: &'a [u8],
    fields: Vec<Fields>,
}

impl<'a> Structured<'a> {
    fn new(data: &'a [u8]) -> Result<Self, String> {
        let infos = bag(data, 0, 16)?;
        let hashes = infos
            .iter()
            .map(|at| u32_at(data, *at))
            .collect::<Result<Vec<_>, _>>()?;
        let sizes = infos
            .iter()
            .map(|at| i32_at(data, at + 12))
            .collect::<Result<Vec<_>, _>>()?;
        let mut bags = Vec::new();
        for at in bag(data, 16, 8)? {
            let inner = bag(data, at, 8)?
                .into_iter()
                .map(|inner| Ok((u32_at(data, inner)?, i32_at(data, inner + 4)?)))
                .collect::<Result<Vec<_>, String>>()?;
            bags.push(inner);
        }
        let dynamic_size = i32_at(data, 24)?;
        let dynamic_at = i32_at(data, 28)?;
        let body_at = i32_at(data, 32)?;
        let dynamic = if dynamic_size > 0 {
            slice(
                data,
                dynamic_at.max(0) as usize,
                (dynamic_at + dynamic_size).max(0) as usize,
            )
        } else {
            &[]
        };
        let body = slice(data, body_at.max(0) as usize, data.len());
        let mut stu = Self {
            hashes,
            bags,
            dynamic,
            body,
            fields: Vec::new(),
        };
        let mut position: i64 = 0;
        for size in sizes {
            let at = usize::try_from(position).map_err(|_| "bad instance size")?;
            let fields = stu.fields_at(at)?;
            stu.fields.push(fields);
            position += i64::from(size);
        }
        Ok(stu)
    }

    fn fields_at(&self, mut position: usize) -> Result<Fields, String> {
        let mut found = Vec::new();
        let bag = i32_at(self.body, position)?;
        position += 4;
        let Some(bag) = usize::try_from(bag).ok().and_then(|bag| self.bags.get(bag)) else {
            return Ok(found);
        };
        for (field, size) in bag {
            let mut size = *size;
            if size == 0 {
                size = i32_at(self.body, position)?;
                position += 4;
            }
            let size = usize::try_from(size).map_err(|_| "negative field size")?;
            found.push((*field, (position, size)));
            position += size;
        }
        Ok(found)
    }

    fn raw(&self, fields: &Fields, field: u32) -> Option<&'a [u8]> {
        let (start, size) = place(fields, field)?;
        Some(slice(self.body, start, start + size))
    }

    fn instance_raw(&self, instance: usize, field: u32) -> Option<&'a [u8]> {
        self.raw(&self.fields[instance], field)
    }

    fn reference(&self, instance: usize, field: u32) -> Option<usize> {
        let value = self.instance_raw(instance, field)?;
        let index = i32::from_le_bytes(value.get(..4)?.try_into().unwrap());
        usize::try_from(index)
            .ok()
            .filter(|index| *index < self.hashes.len())
    }

    fn array(&self, instance: usize, field: u32, stride: usize) -> Result<Vec<&'a [u8]>, String> {
        let Some(value) = self
            .instance_raw(instance, field)
            .filter(|value| value.len() >= 4)
        else {
            return Ok(Vec::new());
        };
        let offset = i32::from_le_bytes(value[..4].try_into().unwrap());
        let Ok(offset) = usize::try_from(offset) else {
            return Ok(Vec::new());
        };
        if offset + 16 > self.dynamic.len() {
            return Ok(Vec::new());
        }
        let count = i32_at(self.dynamic, offset)?;
        let start = i64_at(self.dynamic, offset + 8)?;
        let start = usize::try_from(start).map_err(|_| "negative array start")?;
        Ok((0..count.max(0) as usize)
            .map(|k| slice(self.dynamic, start + stride * k, start + stride * (k + 1)))
            .collect())
    }

    fn inline(&self, instance: usize, field: u32) -> Result<Option<Fields>, String> {
        match place(&self.fields[instance], field) {
            Some((start, _)) => self.fields_at(start).map(Some),
            None => Ok(None),
        }
    }

    fn byte(&self, fields: &Fields, field: u32, default: u8) -> u8 {
        self.raw(fields, field)
            .and_then(|raw| raw.first().copied())
            .unwrap_or(default)
    }

    /// The holder's STUDominoFlags, packed ([`pack_flags`]).
    fn mask(&self, holder: Option<usize>) -> Result<u16, String> {
        let flags = match holder {
            Some(holder) => self.inline(holder, FLAGS)?,
            None => None,
        };
        Ok(pack_flags(|field| {
            flags
                .as_ref()
                .and_then(|flags| self.raw(flags, field))
                .and_then(|value| value.first())
                .map(|value| *value != 0)
        }))
    }

    /// The category of a model's list entry when the client makes it a
    /// static shape (1, 0x80 or 0x1000), the only ones the character mover
    /// meets (see the module notes); `None` otherwise. The builders pick
    /// 0x80 (motion type 2) or 1 (type 4) when the entry's enum that makes
    /// it is 3, else 0x1000 (`0x7FF78A9CC460` @`0x7FF78A9CDC39`,
    /// `0x7FF78AA31B80` @`0x7FF78AA3153C`); type 0 picks 1, or 0x1000 when
    /// a flag of the model at +177 is set (`0x7FF78A9FD400`
    /// @`0x7FF78A9FDBEA`; what sets it was not read, taken as unset).
    fn static_category(&self, wrapper: usize, holder: Option<usize>) -> Option<u16> {
        let motion = self.byte(&self.fields[wrapper], MOTION, 0);
        let (_, holder_class, made, static_default) =
            BUILDERS.iter().find(|builder| builder.0 == motion)?;
        let holder = holder.filter(|holder| self.hashes[*holder] == *holder_class)?;
        let fields = &self.fields[holder];
        let made = self.byte(fields, *made, 3);
        if made == 0 || self.byte(fields, STATIC, *static_default) == 0 {
            return None;
        }
        if *holder_class == 0x897B_ADC9 && self.byte(fields, SENSOR, 0) != 0 {
            return None;
        }
        Some(match (motion, made) {
            (0, _) => 1,
            (2, 3) => 0x80,
            (4, 3) => 1,
            _ => 0x1000,
        })
    }

    /// The model's nodes: number -> (translation, rotation xyzw, parent).
    fn nodes(&self) -> Result<HashMap<u32, Node>, String> {
        let mut out = HashMap::new();
        for (instance, kind) in self.hashes.iter().enumerate() {
            if !NODE_CLASSES.contains(kind) {
                continue;
            }
            let fields = &self.fields[instance];
            let Some(ident) = self.raw(fields, NODE_ID).filter(|ident| ident.len() >= 4) else {
                continue;
            };
            let rotation = match self
                .raw(fields, ROTATION)
                .filter(|rotation| rotation.len() >= 16)
            {
                Some(raw) => [0, 4, 8, 12].map(|at| f32_at(raw, at).expect("16 bytes")),
                None => [0.0, 0.0, 0.0, 1.0],
            };
            let parent = self
                .raw(fields, PARENT)
                .filter(|parent| parent.len() >= 4)
                .map(|raw| u32_at(raw, 0).expect("4 bytes"));
            out.insert(
                u32_at(ident, 0)?,
                (vector(self.raw(fields, TRANSLATION)), rotation, parent),
            );
        }
        Ok(out)
    }
}

type Node = ([f32; 3], [f32; 4], Option<u32>);

fn bag(data: &[u8], at: usize, stride: usize) -> Result<Vec<usize>, String> {
    let count = i32_at(data, at)?;
    let offset = i32_at(data, at + 4)?;
    if count < 0 || offset < 0 || offset as usize + stride * count as usize > data.len() {
        return Err("bad structured data".into());
    }
    Ok((0..count as usize)
        .map(|k| offset as usize + stride * k)
        .collect())
}

fn vector(raw: Option<&[u8]>) -> [f32; 3] {
    match raw.filter(|raw| raw.len() >= 12) {
        Some(raw) => [0, 4, 8].map(|at| f32_at(raw, at).expect("12 bytes")),
        None => [0.0; 3],
    }
}

fn float(raw: Option<&[u8]>) -> f32 {
    raw.filter(|raw| raw.len() >= 4)
        .map_or(0.0, |raw| f32_at(raw, 0).expect("4 bytes"))
}

/// A 64-bit result rounded to a 32-bit float, as Python's `_f32` does.
fn f(value: f64) -> f32 {
    value as f32
}

/// v turned by the quaternion q in float32: v + 2 q x (w v + q x v), each
/// step computed in 64 bits and rounded to 32.
fn rotate32(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let [x, y, z, w] = q.map(f64::from);
    let [vx, vy, vz] = v.map(f64::from);
    let (cx, cy, cz) = (f(y * vz - z * vy), f(z * vx - x * vz), f(x * vy - y * vx));
    let ux = f(f64::from(f(w * vx)) + f64::from(cx));
    let uy = f(f64::from(f(w * vy)) + f64::from(cy));
    let uz = f(f64::from(f(w * vz)) + f64::from(cz));
    let [ux, uy, uz] = [ux, uy, uz].map(f64::from);
    let (tx, ty, tz) = (f(y * uz - z * uy), f(z * ux - x * uz), f(x * uy - y * ux));
    [
        f(vx + f64::from(f(f64::from(tx) + f64::from(tx)))),
        f(vy + f64::from(f(f64::from(ty) + f64::from(ty)))),
        f(vz + f64::from(f(f64::from(tz) + f64::from(tz)))),
    ]
}

/// (translation, rotation) of a node in the model's frame, its parents
/// applied; `None` for the root (no translation, no turn).
fn node_frame(table: &HashMap<u32, Node>, mut number: Option<u32>) -> Option<([f32; 3], [f32; 4])> {
    let mut translation = [0.0_f32; 3];
    let mut rotation = [0.0_f32, 0.0, 0.0, 1.0];
    let mut seen = Vec::new();
    while let Some(current) = number {
        let Some((t, q, parent)) = table.get(&current) else {
            break;
        };
        if seen.contains(&current) {
            break;
        }
        seen.push(current);
        let turned = rotate32(*q, translation);
        translation = [0, 1, 2].map(|k| f(f64::from(turned[k]) + f64::from(t[k])));
        let [x1, y1, z1, w1] = q.map(f64::from);
        let [x2, y2, z2, w2] = rotation.map(f64::from);
        rotation = [
            f(w1 * x2 + x1 * w2 + y1 * z2 - z1 * y2),
            f(w1 * y2 - x1 * z2 + y1 * w2 + z1 * x2),
            f(w1 * z2 + x1 * y2 - y1 * x2 + z1 * w2),
            f(w1 * w2 - x1 * x2 - y1 * y2 - z1 * z2),
        ];
        number = *parent;
    }
    if rotation == [0.0, 0.0, 0.0, 1.0] && translation.iter().all(|v| f64::from(*v).abs() < 1e-12) {
        return None;
    }
    Some((translation, rotation))
}

fn moved(point: [f32; 3], frame: ([f32; 3], [f32; 4])) -> [f32; 3] {
    let (t, q) = frame;
    let turned = rotate32(q, point);
    [0, 1, 2].map(|k| f(f64::from(turned[k]) + f64::from(t[k])))
}

/// A model shape moved to its node's place in the model (float32).
fn at_node(shape: Shape, frame: ([f32; 3], [f32; 4])) -> Shape {
    match shape {
        Shape::Mesh(mask, points, records) => Shape::Mesh(
            mask,
            points.into_iter().map(|p| moved(p, frame)).collect(),
            records,
        ),
        Shape::Hull(hull) => {
            let (t, q) = frame;
            let planes = hull
                .planes
                .iter()
                .map(|[nx, ny, nz, w]| {
                    let n = rotate32(q, [*nx, *ny, *nz]);
                    let product = |k: usize| f64::from(f(f64::from(n[k]) * f64::from(t[k])));
                    let offset = f(f64::from(f(product(0) + product(1))) + product(2));
                    [n[0], n[1], n[2], f(f64::from(*w) + f64::from(offset))]
                })
                .collect();
            Shape::Hull(Hull {
                mask: hull.mask,
                centroid: moved(hull.centroid, frame),
                points: hull.points.into_iter().map(|p| moved(p, frame)).collect(),
                planes,
                faces: hull.faces,
                half: hull.half,
            })
        }
        Shape::Capsule(mask, a, b, radius) => {
            Shape::Capsule(mask, moved(a, frame), moved(b, frame), radius)
        }
        Shape::Sphere(mask, centre, radius) => Shape::Sphere(mask, moved(centre, frame), radius),
    }
}

/// A 00C model as the props need it.
struct Model {
    /// Its physics shapes that stop the category, in model space, as the
    /// engine keeps them: only the static ones, each moved to its node's
    /// place in the model. A mesh's triangles carry their own masks; the
    /// other shapes the holder's.
    shapes: Vec<Shape>,
    /// Each shape's category ([`Structured::static_category`]).
    categories: Vec<u16>,
    /// The model node each shape hangs on (`m_30F925F4`), when it names one.
    nodes: Vec<Option<u32>>,
    /// Whether the model is a prop's (see [`root_model`]): it has a static
    /// shape that stops the mover, or, the prop catalogue's older test, a
    /// list entry whose motion type isn't 2 has a polytope, capsule or
    /// sphere, or a mesh triangle with a team proxy bit.
    catalogued: bool,
}

/// Read a model's shapes that stop `category`, which holds the mover's bits.
fn read_model(data: &[u8], category: u16) -> Result<Model, String> {
    let mut model = Model {
        shapes: Vec::new(),
        categories: Vec::new(),
        nodes: Vec::new(),
        catalogued: false,
    };
    let Some(payload) = chunk(data, b"MSTU")? else {
        return Ok(model);
    };
    if payload.len() < 16 {
        return Ok(model);
    }
    let offset = i64_at(payload, 0)?;
    let size = i64_at(payload, 8)?;
    let start = usize::try_from(offset).map_err(|_| "bad MSTU offset")?;
    let end = usize::try_from(offset + size).map_err(|_| "bad MSTU size")?;
    let stu = Structured::new(slice(payload, start, end))?;
    if stu.fields.is_empty() {
        return Err("model without instances".into());
    }
    let table = stu.nodes()?;
    for item in stu.array(0, SHAPES, 8)? {
        let wrapper = i32_at(item, 0)?;
        let Some(wrapper) = usize::try_from(wrapper)
            .ok()
            .filter(|w| *w < stu.hashes.len())
        else {
            continue;
        };
        let Some(shape) = stu.reference(wrapper, SHAPE) else {
            continue;
        };
        let kind = stu.hashes[shape];
        let holder = stu.reference(wrapper, HOLDER);
        if stu.byte(&stu.fields[wrapper], MOTION, 0) != 2 {
            model.catalogued |= match kind {
                MESH => mesh_records(&stu, shape, MOVER)?
                    .1
                    .iter()
                    .any(|record| record.1 & MOVER != 0),
                CONVEX | CAPSULE | SPHERE => true,
                _ => false,
            };
        }
        let Some(shape_category) = stu.static_category(wrapper, holder) else {
            continue;
        };
        let node = stu
            .instance_raw(wrapper, NODE)
            .filter(|node| node.len() >= 4)
            .map(|node| u32_at(node, 0).expect("4 bytes"));
        let frame = node.and_then(|node| node_frame(&table, Some(node)));
        let mask = stu.mask(holder)?;
        let fields = &stu.fields[shape];
        let made = match kind {
            MESH => {
                let (points, records) = mesh_records(&stu, shape, category)?;
                (!records.is_empty()).then_some(Shape::Mesh(mask, points, records))
            }
            _ if mask & category == 0 => None,
            // Faces as loops of half edges {s8 twin, u8 origin, u8 face, u8 next}.
            CONVEX => {
                let points: Vec<[f32; 3]> = stu
                    .array(shape, VERTICES, 12)?
                    .into_iter()
                    .map(|raw| vector(Some(raw)))
                    .collect();
                let planes = stu
                    .array(shape, PLANES, 16)?
                    .into_iter()
                    .map(|raw| {
                        Ok([
                            f32_at(raw, 0)?,
                            f32_at(raw, 4)?,
                            f32_at(raw, 8)?,
                            f32_at(raw, 12)?,
                        ])
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                let edges: Vec<u8> = stu.array(shape, HALF_EDGES, 1)?.concat();
                let half = edges
                    .chunks_exact(4)
                    .map(|edge| edge.try_into().unwrap())
                    .collect();
                let faces: Vec<u8> = stu.array(shape, FACES, 1)?.concat();
                (!points.is_empty() && planes.len() == faces.len()).then(|| {
                    Shape::Hull(Hull {
                        mask,
                        centroid: vector(stu.raw(fields, CENTROID)),
                        points,
                        planes,
                        faces,
                        half,
                    })
                })
            }
            CAPSULE => Some(Shape::Capsule(
                mask,
                vector(stu.raw(fields, POINT_A)),
                vector(stu.raw(fields, POINT_B)),
                float(stu.raw(fields, RADIUS)),
            )),
            SPHERE => Some(Shape::Sphere(
                mask,
                vector(stu.raw(fields, CENTRE)),
                float(stu.raw(fields, RADIUS)),
            )),
            _ => None,
        };
        if let Some(made) = made {
            model.catalogued |= match &made {
                Shape::Mesh(_, _, records) => records.iter().any(|record| record.1 & MOVER != 0),
                _ => mask & MOVER != 0,
            };
            model.categories.push(shape_category);
            model.nodes.push(node);
            model.shapes.push(match frame {
                Some(frame) => at_node(made, frame),
                None => made,
            });
        }
    }
    Ok(model)
}

/// A mesh's vertices and its triangles that stop the category with all
/// three corners in the mesh (the 32-byte records of chunk 0x11; a wing out
/// of the mesh becomes -1).
fn mesh_records(
    stu: &Structured<'_>,
    shape: usize,
    category: u16,
) -> Result<(Vec<[f32; 3]>, Vec<Record>), String> {
    let points: Vec<[f32; 3]> = stu
        .array(shape, VERTICES, 16)?
        .into_iter()
        .map(|raw| vector(Some(raw)))
        .collect();
    let raw: Vec<u8> = stu.array(shape, TRIANGLES, 1)?.concat();
    let mut records = Vec::new();
    for record in raw.chunks_exact(32) {
        let word = |k: usize| u32_at(record, 4 * k).expect("32 bytes");
        let half = |k: usize| u16::from_le_bytes([record[2 * k], record[2 * k + 1]]);
        let corners = [word(0), word(1), word(2)];
        let (surface, own) = (half(13), half(14));
        if own & category != 0
            && corners
                .iter()
                .all(|corner| (*corner as usize) < points.len())
        {
            let wing = |k: usize| {
                let w = word(k) as i32;
                if w >= 0 && (w as usize) < points.len() {
                    w
                } else {
                    -1
                }
            };
            let [a, b, c] = corners.map(|corner| corner as i32);
            records.push(([a, b, c, wing(3), wing(4), wing(5)], own, surface));
        }
    }
    Ok((points, records))
}
