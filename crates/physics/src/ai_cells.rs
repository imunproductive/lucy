//! The client's AI cell data: the voxel grids its path query searches
//! (`D:/Work/Coding/refs/reports/Navigation.md`, "Document 0").
//!
//! The data is `data/aicells/<map>.bin.gz`, the map chunk 0x12's document 0
//! (`STU_3BC32A6E`) as `orpheus_assets` derives it: one byte block per cell,
//! keyed as the client keys its grid.
//!
//! The grid (`0x7FF78AA64190` builds it with `0x7FF78A614BD0(grid, {16.0,
//! 22.4})`): cells of 64 × 64 × 64 voxels, a voxel 0.25 m wide and 1433 /
//! 4096 m high (22.4 / 64 truncated to 1/4096 by `0x7FF78A758990`), so a
//! cell is 16 × 22.390625 × 16 m. A cell's key (`0x7FF78A618CA0`) packs its
//! coordinates as `(x + 2048) & 0xFFF | (z + 2048) & 0xFFF << 12 | (y + 128)
//! & 0xFF << 24`.
//!
//! Each block holds two voxel graphs of the same shape (the block's builder
//! `0x7FF78A6105F0` writes graph A from its first source, graph B from its
//! second):
//! - [`Graph::Walk`] (graph A, sections 0–8, mask at `+136`, count at `+152`):
//!   the voxel layer just above walkable floors (the Practice Range's floor at
//!   y 1.0 has its voxel 3, roofs at 13.26 and 19.0 their voxels 38 and 55).
//!   The client's path query in mode 0 (`0x7FF78A627F90`, lookup
//!   `0x7FF789AECFF0`) searches it.
//! - [`Graph::Air`] (graph B, sections 9–16, mask at `+144`, count at
//!   `+156`): the free space (everything above the floor layer up to the
//!   ceilings, the void under a map). The path query in mode 1
//!   (`0x7FF78A627A60`, lookup `0x7FF78A5FB310`), the one Widowmaker's ledge
//!   pull uses, searches it.
//!
//! A graph is a three-level tree over the cell: 4 m bricks (4 × 4 × 4 per
//! cell), 1 m units (4 × 4 × 4 per brick, 1 × 1.4 × 1 m) and voxels (4 × 4 ×
//! 4 per unit). An index within a level is `y | z << 2 | x << 4` of that
//! level's 2-bit coordinates. The block's sections, by their role (graph A
//! / graph B):
//! - root mask (`+136` / `+144`): the bricks that hold any voxel;
//! - regions (0 / 9, u64 each, `+152` / `+156` of them): the cell's
//!   connected parts;
//! - brick lists (1 / 10, u64): one per root bit in bit order, either a
//!   brick node (bit 63 set) or a run `{u32 first, u32 count | 1 << 31...}`
//!   (bit 63 clear: the low word is the first node's index, the high word's
//!   low 31 bits the count). A brick node: bits 0–7 its region, bits 8–17
//!   (A) / 8–19 (B) the index of its unit mask, bits 29–40 (A) / 36–50 (B)
//!   its octant record;
//! - octant records (3 / 11, u16): a byte mask of the brick's 2 m octants
//!   (`x & 8 | z & 8 << 1 | y & 8 << 2`, as bits 0, 1, 2), then one u16 per
//!   octant present: an index into
//! - unit records (4 / 12, 4 bytes): a u16 base and a byte mask of the
//!   octant's 1 m units (bits 2 of x, z, y); the unit's entry list is at
//!   base + the units present before it;
//! - entries (5 / 13, u32): either an entry (bit 31 set) or a run (low half
//!   the first entry, bits 16–30 the count). An entry: bits 0–9 (A) / 0–11
//!   (B) the index of its voxel mask, bits 10–21 (A) / 12–27 (B) its links;
//! - links (6 / 14, u32): per entry a header (`n1` bits 0–10, `n2` bits
//!   11–21, `n3` bits 22–31), `n1` words the reader skips, `n2` outgoing
//!   words, `n3` incoming words. A link word: bits 0–25 one bit per
//!   direction (the client's direction table `0x7FF78BD27690`), bits 26–31
//!   added to a running link number;
//! - masks (7 / 15, u64): unit and voxel masks, shared;
//! - one byte per mask (8 / 16), not read.
//!
//! The lookup of a voxel (`0x7FF78A5FB310`, `0x7FF789AECFF0`) walks the tree
//! to the first entry whose voxel mask holds it.

use std::{
    collections::HashMap,
    fs::File,
    io::{self, BufReader, Read},
    path::{Path, PathBuf},
};

use flate2::read::GzDecoder;

pub type Vec3 = [f32; 3];

/// A voxel's width (x, z).
pub const VOXEL: f32 = 0.25;
/// A voxel's height: 22.4 / 64 truncated to 1/4096 m, as the client's grid.
pub const VOXEL_HEIGHT: f32 = 1433.0 / 4096.0;
/// Voxels per cell side.
const SIDE: i32 = 64;
/// A cell's width (x, z) and height.
const CELL: f32 = VOXEL * SIDE as f32;
const CELL_HEIGHT: f32 = VOXEL_HEIGHT * SIDE as f32;
const MAGIC: &[u8; 8] = b"ORPHAIC1";
const HEADER: usize = 160;

/// A map's AI cell file under the runtime folder.
pub fn file(runtime: &Path, map_guid: u64) -> PathBuf {
    runtime
        .join("data/aicells")
        .join(format!("{:04X}.bin.gz", map_guid as u32))
}

/// Which of a block's two graphs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Graph {
    /// Graph A: the voxels just above walkable floors.
    Walk,
    /// Graph B: the free space.
    Air,
}

/// Where a graph's parts sit in a block and how its words are packed.
struct Layout {
    root: usize,
    bricks: usize,
    octants: usize,
    units: usize,
    entries: usize,
    links: usize,
    masks: usize,
    brick_mask: (u32, u64),
    brick_record: (u32, u64),
    entry_mask: u32,
    entry_links: (u32, u32),
}

const WALK: Layout = Layout {
    root: 136,
    bricks: 1,
    octants: 3,
    units: 4,
    entries: 5,
    links: 6,
    masks: 7,
    brick_mask: (8, 0x3FF),
    brick_record: (29, 0xFFF),
    entry_mask: 0x3FF,
    entry_links: (10, 0xFFF),
};

const AIR: Layout = Layout {
    root: 144,
    bricks: 10,
    octants: 11,
    units: 12,
    entries: 13,
    links: 14,
    masks: 15,
    brick_mask: (8, 0xFFF),
    brick_record: (36, 0x7FFF),
    entry_mask: 0xFFF,
    entry_links: (12, 0xFFFF),
};

impl Graph {
    fn layout(self) -> &'static Layout {
        match self {
            Self::Walk => &WALK,
            Self::Air => &AIR,
        }
    }
}

/// A voxel as the client's cursor holds it (`0x7FF78A61D3B0`): per axis the
/// cell's biased key field times 64 plus the voxel within the cell (x and z
/// biased by 2048 cells, y by 128).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Voxel {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl Voxel {
    /// The voxel a point is in (`0x7FF78A619180`).
    pub fn of(point: Vec3) -> Self {
        let inv_cell = 1.0 / (VOXEL * SIDE as f32);
        let inv_cell_height = 1.0 / (VOXEL_HEIGHT * SIDE as f32);
        // `0x7FF78A618CA0`: truncate, one less below zero.
        let cell = |value: f32, inv: f32| {
            let truncated = (value * inv) as i32;
            if value < 0.0 {
                truncated - 1
            } else {
                truncated
            }
        };
        let (cx, cy, cz) = (
            cell(point[0], inv_cell),
            cell(point[1], inv_cell_height),
            cell(point[2], inv_cell),
        );
        let local =
            |value: f32, origin: f32, inv: f32| ((value - origin) * inv).clamp(0.0, 63.0) as i32;
        let size = VOXEL * SIDE as f32;
        let height = VOXEL_HEIGHT * SIDE as f32;
        Self {
            x: ((cx + 2048) & 0xFFF) * SIDE + local(point[0], cx as f32 * size, 1.0 / VOXEL),
            y: ((cy + 128) & 0xFF) * SIDE + local(point[1], cy as f32 * height, 1.0 / VOXEL_HEIGHT),
            z: ((cz + 2048) & 0xFFF) * SIDE + local(point[2], cz as f32 * size, 1.0 / VOXEL),
        }
    }

    /// The key of the voxel's cell.
    pub fn key(self) -> u32 {
        (self.x >> 6) as u32 & 0xFFF
            | ((self.z >> 6) as u32 & 0xFFF) << 12
            | ((self.y >> 6) as u32) << 24
    }

    /// The point the client gives a voxel (`0x7FF78A61A7E0` without bounds):
    /// the middle of its floor, 4 mm up, in the client's order of operations
    /// (the offset in the cell first, then the cell's corner).
    pub fn floor(self) -> Vec3 {
        let corner = |index: i32, bias: i32, size: f32| ((index >> 6) - bias) as f32 * size;
        let local = |index: i32, size: f32| (index & 63) as f32 * size;
        [
            (local(self.x, VOXEL) + VOXEL * 0.5) + corner(self.x, 2048, CELL),
            (local(self.y, VOXEL_HEIGHT) + 0.004) + corner(self.y, 128, CELL_HEIGHT),
            (local(self.z, VOXEL) + VOXEL * 0.5) + corner(self.z, 2048, CELL),
        ]
    }

    /// The voxel within its unit, `y | z << 2 | x << 4` of the low 2 bits.
    pub fn sub(self) -> u8 {
        (self.y & 3 | (self.z & 3) << 2 | (self.x & 3) << 4) as u8
    }
}

/// Where a lookup found a voxel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    /// The brick node's index in the graph's brick lists.
    pub brick: u32,
    /// The node's place in its brick's list.
    pub slot: u32,
    /// The entry's index in the graph's entries.
    pub entry: u32,
    /// The entry's place in its unit's list.
    pub layer: u32,
}

impl Hit {
    /// The client's node of the unit (`0x7FF78A5FB310`'s fourth argument):
    /// kind 3, the unit's coordinates in the cell (x bits 2–5, z 6–9, y
    /// 10–13), the entry's place `layer + 8 × slot` from bit 14, the cell key
    /// in the high word.
    pub fn node(self, voxel: Voxel) -> u64 {
        let unit = |value: i32| (value as u64 & 0x3C) >> 2;
        3 | unit(voxel.x) << 2
            | unit(voxel.z) << 6
            | unit(voxel.y) << 10
            | (u64::from(self.layer) + 8 * u64::from(self.slot)) << 14 & 0xFFFF_C000
            | u64::from(voxel.key()) << 32
    }
}

/// One cell's byte block.
struct Block {
    bytes: Box<[u8]>,
    sections: [usize; 17],
}

impl Block {
    fn new(bytes: Vec<u8>) -> io::Result<Self> {
        if bytes.len() < HEADER {
            return Err(invalid("a block is shorter than its header"));
        }
        let mut sections = [0; 17];
        let mut last = HEADER;
        for (index, section) in sections.iter_mut().enumerate() {
            let offset = u64::from_le_bytes(bytes[8 * index..8 * index + 8].try_into().unwrap());
            let offset =
                usize::try_from(offset).map_err(|_| invalid("a section past the block"))?;
            if offset < last || offset > bytes.len() {
                return Err(invalid("a block's sections are out of order"));
            }
            *section = offset;
            last = offset;
        }
        Ok(Self {
            bytes: bytes.into_boxed_slice(),
            sections,
        })
    }

    fn word<const N: usize>(&self, at: usize) -> Option<[u8; N]> {
        self.bytes.get(at..at + N)?.try_into().ok()
    }

    fn u64(&self, section: usize, index: usize) -> Option<u64> {
        self.word(self.sections[section] + 8 * index)
            .map(u64::from_le_bytes)
    }

    fn u32(&self, section: usize, index: usize) -> Option<u32> {
        self.word(self.sections[section] + 4 * index)
            .map(u32::from_le_bytes)
    }

    fn u16(&self, section: usize, index: usize) -> Option<u16> {
        self.word(self.sections[section] + 2 * index)
            .map(u16::from_le_bytes)
    }

    fn u8(&self, section: usize, index: usize) -> Option<u8> {
        self.bytes.get(self.sections[section] + index).copied()
    }

    fn root(&self, layout: &Layout) -> u64 {
        u64::from_le_bytes(self.word(layout.root).expect("the header holds the masks"))
    }

    /// A list of nodes: one node (`flag` set) or a run `{first, count}`.
    fn run(word: u64, index: usize, flag: u64, half: u32) -> (usize, usize) {
        if word & flag != 0 {
            (index, 1)
        } else {
            let low = (word & ((1 << half) - 1)) as usize;
            let count = ((word >> half) & ((flag >> half) - 1)) as usize;
            (low, count)
        }
    }

    /// `0x7FF78A5FB310` / `0x7FF789AECFF0` over one graph, the voxel given
    /// by its 6-bit coordinates in the cell.
    fn find(&self, layout: &Layout, voxel: [u8; 3]) -> Option<Hit> {
        let [x, y, z] = voxel.map(u64::from);
        let voxel_bit = 1u64 << (y & 3 | (z & 3) << 2 | (x & 3) << 4);
        let mut found = None;
        self.unit_entries(layout, voxel, |hit, entry| {
            let mask = self.mask(layout, entry);
            if mask & voxel_bit != 0 {
                found = Some(hit);
            }
            found.is_some()
        });
        found
    }

    /// The voxel mask of an entry word.
    fn mask(&self, layout: &Layout, entry: u32) -> u64 {
        self.u64(layout.masks, (entry & layout.entry_mask) as usize)
            .unwrap_or(0)
    }

    /// Every entry of the unit holding a voxel (6-bit coordinates in the
    /// cell), brick node by brick node, in the lookup's order, until `visit`
    /// returns true.
    fn unit_entries(
        &self,
        layout: &Layout,
        [x, y, z]: [u8; 3],
        mut visit: impl FnMut(Hit, u32) -> bool,
    ) -> Option<()> {
        let index = |shift: u8| {
            let bits = |value: u8| u64::from((value >> shift) & 3);
            bits(y) | bits(z) << 2 | bits(x) << 4
        };
        let root = self.root(layout);
        let brick_bit = 1u64 << index(4);
        if root & brick_bit == 0 {
            return None;
        }
        let rank = (root & (brick_bit - 1)).count_ones() as usize;
        let (first, count) = Self::run(self.u64(layout.bricks, rank)?, rank, 1 << 63, 32);
        let octant = |bit: u8| {
            u8::from(x & bit != 0) | u8::from(z & bit != 0) << 1 | u8::from(y & bit != 0) << 2
        };
        let unit_bit = 1u64 << index(2);
        for slot in 0..count {
            let brick = first + slot;
            let node = self.u64(layout.bricks, brick)?;
            let unit_mask = (node >> layout.brick_mask.0) & layout.brick_mask.1;
            if self.u64(layout.masks, unit_mask as usize)? & unit_bit == 0 {
                continue;
            }
            let record = ((node >> layout.brick_record.0) & layout.brick_record.1) as usize;
            let octants = self.u16(layout.octants, record)? as u8;
            let below = (octants & ((1 << octant(8)) - 1)).count_ones() as usize;
            let units = self.u16(layout.octants, record + below + 1)? as usize;
            let base = self.u16(layout.units, 2 * units)? as usize;
            let present = self.u8(layout.units, 4 * units + 2)?;
            let at = base + (present & ((1 << octant(4)) - 1)).count_ones() as usize;
            let list = self.u32(layout.entries, at)?;
            let (first_entry, entries) = Self::run(u64::from(list), at, 1 << 31, 16);
            for layer in 0..entries {
                let entry = self.u32(layout.entries, first_entry + layer)?;
                let hit = Hit {
                    brick: brick as u32,
                    slot: slot as u32,
                    entry: (first_entry + layer) as u32,
                    layer: layer as u32,
                };
                if visit(hit, entry) {
                    return Some(());
                }
            }
        }
        Some(())
    }

    /// The links record of an entry: its words after the header, split into
    /// the implicit, outgoing and incoming words.
    fn links(&self, layout: &Layout, entry: u32) -> Option<[&[u8]; 3]> {
        let at = ((entry >> layout.entry_links.0) & layout.entry_links.1) as usize;
        let header = self.u32(layout.links, at)?;
        let counts = [header & 0x7FF, (header >> 11) & 0x7FF, header >> 22].map(|n| n as usize);
        let mut start = self.sections[layout.links] + 4 * (at + 1);
        let mut part = |count: usize| {
            let words = self.bytes.get(start..start + 4 * count);
            start += 4 * count;
            words
        };
        Some([part(counts[0])?, part(counts[1])?, part(counts[2])?])
    }
}

/// A map's AI cells.
pub struct AiCells {
    blocks: Vec<Block>,
    cells: HashMap<u32, usize>,
}

impl std::fmt::Debug for AiCells {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AiCells")
            .field("cells", &self.cells.len())
            .finish()
    }
}

impl AiCells {
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::read(BufReader::new(File::open(path)?))
    }

    /// Read a gzipped file.
    pub fn read(input: impl Read) -> io::Result<Self> {
        let mut bytes = Vec::new();
        GzDecoder::new(input).read_to_end(&mut bytes)?;
        Self::from_bytes(&bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> io::Result<Self> {
        let mut input = Input { bytes, at: 0 };
        if input.take(MAGIC.len())? != MAGIC {
            return Err(invalid("not an AI cell file"));
        }
        let block_count = input.u32()?;
        let mut blocks = Vec::new();
        for _ in 0..block_count {
            let length = input.u32()? as usize;
            blocks.push(Block::new(input.take(length)?.to_vec())?);
        }
        let cell_count = input.u32()?;
        let mut cells = HashMap::new();
        for _ in 0..cell_count {
            let key = input.u32()?;
            let block = input.u32()? as usize;
            if block >= blocks.len() {
                return Err(invalid("a cell names a block past the end"));
            }
            cells.insert(key, block);
        }
        Ok(Self { blocks, cells })
    }

    /// How many cells the map has.
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    fn block(&self, key: u32) -> Option<&Block> {
        self.cells.get(&key).map(|&block| &self.blocks[block])
    }

    /// Whether the map has the cell with this key.
    pub fn has_cell(&self, key: u32) -> bool {
        self.cells.contains_key(&key)
    }

    /// Where `graph` holds `voxel`, `None` when it doesn't.
    pub fn find(&self, graph: Graph, voxel: Voxel) -> Option<Hit> {
        let local = |value: i32| (value & 63) as u8;
        self.block(voxel.key())?.find(
            graph.layout(),
            [local(voxel.x), local(voxel.y), local(voxel.z)],
        )
    }

    /// The node (unit entry) of `graph` that holds `voxel`.
    pub fn node(&self, graph: Graph, voxel: Voxel) -> Option<Node> {
        let hit = self.find(graph, voxel)?;
        Some(Node {
            unit: [voxel.x >> 2, voxel.y >> 2, voxel.z >> 2],
            place: hit.layer + 8 * hit.slot,
            entry: hit.entry,
        })
    }

    /// A node's voxel mask, bit `y | z << 2 | x << 4` of the voxel in its
    /// unit.
    pub fn mask(&self, graph: Graph, node: Node) -> u64 {
        let layout = graph.layout();
        self.block(node.key())
            .and_then(|block| {
                let entry = block.u32(layout.entries, node.entry as usize)?;
                Some(block.mask(layout, entry))
            })
            .unwrap_or(0)
    }

    /// The nodes a node links to, each once ([`Self::linked`] without the
    /// directions and repeats), into `out` (cleared first).
    pub fn neighbours(&self, graph: Graph, node: Node, out: &mut Vec<Node>) {
        let mut linked = Vec::new();
        self.linked(graph, node, &mut linked);
        out.clear();
        for (next, _) in linked {
            if !out.contains(&next) {
                out.push(next);
            }
        }
    }

    /// The nodes a node links to and the direction of each link, in the
    /// order of the client's neighbour walk (`0x7FF78A6195F0` /
    /// `0x7FF78A612590`, every direction), into `out` (cleared first):
    /// - an implicit word's directions lead to the unit beside in the same
    ///   cell (the client changes only the unit's coordinates in the cell),
    ///   to the entry whose place there is the running link number, when
    ///   there is one;
    /// - an outgoing word's directions lead to the unit beside (in any cell),
    ///   to each of its entries whose incoming words have the running number
    ///   with the opposite direction (`0x7FF78A61B840` / `0x7FF78A612A10`
    ///   list them first, each node once);
    /// - the walk gives one listed outgoing link whenever it runs out of
    ///   implicit words, or a word's last direction finds no entry, and the
    ///   rest once the implicit words are done.
    pub fn linked(&self, graph: Graph, node: Node, out: &mut Vec<(Node, u8)>) {
        out.clear();
        let layout = graph.layout();
        let Some(block) = self.block(node.key()) else {
            return;
        };
        let Some(entry) = block.u32(layout.entries, node.entry as usize) else {
            return;
        };
        let Some([implicit, outgoing, _]) = block.links(layout, entry) else {
            return;
        };
        let listed = self.outgoing(layout, node, outgoing);
        let mut listed = listed.into_iter();
        let mut implicit = words(implicit);
        let (mut number, mut directions) = (0, 0u32);
        loop {
            if directions == 0 {
                for word in implicit.by_ref() {
                    number += word >> 26;
                    directions = word & 0x3FF_FFFF;
                    if directions != 0 {
                        break;
                    }
                }
                if directions == 0 {
                    match listed.next() {
                        Some(link) => {
                            out.push(link);
                            continue;
                        }
                        None => return,
                    }
                }
            }
            let direction = directions.trailing_zeros() as usize;
            directions &= directions - 1;
            let step = DIRECTIONS[direction];
            let unit: [i32; 3] = std::array::from_fn(|axis| {
                node.unit[axis] & !15 | (node.unit[axis] + step[axis]) & 15
            });
            let mut found = None;
            block.unit_entries(layout, unit_voxel(unit), |hit, _| {
                let place = hit.layer + 8 * hit.slot;
                if place == number {
                    found = Some(Node {
                        unit,
                        place,
                        entry: hit.entry,
                    });
                }
                found.is_some()
            });
            match found {
                Some(next) => out.push((next, direction as u8)),
                None if directions == 0 => {
                    if let Some(link) = listed.next() {
                        out.push(link);
                    }
                }
                None => {}
            }
        }
    }

    /// The outgoing links' nodes and directions, each node once, in the
    /// order the client lists them.
    fn outgoing(&self, layout: &Layout, node: Node, outgoing: &[u8]) -> Vec<(Node, u8)> {
        let mut listed: Vec<(Node, u8)> = Vec::new();
        let mut number = 0;
        for word in words(outgoing) {
            number += word >> 26;
            for direction in bits(word) {
                let step = DIRECTIONS[direction];
                let unit: [i32; 3] = std::array::from_fn(|axis| node.unit[axis] + step[axis]);
                let target = Node {
                    unit,
                    place: 0,
                    entry: 0,
                };
                let Some(other) = self.block(target.key()) else {
                    continue;
                };
                let back = OPPOSITE[direction];
                other.unit_entries(layout, unit_voxel(unit), |hit, entry| {
                    let linked = other.links(layout, entry).is_some_and(|[_, _, incoming]| {
                        let mut theirs = 0;
                        words(incoming).any(|word| {
                            theirs += word >> 26;
                            theirs == number && word & 1 << back != 0
                        })
                    });
                    let next = Node {
                        unit,
                        place: hit.layer + 8 * hit.slot,
                        entry: hit.entry,
                    };
                    if linked && !listed.iter().any(|&(listed, _)| listed == next) {
                        listed.push((next, direction as u8));
                    }
                    false
                });
            }
        }
        listed
    }

    /// The node of `graph` at a unit with a place (`0x7FF78A5FAE90` /
    /// `0x7FF78A5FCAA0` by the client's node).
    pub fn node_at(&self, graph: Graph, unit: [i32; 3], place: u32) -> Option<Node> {
        let layout = graph.layout();
        let probe = Node {
            unit,
            place,
            entry: 0,
        };
        let mut found = None;
        self.block(probe.key())?
            .unit_entries(layout, unit_voxel(unit), |hit, _| {
                if hit.layer + 8 * hit.slot == place {
                    found = Some(Node {
                        entry: hit.entry,
                        ..probe
                    });
                }
                found.is_some()
            });
        found
    }

    /// A node's entry word.
    fn entry_word(&self, graph: Graph, node: Node) -> u32 {
        self.block(node.key())
            .and_then(|block| block.u32(graph.layout().entries, node.entry as usize))
            .unwrap_or(0)
    }
}

/// A node of a graph: one entry of a 1 m unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Node {
    /// The unit, per axis the voxel's coordinate / 4 ([`Voxel`]).
    pub unit: [i32; 3],
    /// The entry's place in its unit (`layer + 8 × slot`, the client's).
    pub place: u32,
    /// The entry's index in its block's entries.
    pub entry: u32,
}

impl Node {
    /// The key of the node's cell.
    pub fn key(self) -> u32 {
        Voxel {
            x: self.unit[0] << 2,
            y: self.unit[1] << 2,
            z: self.unit[2] << 2,
        }
        .key()
    }

    /// The client's node (kind 3, as its lookups give it): the unit's
    /// coordinates in the cell (x bits 2–5, z 6–9, y 10–13), the place from
    /// bit 14, the cell key in the high word.
    pub fn client_id(self) -> u64 {
        let [x, y, z] = self.unit.map(|value| (value & 15) as u64);
        3 | x << 2 | z << 6 | y << 10 | u64::from(self.place) << 14 | u64::from(self.key()) << 32
    }

    /// A voxel of the node's unit: `sub` is `y | z << 2 | x << 4` within it.
    pub fn voxel(self, sub: u32) -> Voxel {
        let [x, y, z] = self.unit;
        Voxel {
            x: x << 2 | (sub >> 4) as i32 & 3,
            y: y << 2 | sub as i32 & 3,
            z: z << 2 | (sub >> 2) as i32 & 3,
        }
    }
}

/// The 26 directions of the links (`0x7FF78BD27690`, x, y, z), and each
/// one's opposite (`0x7FF78BD26780`).
const DIRECTIONS: [[i32; 3]; 26] = [
    [0, 0, 1],
    [1, 0, 1],
    [1, 0, 0],
    [1, 0, -1],
    [0, 1, 0],
    [0, 1, 1],
    [1, 1, 1],
    [1, 1, 0],
    [1, 1, -1],
    [0, -1, 1],
    [1, -1, 1],
    [1, -1, 0],
    [1, -1, -1],
    [-1, 0, 1],
    [-1, 0, 0],
    [-1, 0, -1],
    [0, 0, -1],
    [-1, 1, 1],
    [-1, 1, 0],
    [-1, 1, -1],
    [0, 1, -1],
    [0, -1, 0],
    [-1, -1, 1],
    [-1, -1, 0],
    [-1, -1, -1],
    [0, -1, -1],
];
const OPPOSITE: [usize; 26] = [
    16, 15, 14, 13, 21, 25, 24, 23, 22, 20, 19, 18, 17, 3, 2, 1, 0, 12, 11, 10, 9, 4, 8, 7, 6, 5,
];

/// The 6-bit voxel coordinates in its cell of a unit's first voxel.
fn unit_voxel(unit: [i32; 3]) -> [u8; 3] {
    let [x, y, z] = unit.map(|value| ((value & 15) << 2) as u8);
    [x, y, z]
}

fn words(bytes: &[u8]) -> impl Iterator<Item = u32> + '_ {
    bytes
        .chunks_exact(4)
        .map(|word| u32::from_le_bytes(word.try_into().unwrap()))
}

/// The direction bits of a link word.
fn bits(word: u32) -> impl Iterator<Item = usize> {
    let mut left = word & 0x3FF_FFFF;
    std::iter::from_fn(move || {
        (left != 0).then(|| {
            let bit = left.trailing_zeros() as usize;
            left &= left - 1;
            bit
        })
    })
}

/// A cursor over the file's bytes.
struct Input<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Input<'a> {
    fn take(&mut self, length: usize) -> io::Result<&'a [u8]> {
        let slice = self
            .bytes
            .get(self.at..self.at + length)
            .ok_or_else(|| invalid("the file is cut short"))?;
        self.at += length;
        Ok(slice)
    }

    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
}

fn invalid(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

mod ledge;
mod query;
mod walk;

pub use ledge::{
    LedgePath, Refusal, START_SEARCH, ledge_path, lift, lift_goal, mode_one, polyline,
};
pub use query::{End, Step, client_path};
pub use walk::WalkSearch;

#[cfg(test)]
mod tests;
