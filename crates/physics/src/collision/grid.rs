//! XZ columns shared by triangle indexes and static contact proxies.

pub(crate) const CELL_WIDTH: f64 = 2.0;
const LARGE_COLUMNS: u64 = 4096;
const MAX_REFERENCES: usize = 1 << 24;
/// A grid keeps a slot for every column of its box while the box has at most
/// this many columns per reference (or [`DENSE_MIN_COLUMNS`]); geometry spread
/// thinner keeps its occupied columns only.
const DENSE_COLUMNS_PER_REFERENCE: u64 = 4;
const DENSE_MIN_COLUMNS: u64 = 1 << 16;

type Column = (i32, i32);

pub(crate) fn cell(coordinate: f64) -> Option<i32> {
    cell_with_width(coordinate, CELL_WIDTH)
}

fn cell_with_width(coordinate: f64, width: f64) -> Option<i32> {
    let value = (coordinate / width).floor();
    (f64::from(i32::MIN)..=f64::from(i32::MAX))
        .contains(&value)
        .then_some(value as i32)
}

#[derive(Clone, Copy)]
pub(crate) struct ColumnRange {
    first: Column,
    last: Column,
}

impl ColumnRange {
    pub(crate) fn new([low_x, low_z]: [f64; 2], [high_x, high_z]: [f64; 2]) -> Option<Self> {
        Self::for_width([low_x, low_z], [high_x, high_z], CELL_WIDTH)
    }

    pub(crate) fn for_width(
        [low_x, low_z]: [f64; 2],
        [high_x, high_z]: [f64; 2],
        width: f64,
    ) -> Option<Self> {
        if low_x > high_x || low_z > high_z {
            return None;
        }
        Some(Self {
            first: (
                cell_with_width(low_x, width)?,
                cell_with_width(low_z, width)?,
            ),
            last: (
                cell_with_width(high_x, width)?,
                cell_with_width(high_z, width)?,
            ),
        })
    }

    pub(crate) fn expanded(self, cells: i32) -> Self {
        Self {
            first: (
                self.first.0.saturating_sub(cells),
                self.first.1.saturating_sub(cells),
            ),
            last: (
                self.last.0.saturating_add(cells),
                self.last.1.saturating_add(cells),
            ),
        }
    }

    fn contains(self, (x, z): Column) -> bool {
        (self.first.0..=self.last.0).contains(&x) && (self.first.1..=self.last.1).contains(&z)
    }

    fn count(self) -> u64 {
        let width = (i64::from(self.last.0) - i64::from(self.first.0) + 1) as u64;
        let depth = (i64::from(self.last.1) - i64::from(self.first.1) + 1) as u64;
        width.saturating_mul(depth)
    }

    /// Its columns, x-major (sorted).
    fn columns(self) -> impl Iterator<Item = Column> {
        (self.first.0..=self.last.0)
            .flat_map(move |x| (self.first.1..=self.last.1).map(move |z| (x, z)))
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct GridLimit;

/// Collects the geometry of a [`ColumnGrid`], which [`Self::build`] lays out
/// once all of it is in.
pub(crate) struct GridBuilder {
    entries: Vec<(u32, ColumnRange)>,
    large: Vec<u32>,
    references: usize,
    large_limit: u64,
}

impl Default for GridBuilder {
    fn default() -> Self {
        Self::with_large_limit(LARGE_COLUMNS)
    }
}

impl GridBuilder {
    pub(crate) fn with_large_limit(large_limit: u64) -> Self {
        Self {
            entries: Vec::new(),
            large: Vec::new(),
            references: 0,
            large_limit,
        }
    }

    /// Geometry over more than the large limit's columns is a candidate of
    /// every column; the rest of each of its columns.
    pub(crate) fn insert(&mut self, number: u32, range: ColumnRange) -> Result<(), GridLimit> {
        let count = range.count();
        if count > self.large_limit {
            self.large.push(number);
            return Ok(());
        }
        let references = self.references + count as usize;
        if references > MAX_REFERENCES {
            return Err(GridLimit);
        }
        self.references = references;
        self.entries.push((number, range));
        Ok(())
    }

    /// Each column lists its geometry in insertion order, in one array.
    pub(crate) fn build(self) -> ColumnGrid {
        let layout = Layout::of(&self.entries, self.references);
        let mut starts = vec![0u32; layout.len() + 1];
        for &(_, range) in &self.entries {
            layout.for_each_slot(range, |slot| starts[slot + 1] += 1);
        }
        for slot in 1..starts.len() {
            starts[slot] += starts[slot - 1];
        }
        let mut next = starts.clone();
        let mut items = vec![0u32; self.references];
        for &(number, range) in &self.entries {
            layout.for_each_slot(range, |slot| {
                items[next[slot] as usize] = number;
                next[slot] += 1;
            });
        }
        ColumnGrid {
            layout,
            starts,
            items,
            large: self.large,
        }
    }
}

/// Where a column's slot is.
#[derive(Debug)]
enum Layout {
    /// Every column of the box from `first`, `depth` columns along z for
    /// each x, x-major.
    Dense {
        first: Column,
        width: usize,
        depth: usize,
    },
    /// The occupied columns only, sorted.
    Sparse(Vec<Column>),
}

impl Layout {
    fn of(entries: &[(u32, ColumnRange)], references: usize) -> Self {
        let Some(bounds) = entries
            .iter()
            .map(|&(_, range)| range)
            .reduce(|a, b| ColumnRange {
                first: (a.first.0.min(b.first.0), a.first.1.min(b.first.1)),
                last: (a.last.0.max(b.last.0), a.last.1.max(b.last.1)),
            })
        else {
            return Self::Sparse(Vec::new());
        };
        let budget = (references as u64)
            .saturating_mul(DENSE_COLUMNS_PER_REFERENCE)
            .max(DENSE_MIN_COLUMNS);
        if bounds.count() <= budget {
            let width = i64::from(bounds.last.0) - i64::from(bounds.first.0) + 1;
            let depth = i64::from(bounds.last.1) - i64::from(bounds.first.1) + 1;
            return Self::Dense {
                first: bounds.first,
                width: width as usize,
                depth: depth as usize,
            };
        }
        let mut columns: Vec<Column> = entries
            .iter()
            .flat_map(|&(_, range)| range.columns())
            .collect();
        columns.sort_unstable();
        columns.dedup();
        Self::Sparse(columns)
    }

    fn len(&self) -> usize {
        match self {
            Self::Dense { width, depth, .. } => width * depth,
            Self::Sparse(columns) => columns.len(),
        }
    }

    fn slot(&self, (x, z): Column) -> Option<usize> {
        match self {
            Self::Dense {
                first,
                width,
                depth,
            } => {
                let dx = usize::try_from(i64::from(x) - i64::from(first.0)).ok()?;
                let dz = usize::try_from(i64::from(z) - i64::from(first.1)).ok()?;
                (dx < *width && dz < *depth).then(|| dx * depth + dz)
            }
            Self::Sparse(columns) => columns.binary_search(&(x, z)).ok(),
        }
    }

    /// The slots of the range's occupied columns, x-major.
    fn for_each_slot(&self, range: ColumnRange, mut visit: impl FnMut(usize)) {
        match self {
            Self::Dense {
                first,
                width,
                depth,
            } => {
                let clamp = |value: i32, low: i32, count: usize| {
                    (i64::from(value) - i64::from(low)).clamp(-1, count as i64)
                };
                let (x0, x1) = (
                    clamp(range.first.0, first.0, *width).max(0),
                    clamp(range.last.0, first.0, *width).min(*width as i64 - 1),
                );
                let (z0, z1) = (
                    clamp(range.first.1, first.1, *depth).max(0),
                    clamp(range.last.1, first.1, *depth).min(*depth as i64 - 1),
                );
                for dx in x0..=x1 {
                    let row = dx as usize * depth;
                    for dz in z0..=z1 {
                        visit(row + dz as usize);
                    }
                }
            }
            Self::Sparse(columns) => {
                let start = columns.partition_point(|&column| column < range.first);
                for (offset, &column) in columns[start..].iter().enumerate() {
                    if column.0 > range.last.0 {
                        break;
                    }
                    if range.contains(column) {
                        visit(start + offset);
                    }
                }
            }
        }
    }
}

/// Geometry by the XZ columns its bounds overlap ([`GridBuilder`]).
#[derive(Debug)]
pub(crate) struct ColumnGrid {
    layout: Layout,
    /// A slot's geometry is `items[starts[slot]..starts[slot + 1]]`.
    starts: Vec<u32>,
    items: Vec<u32>,
    large: Vec<u32>,
}

impl Default for ColumnGrid {
    fn default() -> Self {
        GridBuilder::default().build()
    }
}

impl ColumnGrid {
    fn slot_items(&self, slot: usize) -> &[u32] {
        &self.items[self.starts[slot] as usize..self.starts[slot + 1] as usize]
    }

    fn regular_column(&self, column: Column) -> &[u32] {
        self.layout
            .slot(column)
            .map_or(&[], |slot| self.slot_items(slot))
    }

    pub(crate) fn column(&self, column: Column) -> impl Iterator<Item = u32> + '_ {
        self.regular_column(column)
            .iter()
            .chain(&self.large)
            .copied()
    }

    /// Visit candidates in first-column order, with oversized geometry at the
    /// end of the first column. Regular IDs may recur in later columns.
    pub(crate) fn visit(&self, range: ColumnRange, mut consider: impl FnMut(u32)) {
        self.column(range.first).for_each(&mut consider);
        let first = self.layout.slot(range.first);
        self.layout.for_each_slot(range, |slot| {
            if Some(slot) != first {
                self.slot_items(slot)
                    .iter()
                    .copied()
                    .for_each(&mut consider);
            }
        });
    }
}
