//! Predicted projectile leases and owner delivery for build 104319.
//!
//! Ported from ow174/game/projectiles.py. A report comes from the exact
//! datagram that carried an owner frame. Cumulative script acknowledgements
//! alone do not confirm a projectile.

use std::{collections::HashSet, io, sync::Arc};

use indexmap::IndexMap;
use serde_json::Value as Json;

use crate::{projectile_wire, replication::BodyId};
use orpheus_core::{bits::BitWriter, tick::MAX_LEASE_FRAME};
use orpheus_physics::static_ray::Vec3;
use orpheus_script::{
    expr::EvalError,
    graph::{GraphStore, Node, asset_guid},
    snapshot::ProjectedSnapshot,
    stream::Stream,
    vm::{ShotEvent, Snapshot},
    weapons::authored_number,
    wire::Value,
};
use orpheus_wire::entity::{EntityUpdate, LeaseBlock, Operation, SkinTheme};

const LEASE_FIRST: u32 = 0x0010_0000;
const LEASE_SPAN: u32 = 1 << 22;
const LEASE_WINDOWS: u32 = 2;
const DEFINITIONS_PER_BODY: usize = 4;
const PREDICTED_ID: u32 = 0xa000_0000;
const ID_LIMIT: u32 = 0x2000_0000;
const REPORT_WINDOW: u32 = 64;
const UNREPORTED_TICKS: u32 = 32;
const RELEASE_MARGIN: u32 = 6;
const RELEASE_AHEAD: u32 = 4096;
pub(crate) const DELIVERY_GRACE: u32 = 64;
const MAX_PROJECTILES: usize = 256;

#[derive(Clone, Debug, Default)]
pub struct ProjectileLeases {
    blocks: Vec<LeaseBlock>,
}

impl ProjectileLeases {
    pub fn new(
        slot: u8,
        frame: u32,
        definitions: impl IntoIterator<Item = u64>,
    ) -> io::Result<Self> {
        if frame > MAX_LEASE_FRAME {
            return Err(invalid("projectile leases cannot cross the command epoch"));
        }
        let mut blocks = Vec::new();
        let start = frame - frame % LEASE_SPAN;
        for definition in definitions {
            if blocks.len() == DEFINITIONS_PER_BODY * LEASE_WINDOWS as usize {
                break;
            }
            if definition == 0
                || blocks
                    .iter()
                    .any(|block: &LeaseBlock| block.definition == definition)
            {
                continue;
            }
            let index = blocks.len() / LEASE_WINDOWS as usize;
            let body = u32::from(slot) * DEFINITIONS_PER_BODY as u32 + index as u32;
            let first_id = LEASE_FIRST + body * LEASE_SPAN;
            if first_id > ID_LIMIT - LEASE_SPAN {
                return Err(invalid("projectile leases exceed the body slot range"));
            }
            for window in 0..LEASE_WINDOWS {
                let start = start + window * LEASE_SPAN;
                blocks.push(LeaseBlock {
                    definition,
                    start,
                    stop: start + LEASE_SPAN,
                    first_id,
                    per_frame: 1,
                });
            }
        }
        Ok(Self { blocks })
    }

    /// Inspect authored volley candidates in instance, state and field order.
    /// A conditional projectile field can name several predicted definitions.
    pub fn from_authored(
        graphs: &GraphStore,
        snapshot: &Snapshot,
        slot: u8,
        frame: u32,
        mut evaluate: impl FnMut(u16, u16, &Json) -> Result<Value, EvalError>,
    ) -> io::Result<Self> {
        let mut definitions = Vec::new();
        for (&instance, state) in &snapshot.instances {
            let graph = graphs
                .graph(state.graph)
                .ok_or_else(|| invalid("missing volley graph"))?;
            for raw in 0..graph.states.len() {
                let Some(node) = graph.state(raw) else {
                    continue;
                };
                if node.class != "STUStatescriptStateWeaponVolley" || !local_prediction(node) {
                    continue;
                }
                let raw = u16::try_from(raw).map_err(io::Error::other)?;
                if predicted_sync(node, |config| evaluate(instance, raw, config))
                    .map_err(io::Error::other)?
                {
                    if let Some(field) = node.field("m_projectileEntity") {
                        projectile_definitions(field, &mut definitions);
                    }
                }
            }
        }
        Self::new(slot, frame, definitions)
    }

    pub fn blocks(&self) -> &[LeaseBlock] {
        &self.blocks
    }

    pub fn entity(&self, definition: u64, frame: u32, index: u8) -> Option<u32> {
        let block = self.blocks.iter().find(|block| {
            block.definition == definition
                && (block.start..block.stop).contains(&frame)
                && index < block.per_frame
        })?;
        Some(
            PREDICTED_ID
                | (block.first_id
                    + (frame - block.start) * u32::from(block.per_frame)
                    + u32::from(index)),
        )
    }
}

pub(crate) fn local_prediction(node: &Node) -> bool {
    node.field("m_45271E8C").is_some_and(|value| {
        value
            .as_bool()
            .unwrap_or_else(|| value.as_i64().is_some_and(|value| value != 0))
    })
}

pub fn predicted_sync(
    node: &Node,
    mut evaluate: impl FnMut(&Json) -> Result<Value, EvalError>,
) -> Result<bool, EvalError> {
    let mut field = |name, default| authored_number(node.field(name), default, &mut evaluate);
    let explicit = field("m_5E4A4E85", 0.0)? as i64;
    if explicit != 0 {
        return Ok(explicit == 1);
    }
    if field("m_A17BE89B", 1.0)? >= 2.0 && field("m_numShotsPerSecond", 0.0)? >= 5.5 {
        return Ok(false);
    }
    Ok(field("m_projectileLifetime", 0.0)? >= 1.19e-5
        && field("m_numProjectilesPerShot", 1.0)? <= 7.0)
}

fn projectile_definitions(value: &Json, out: &mut Vec<u64>) {
    match value {
        Json::Object(fields) => {
            if fields.get("$").and_then(Json::as_str) == Some("STU_8556841E") {
                if let Some(definition) = fields.get("m_entityDef").and_then(asset_guid) {
                    if !out.contains(&definition) {
                        out.push(definition);
                    }
                }
            }
            for value in fields.values() {
                projectile_definitions(value, out);
            }
        }
        Json::Array(values) => {
            for value in values {
                projectile_definitions(value, out);
            }
        }
        _ => {}
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ProjectileToken(u64);

#[derive(Clone, Copy, Debug)]
struct OwnerReport {
    frame: u32,
    introduces: bool,
    arrived: Option<bool>,
}

struct PendingProjectile {
    token: ProjectileToken,
    shot: ShotEvent,
    definition: u64,
    bound: u32,
    reports: IndexMap<u32, OwnerReport>,
    creates: IndexMap<u32, bool>,
    created_at: Option<u32>,
    release_at: Option<u32>,
    /// The Biotic Field it leaves, and the shooter's feet when he fired
    /// (projectiles.py `Shot.spawn`, `Shot.where`).
    field: Option<(FieldSchedule, [f64; 3])>,
}

/// What the match does about an owner's projectile besides its own
/// records (projectiles.py `_end` for the other clients' copies, and
/// `_field`). A field's entities go when its shot is no longer live
/// ([`OwnerProjectiles::fields`]).
#[derive(Clone, Debug, PartialEq)]
pub enum ProjectileEvent {
    /// The shot ended (`_end`): the other clients' copies finish.
    Ended {
        token: ProjectileToken,
        explode: bool,
    },
    /// The field heals this tick (`heal_area`).
    FieldHeal {
        token: ProjectileToken,
        centre: [f64; 3],
        amount: f64,
    },
    /// Every client gets the field once (`_deliver_field`): the owner's
    /// parented to each of his canisters that arrived, the others' placed at
    /// the shooter's feet. A field that already faded sends its faded frame.
    FieldDeliver {
        token: ProjectileToken,
        feet: [f64; 3],
        parents: Vec<u32>,
        faded: bool,
    },
    /// The field faded: its frames go again without st5.
    FieldFaded { token: ProjectileToken },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectileDelivery {
    Confirm {
        token: ProjectileToken,
        entity: u32,
        definition: u64,
    },
    Destroy {
        token: ProjectileToken,
        entity: u32,
        explosion_frame: Option<u32>,
    },
}

impl ProjectileDelivery {
    pub fn update(self) -> Result<EntityUpdate, orpheus_core::bits::BitError> {
        match self {
            Self::Confirm {
                entity, definition, ..
            } => Ok(projectile_wire::confirmation(entity, definition)),
            Self::Destroy {
                entity,
                explosion_frame,
                ..
            } => Ok(EntityUpdate {
                entity,
                operation: Some(Operation::Destroy),
                ch2: explosion_frame
                    .map(|frame| projectile_wire::release(frame, 1))
                    .transpose()?,
                ..EntityUpdate::default()
            }),
        }
    }
}

/// One owner body generation's confirmation state. Keep it with the body
/// stream and retire it when that body is replaced or loses authority.
pub struct OwnerProjectiles {
    body: u32,
    leases: ProjectileLeases,
    pending: Vec<PendingProjectile>,
    occupied: HashSet<u32>,
    next: u64,
    events: Vec<ProjectileEvent>,
}

impl OwnerProjectiles {
    pub fn new(body: u32, leases: ProjectileLeases) -> Self {
        Self {
            body,
            leases,
            pending: Vec::new(),
            occupied: HashSet::new(),
            next: 0,
            events: Vec::new(),
        }
    }

    /// A Biotic Field canister (projectiles.py `fired` with a spawn): it
    /// is confirmed as any projectile, then its field runs on the field's
    /// clock instead of a release.
    pub fn admit_field(
        &mut self,
        shot: ShotEvent,
        definition: u64,
        flight_bound_ticks: u32,
        feet: [f64; 3],
    ) -> Option<ProjectileToken> {
        let token = self.admit(shot, definition, flight_bound_ticks)?;
        if let Some(pending) = self.pending.last_mut() {
            pending.field = Some((FieldSchedule::new(), feet));
        }
        Some(token)
    }

    /// What the match must do since the last call.
    pub fn take_events(&mut self) -> Vec<ProjectileEvent> {
        std::mem::take(&mut self.events)
    }

    /// The Biotic Fields still running (projectiles.py `shots` with a
    /// spawn): the other shots' field entities go.
    pub fn fields(&self) -> impl Iterator<Item = ProjectileToken> + '_ {
        self.pending
            .iter()
            .filter(|pending| pending.field.is_some())
            .map(|pending| pending.token)
    }

    pub fn admit(
        &mut self,
        shot: ShotEvent,
        definition: u64,
        flight_bound_ticks: u32,
    ) -> Option<ProjectileToken> {
        if shot.entity != self.body || self.pending.len() == MAX_PROJECTILES {
            return None;
        }
        self.leases.entity(definition, shot.frame, 0)?;
        if self
            .pending
            .iter()
            .any(|active| active.definition == definition && active.shot.frame == shot.frame)
        {
            return None; // the advertised lease has one projectile ID per definition and frame
        }
        self.next = self.next.checked_add(1)?;
        let token = ProjectileToken(self.next);
        self.pending.push(PendingProjectile {
            token,
            shot,
            definition,
            bound: flight_bound_ticks,
            reports: IndexMap::new(),
            creates: IndexMap::new(),
            created_at: None,
            release_at: None,
            field: None,
        });
        Some(token)
    }

    /// Call only for the first successful send of a logical owner report.
    pub fn owner_sent(
        &mut self,
        last: u32,
        frame: u32,
        identity: impl Fn(u16, u16) -> Option<(u16, u64)>,
        introduces: impl Fn(u16, u16) -> bool,
    ) {
        for pending in &mut self.pending {
            let shot = &pending.shot;
            if (shot.frame..=shot.frame.saturating_add(REPORT_WINDOW)).contains(&frame)
                && identity(shot.instance, shot.raw_state)
                    == Some((shot.activation, shot.activation_ms))
            {
                pending.reports.entry(last).or_insert(OwnerReport {
                    frame,
                    introduces: introduces(shot.instance, shot.raw_state),
                    arrived: None,
                });
            }
        }
    }

    pub fn owner_arrival(&mut self, last: u32, arrived: bool) {
        for pending in &mut self.pending {
            if let Some(report) = pending.reports.get_mut(&last) {
                if report.arrived.is_none() {
                    report.arrived = Some(arrived);
                }
            }
        }
    }

    pub fn create_arrived(&mut self, token: ProjectileToken, entity: u32) {
        if let Some(pending) = self
            .pending
            .iter_mut()
            .find(|pending| pending.token == token)
        {
            if let Some(arrived) = pending.creates.get_mut(&entity) {
                *arrived = true;
            }
        }
    }

    pub fn confirmation_current(&self, token: ProjectileToken, entity: u32) -> bool {
        self.pending
            .iter()
            .any(|pending| pending.token == token && pending.creates.contains_key(&entity))
    }

    pub fn flight_ended(&mut self, token: ProjectileToken, travelled_ticks: u32, tick: u32) {
        let Some(pending) = self
            .pending
            .iter_mut()
            .find(|pending| pending.token == token)
        else {
            return;
        };
        if pending.release_at.is_none() && pending.field.is_none() {
            let start = pending
                .reports
                .values()
                .find(|report| report.arrived != Some(false))
                .map_or(pending.shot.frame, |report| report.frame);
            pending.release_at = Some(
                start
                    .saturating_add(travelled_ticks)
                    .saturating_add(RELEASE_MARGIN)
                    .max(tick),
            );
        }
    }

    /// One update (projectiles.py `Projectiles.update`) at `tick`, whose
    /// retained combat frames start at `first_tick` (combat.py `update`).
    pub fn advance(&mut self, tick: u32, first_tick: u32) -> Vec<ProjectileDelivery> {
        let mut out = Vec::new();
        let events = &mut self.events;
        let leases = &self.leases;
        let occupied = &mut self.occupied;
        self.pending.retain_mut(|pending| {
            pending
                .reports
                .retain(|_, report| report.arrived != Some(false));
            if let Some((schedule, feet)) = &mut pending.field {
                // projectiles.py `_field`.
                let (feet, token) = (*feet, pending.token);
                let step = schedule.step(tick, first_tick);
                if step.confirm {
                    confirm(leases, occupied, pending, tick, &mut out);
                }
                if step.heal > 0.0 {
                    events.push(ProjectileEvent::FieldHeal {
                        token,
                        centre: field_centre(feet),
                        amount: step.heal,
                    });
                }
                if step.expired {
                    // `_end(explode=False)`: its fields go with the shot.
                    retire(pending, occupied, None, &mut out);
                    return false;
                }
                if step.faded_now {
                    events.push(ProjectileEvent::FieldFaded { token });
                }
                if step.deliver {
                    let parents = pending
                        .creates
                        .iter()
                        .filter(|&(_, &arrived)| arrived)
                        .map(|(&entity, _)| entity)
                        .collect();
                    events.push(ProjectileEvent::FieldDeliver {
                        token,
                        feet,
                        parents,
                        faded: step.faded,
                    });
                }
                return true;
            }
            let due = pending.release_at.or_else(|| {
                pending.created_at.map(|created| {
                    created
                        .saturating_add(pending.bound)
                        .saturating_add(RELEASE_MARGIN)
                })
            });
            if due.is_some_and(|due| tick >= due.saturating_add(DELIVERY_GRACE)) {
                retire(pending, occupied, Some(tick), &mut out);
                events.push(ProjectileEvent::Ended {
                    token: pending.token,
                    explode: true,
                });
                return false;
            }
            confirm(leases, occupied, pending, tick, &mut out);
            if due.is_some_and(|due| tick >= due)
                && !pending.creates.is_empty()
                && pending.creates.values().all(|&arrived| arrived)
            {
                retire(pending, occupied, Some(tick), &mut out);
                events.push(ProjectileEvent::Ended {
                    token: pending.token,
                    explode: true,
                });
                return false;
            }
            true
        });
        out
    }

    /// The body's projectiles end with it (projectiles.py `_end` with
    /// `explode=alive` false): no explosion, and its fields go.
    pub fn retire(&mut self) -> Vec<ProjectileDelivery> {
        let mut out = Vec::new();
        for pending in &self.pending {
            retire(pending, &mut self.occupied, None, &mut out);
            if pending.field.is_none() {
                self.events.push(ProjectileEvent::Ended {
                    token: pending.token,
                    explode: false,
                });
            }
        }
        self.pending.clear();
        out
    }

    pub(crate) fn cancel(&mut self, token: ProjectileToken, out: &mut Vec<ProjectileDelivery>) {
        if let Some(index) = self
            .pending
            .iter()
            .position(|pending| pending.token == token)
        {
            let pending = self.pending.remove(index);
            retire(&pending, &mut self.occupied, None, out);
            if pending.field.is_none() {
                self.events.push(ProjectileEvent::Ended {
                    token,
                    explode: false,
                });
            }
        }
    }
}

/// projectiles.py `_confirm`: once an owner frame that reported the shot
/// arrived (or none did for UNREPORTED_TICKS), a create of each leased id
/// the client may have shot it under.
fn confirm(
    leases: &ProjectileLeases,
    occupied: &mut HashSet<u32>,
    pending: &mut PendingProjectile,
    tick: u32,
    out: &mut Vec<ProjectileDelivery>,
) {
    let reports = pending
        .reports
        .values()
        .filter(|report| report.arrived == Some(true));
    let has_report = reports.clone().next().is_some();
    let fallback = pending.reports.is_empty()
        && pending.creates.is_empty()
        && tick >= pending.shot.frame.saturating_add(UNREPORTED_TICKS);
    if !has_report && !fallback {
        return;
    }
    let frames: Vec<_> = std::iter::once(pending.shot.frame)
        .chain(
            reports
                .filter(|report| report.introduces)
                .map(|report| report.frame),
        )
        .collect();
    for frame in frames {
        let Some(entity) = leases.entity(pending.definition, frame, 0) else {
            continue;
        };
        if occupied.insert(entity) {
            pending.creates.insert(entity, false);
            pending.created_at.get_or_insert(tick);
            out.push(ProjectileDelivery::Confirm {
                token: pending.token,
                entity,
                definition: pending.definition,
            });
        }
    }
}

fn retire(
    pending: &PendingProjectile,
    occupied: &mut HashSet<u32>,
    tick: Option<u32>,
    out: &mut Vec<ProjectileDelivery>,
) {
    for (&entity, &arrived) in &pending.creates {
        occupied.remove(&entity);
        out.push(ProjectileDelivery::Destroy {
            token: pending.token,
            entity,
            explosion_frame: tick
                .filter(|_| arrived)
                .map(|tick| tick.saturating_add(RELEASE_AHEAD)),
        });
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

// --- the Biotic Field (ow174/game/projectiles.py) ---------------------------

const FRAME_MS: u32 = 16;
/// 03DC.003, the Biotic Field (0257 st1's m_projectileSpawn).
pub const FIELD_ENTITY: u64 = 0x0400_0000_0000_03dc;
/// The field entity's initial graph.
pub const FIELD_GRAPH: u32 = 0x0258;
/// The field entities' ids: FIELD_FIRST + n % FIELD_SPAN.
pub const FIELD_FIRST: u32 = 0xa008_0000;
pub const FIELD_SPAN: u32 = 0x0004_0000;
/// The server projectiles the other clients see: SEEN_FIRST + n % SEEN_SPAN.
pub const SEEN_FIRST: u32 = 0xa00c_0000;
pub const SEEN_SPAN: u32 = 0x0004_0000;
/// Approximate deployment delay in world ticks after the cast projectile is
/// admitted.
pub const FIELD_DELAY: u32 = 12;
/// 0258: #567 = 5 s, st1 waits #567 - 0.5 then st4 waits 0.5, then
/// DestroyEntity.
pub const FIELD_SECONDS: f64 = 5.0;
/// ceil(FIELD_SECONDS * 1000 / FRAME_MS).
pub const FIELD_TICKS: u32 = 5000_u32.div_ceil(FRAME_MS);
/// 0258 st1 ends: st5 (its DataFlowMapping) goes off.
pub const FADE_SECONDS: f64 = 4.5;
/// 0258 #420 = 35 a player (ApplyAura 025F.053's 002A; per second unverified).
pub const HEAL_PER_SECOND: f64 = 35.0;
/// 0258 #422 = 5: st0 TrackTargets m_radius.
pub const HEAL_RADIUS: f64 = 5.0;
/// TrackTargets' offset (0, 1, 0) from the field (the sphere's centre is
/// unverified).
pub const HEAL_UP: f64 = 1.0;

/// The entities a volley's projectile leaves where it lands (`_defs` of the
/// volley's `m_49CBA3BC` in `Projectiles.fired`); the first one is the
/// shot's spawn.
pub fn spawned_definitions(node: &Node) -> Vec<u64> {
    let mut found = Vec::new();
    if let Some(field) = node.field("m_49CBA3BC") {
        projectile_definitions(field, &mut found);
    }
    found
}

/// The heal sphere's centre over the shooter's feet (`field_centre`).
pub fn field_centre(where_feet: [f64; 3]) -> [f64; 3] {
    let [x, y, z] = where_feet;
    [x, y + HEAL_UP, z]
}

/// What one `Projectiles._field` step asks of the caller, in this order:
/// confirm the owner's canister, heal, then (unless expired) deliver the
/// field entities and resend the field frames if it faded now.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FieldStep {
    /// tick < expires: run the owner confirmation (`_confirm`).
    pub confirm: bool,
    /// The first step at or after the start: the field opened.
    pub opened: bool,
    /// The heal amount for this step (`heal_area`), 0 when none.
    pub heal: f64,
    /// The fade crossed on this step: resend every arrived field frame.
    pub faded_now: bool,
    pub faded: bool,
    /// Started and not expired: create the field entities (`_deliver_field`).
    pub deliver: bool,
    /// tick >= expires: end the shot without an explosion.
    pub expired: bool,
}

/// The Biotic Field's clock (`Shot.field_due`, `field_tick`, `faded` and
/// `Projectiles._field`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FieldSchedule {
    due: Option<u32>,
    opened: bool,
    faded: bool,
}

impl FieldSchedule {
    pub fn new() -> Self {
        Self::default()
    }

    /// One update at `tick` covering the ticks from `first_tick` on. The
    /// start is the first step's tick + FIELD_DELAY; the opening tick has no
    /// heal and the field ends before its expiry tick.
    pub fn step(&mut self, tick: u32, first_tick: u32) -> FieldStep {
        let start = *self.due.get_or_insert(tick + FIELD_DELAY);
        let expires = start + FIELD_TICKS;
        let mut step = FieldStep {
            confirm: tick < expires,
            faded: self.faded,
            ..FieldStep::default()
        };
        if tick < start {
            return step;
        }
        step.opened = !self.opened;
        self.opened = true;
        let steps =
            i64::from(tick.min(expires - 1)) - (i64::from(first_tick) - 1).max(i64::from(start));
        if steps > 0 {
            step.heal = HEAL_PER_SECOND * f64::from(FRAME_MS) / 1000.0 * steps as f64;
        }
        if tick >= expires {
            step.expired = true;
            return step;
        }
        let seconds = f64::from(tick - start) * f64::from(FRAME_MS) / 1000.0;
        if !self.faded && seconds >= FADE_SECONDS {
            self.faded = true;
            step.faded_now = true;
            step.faded = true;
        }
        step.deliver = true;
        step
    }

    pub fn faded(&self) -> bool {
        self.faded
    }
}

/// The Biotic Field entities' ids (`Projectiles.next_field`), one counter
/// for the room.
#[derive(Clone, Copy, Debug, Default)]
pub struct FieldIds {
    next: u32,
}

impl FieldIds {
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> u32 {
        // FIELD_SPAN divides 2^32, so the wrapping counter keeps Python's sequence.
        let entity = FIELD_FIRST + self.next % FIELD_SPAN;
        self.next = self.next.wrapping_add(1);
        entity
    }
}

/// 0258's two full frames (`field_frame`): while the field is on, and once
/// it faded.
#[derive(Clone, Debug)]
pub struct FieldFrames {
    plain: BitWriter,
    faded: BitWriter,
}

impl FieldFrames {
    pub fn new(
        graphs: &GraphStore,
        codecs: &orpheus_script::codecs::CodecTable,
    ) -> io::Result<Self> {
        Ok(Self {
            plain: projectile_wire::field_frame(graphs, codecs, false)?,
            faded: projectile_wire::field_frame(graphs, codecs, true)?,
        })
    }

    fn frame(&self, faded: bool) -> &BitWriter {
        if faded { &self.faded } else { &self.plain }
    }
}

/// A shot as the room names it: its owner's body and token.
pub(crate) type ShotKey = (BodyId, ProjectileToken);

/// What a [`FieldCopies`] record is, for its acknowledgement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FieldDelivery {
    Create,
    Frame { last: u32 },
    Destroy,
}

/// One client's Biotic Field entities (projectiles.py `Shot.field_ids` and
/// `FieldArrived`): one per shot and parent, created once; its frame goes
/// when its create arrived and again when the field fades; they all go when
/// the shot ends (`_end`).
#[derive(Debug, Default)]
pub(crate) struct FieldCopies {
    copies: Vec<FieldCopy>,
    /// Field entities whose destroy has not arrived.
    destroys: HashSet<u32>,
}

#[derive(Debug)]
struct FieldCopy {
    shot: ShotKey,
    parent: Option<u32>,
    entity: u32,
    /// The field's statescript on this client (`Stream(entity)`).
    stream: Stream,
    frames: Arc<FieldFrames>,
    arrived: bool,
    faded: bool,
}

impl FieldCopy {
    /// `FieldArrived.send`: the field's full frame as it is now.
    fn frame(&mut self) -> io::Result<(EntityUpdate, FieldDelivery)> {
        let chunk = self
            .stream
            .full(self.frames.frame(self.faded))
            .map_err(io::Error::other)?;
        Ok((chunk.update, FieldDelivery::Frame { last: chunk.last }))
    }
}

/// One shot's field as `_deliver_field` creates it for a client.
pub(crate) struct FieldDeliver<'a> {
    pub shot: ShotKey,
    /// The owner's canisters that arrived, or `[None]`: at the feet.
    pub parents: &'a [Option<u32>],
    pub feet: [f64; 3],
    /// The caster's skin theme.
    pub skin: SkinTheme,
    pub faded: bool,
    pub frames: &'a Arc<FieldFrames>,
}

impl FieldCopies {
    /// `_deliver_field` for this client: a create for each parent it has
    /// no field of the shot for yet.
    pub(crate) fn deliver(
        &mut self,
        field: &FieldDeliver<'_>,
        ids: &mut FieldIds,
    ) -> Vec<(EntityUpdate, FieldDelivery)> {
        let mut out = Vec::new();
        for &parent in field.parents {
            if self
                .copies
                .iter()
                .any(|copy| copy.shot == field.shot && copy.parent == parent)
            {
                continue;
            }
            let entity = ids.next();
            self.destroys.remove(&entity);
            self.copies.push(FieldCopy {
                shot: field.shot,
                parent,
                entity,
                stream: Stream::new(entity),
                frames: Arc::clone(field.frames),
                arrived: false,
                faded: field.faded,
            });
            out.push((
                projectile_wire::field_create(entity, parent, field.feet, field.skin),
                FieldDelivery::Create,
            ));
        }
        out
    }

    /// The field's create arrived (`FieldArrived.arrived`): its frame goes,
    /// the first time only.
    pub(crate) fn created(
        &mut self,
        entity: u32,
    ) -> io::Result<Option<(EntityUpdate, FieldDelivery)>> {
        let Some(copy) = self
            .copies
            .iter_mut()
            .find(|copy| copy.entity == entity && !copy.arrived)
        else {
            return Ok(None);
        };
        copy.arrived = true;
        copy.frame().map(Some)
    }

    /// The shot's field faded: the frames of its arrived copies go again
    /// without st5, and the others' go faded when they arrive.
    pub(crate) fn fade(&mut self, shot: ShotKey) -> io::Result<Vec<(EntityUpdate, FieldDelivery)>> {
        let mut out = Vec::new();
        for copy in self.copies.iter_mut().filter(|copy| copy.shot == shot) {
            copy.faded = true;
            if copy.arrived {
                out.push(copy.frame()?);
            }
        }
        Ok(out)
    }

    pub(crate) fn frame_arrived(&mut self, entity: u32, last: u32) {
        if let Some(copy) = self.copies.iter_mut().find(|copy| copy.entity == entity) {
            copy.stream.arrived(last);
        }
    }

    /// `_end`: the fields of the shots that are no longer live go.
    pub(crate) fn end(
        &mut self,
        live: impl Fn(ShotKey) -> bool,
    ) -> Vec<(EntityUpdate, FieldDelivery)> {
        let mut out = Vec::new();
        self.copies.retain(|copy| {
            if live(copy.shot) {
                return true;
            }
            self.destroys.insert(copy.entity);
            out.push((
                EntityUpdate {
                    entity: copy.entity,
                    operation: Some(Operation::Destroy),
                    ..EntityUpdate::default()
                },
                FieldDelivery::Destroy,
            ));
            false
        });
        out
    }

    pub(crate) fn destroy_arrived(&mut self, entity: u32) {
        self.destroys.remove(&entity);
    }

    /// A create or frame stays current while its field lives; a destroy
    /// until it arrived.
    pub(crate) fn current(&self, delivery: FieldDelivery, entity: u32) -> bool {
        match delivery {
            FieldDelivery::Create | FieldDelivery::Frame { .. } => {
                self.copies.iter().any(|copy| copy.entity == entity)
            }
            FieldDelivery::Destroy => self.destroys.contains(&entity),
        }
    }
}

/// A player's projectile the other clients see (projectiles.py `fired`
/// without a spawn, for `_show_others`).
#[derive(Clone, Debug)]
pub struct FiredProjectile {
    pub(crate) body: BodyId,
    pub(crate) token: ProjectileToken,
    pub(crate) shot: ShotEvent,
    pub(crate) definition: u64,
    /// The volley state's `volleys & 0xFF`.
    pub(crate) volleys: u16,
    pub(crate) origin: [f64; 3],
    pub(crate) direction: Vec3,
    /// The ticks it flies at most (`Shot.bound`).
    pub(crate) bound: u32,
}

// --- observed projectiles (`_show_others`, `_ObservedProjectile`) -----------

/// The ids of the server projectiles the other clients see
/// (`Projectiles.next_seen`).
#[derive(Clone, Copy, Debug, Default)]
pub struct SeenIds {
    next: u32,
}

impl SeenIds {
    pub fn new() -> Self {
        Self::default()
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> u32 {
        // SEEN_SPAN divides 2^32, so the wrapping counter keeps Python's sequence.
        let entity = SEEN_FIRST + self.next % SEEN_SPAN;
        self.next = self.next.wrapping_add(1);
        entity
    }
}

/// One output of `ObservedProjectile::advance`, in queue order.
#[derive(Debug, PartialEq)]
pub enum ObservedOut {
    /// The projectile entity's create (`confirm_record`), under the map
    /// until the launch starts it.
    Create(EntityUpdate),
    /// The shooter's ch2 shot record naming the entity.
    Launch(EntityUpdate),
    /// Withdraw the queued, unacknowledged launch (`cancel_entity_record`).
    CancelLaunch,
    /// The projectile entity's destroy, with the release when it exploded.
    Destroy(EntityUpdate),
    /// Drop this delivery.
    Done,
}

/// One viewer's create, shooter event and retirement of a player's remote
/// projectile (`_ObservedProjectile`; the bot volley retention is not here).
#[derive(Debug)]
pub struct ObservedProjectile {
    body: u32,
    entity: u32,
    definition: u64,
    frame: u32,
    instance: u16,
    state: u16,
    activation: u16,
    volleys: u16,
    origin: [f32; 3],
    direction: Vec3,
    launch: EntityUpdate,
    deadline: u32,
    pending_create: bool,
    launch_queued: bool,
    creation_arrived: bool,
    launch_arrived: bool,
    finished: bool,
    explode: bool,
}

impl ObservedProjectile {
    /// The shot record of `_show_others`: the shot's frame, volley instance,
    /// state and full activation counter, the volley state's `volleys & 0xFF`,
    /// shot counter 0, and this viewer's entity. `deadline` is the tick plus
    /// the shot's bound plus DELIVERY_GRACE.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        body: u32,
        entity: u32,
        definition: u64,
        shot: &ShotEvent,
        volleys: u16,
        origin: [f32; 3],
        direction: Vec3,
        deadline: u32,
    ) -> io::Result<Self> {
        let volleys = volleys & 0xff;
        let entities = [entity];
        let record = projectile_wire::ShotRecord {
            frame: shot.frame,
            instance: shot.instance,
            state: shot.raw_state,
            activation: shot.activation,
            ordinal: 0,
            volleys,
            origin,
            direction,
            entities: &entities,
        };
        let launch = EntityUpdate {
            entity: body,
            ch2: Some(projectile_wire::shot_events(&[record])?),
            ..EntityUpdate::default()
        };
        Ok(Self {
            body,
            entity,
            definition,
            frame: shot.frame,
            instance: shot.instance,
            state: shot.raw_state,
            activation: shot.activation,
            volleys,
            origin,
            direction,
            launch,
            deadline,
            pending_create: true,
            launch_queued: false,
            creation_arrived: false,
            launch_arrived: false,
            finished: false,
            explode: false,
        })
    }

    /// `_show_others`: the create goes at once when the viewer's stream has
    /// a baseline, else it waits for one in `advance`.
    pub fn open(&mut self, stream_has_base: bool) -> Option<ObservedOut> {
        (self.pending_create && stream_has_base).then(|| {
            self.pending_create = false;
            ObservedOut::Create(projectile_wire::confirmation(self.entity, self.definition))
        })
    }

    pub fn body(&self) -> u32 {
        self.body
    }

    pub fn entity(&self) -> u32 {
        self.entity
    }

    pub fn frame(&self) -> u32 {
        self.frame
    }

    pub fn origin(&self) -> [f32; 3] {
        self.origin
    }

    pub fn direction(&self) -> Vec3 {
        self.direction
    }

    /// The volley key (instance, raw state) the launch needs on the viewer.
    pub fn volley_key(&self) -> (u16, u16) {
        (self.instance, self.state)
    }

    /// Whether the viewer's acknowledged H=0 baseline holds this shot's
    /// volley (`_volley_arrived`).
    pub fn volley_arrived(&self, base: &ProjectedSnapshot) -> bool {
        volley_matches(
            base,
            self.instance,
            self.state,
            self.activation,
            self.volleys,
        )
    }

    pub fn creation_arrived(&mut self) {
        self.creation_arrived = true;
    }

    pub fn launch_arrived(&mut self) {
        self.launch_arrived = true;
    }

    /// The shot ended (`_ObservedProjectile.finish`): an explosion or a drain
    /// keeps DELIVERY_GRACE ticks for the launch, else it retires now.
    pub fn finish(&mut self, tick: u32, explode: bool, drain: bool) {
        self.finished = true;
        self.explode = explode;
        let grace = if explode || drain { DELIVERY_GRACE } else { 0 };
        self.deadline = self.deadline.min(tick.saturating_add(grace));
    }

    /// `_ObservedProjectile.advance`. Ending (the source stream changed, the
    /// deadline, or finished after the launch arrived) yields
    /// [CancelLaunch if launched but unacknowledged], [Destroy unless the
    /// create never went], Done. Otherwise [Create once the stream has a
    /// baseline], [Launch once the create arrived and the baseline holds the
    /// volley].
    pub fn advance(
        &mut self,
        tick: u32,
        source_live: bool,
        stream_has_base: bool,
        volley_arrived: bool,
    ) -> Vec<ObservedOut> {
        let mut out = Vec::new();
        let launched = self.launch_arrived;
        if !source_live || tick >= self.deadline || (self.finished && launched) {
            if self.launch_queued && !launched {
                out.push(ObservedOut::CancelLaunch);
            }
            if !self.pending_create {
                let ch2 = (source_live && self.explode && launched).then(|| {
                    projectile_wire::release(tick.wrapping_add(RELEASE_AHEAD), 1)
                        .expect("a one-impact release fits its fields")
                });
                out.push(ObservedOut::Destroy(EntityUpdate {
                    entity: self.entity,
                    operation: Some(Operation::Destroy),
                    ch2,
                    ..EntityUpdate::default()
                }));
            }
            out.push(ObservedOut::Done);
            return out;
        }
        if let Some(create) = self.open(stream_has_base) {
            out.push(create);
        }
        if !self.launch_queued && self.creation_arrived && volley_arrived {
            self.launch_queued = true;
            out.push(ObservedOut::Launch(self.launch.clone()));
        }
        out
    }
}

/// `_ObservedProjectile.matching_volley`: the baseline's state at
/// (instance, state) has payload counter == the activation and
/// `volleys & 0xFF` == the record's volleys.
pub fn volley_matches(
    projected: &ProjectedSnapshot,
    instance: u16,
    state: u16,
    activation: u16,
    volleys: u16,
) -> bool {
    let Some(payload) = projected.state_payload(instance, state) else {
        return false;
    };
    let counter = match payload.get("counter") {
        Some(Value::Integer(counter)) => *counter,
        _ => return false,
    };
    let seen = match payload.get("volleys") {
        Some(Value::Integer(volleys)) => *volleys,
        None => 0,
        _ => return false,
    };
    counter == i64::from(activation) && seen & 0xff == i64::from(volleys)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orpheus_script::vm::{
        Component, EmptyWorld, MAX_INIT_WORK, player_entity_variables, player_roots,
    };
    use std::sync::Arc;

    const BODY: u32 = 0xa000_0101;
    const CANISTER: u64 = 0x0400_0000_0000_072d;
    const VISOR: u64 = 0x0400_0000_0000_1c86;
    const HELIX: u64 = 0x0400_0000_0000_072c;

    fn shot(frame: u32) -> ShotEvent {
        ShotEvent {
            entity: BODY,
            instance: 11,
            graph: 0x0254,
            raw_state: 30,
            frame,
            time_ms: u64::from(frame) * 16,
            activation: 2,
            activation_ms: u64::from(frame) * 16,
            ordinal: 0,
        }
    }

    #[test]
    fn lease_windows_follow_body_slots_and_reserve_one_entity_per_frame() {
        let frame = LEASE_SPAN + 1234;
        let leases = ProjectileLeases::new(0, frame, [CANISTER, CANISTER, VISOR, HELIX]).unwrap();
        assert_eq!(leases.blocks().len(), 6);
        assert_eq!(
            leases.blocks()[4],
            LeaseBlock {
                definition: HELIX,
                start: LEASE_SPAN,
                stop: 2 * LEASE_SPAN,
                first_id: 0x0090_0000,
                per_frame: 1,
            }
        );
        assert_eq!(leases.entity(HELIX, frame, 0), Some(0xa090_04d2));
        assert_eq!(
            leases.entity(HELIX, frame + LEASE_SPAN, 0),
            Some(0xa090_04d2)
        );
        assert_eq!(leases.entity(HELIX, frame - LEASE_SPAN, 0), None);
        assert_eq!(leases.entity(HELIX, frame, 1), None);
        let next = ProjectileLeases::new(1, frame, [CANISTER, VISOR, HELIX]).unwrap();
        assert_eq!(next.entity(HELIX, frame, 0), Some(0xa190_04d2));
        assert!(ProjectileLeases::new(u8::MAX, frame, [HELIX]).is_err());
        assert!(ProjectileLeases::new(0, MAX_LEASE_FRAME + 1, [HELIX]).is_err());
        assert!(ProjectileLeases::new(0, MAX_LEASE_FRAME, [HELIX]).is_ok());
    }

    #[test]
    #[ignore = "requires the derived 104319 statescript catalog"]
    fn authored_soldier_leases_match_python_and_exclude_the_rifle() {
        let root = orpheus_testkit::runtime_dir();
        let graphs = Arc::new(GraphStore::load(root.join("data/game_graphs_174.json.gz")).unwrap());
        let body = &graphs.bodies()[&0x0400_0000_0000_03cf];
        let mut component = Component::start(
            Arc::clone(&graphs),
            BODY,
            player_entity_variables(body, 200.0, 200.0),
            player_roots(body, &graphs).unwrap(),
            1000,
            &mut EmptyWorld,
            MAX_INIT_WORK,
        )
        .unwrap();
        component.advance_frame(1000, &mut EmptyWorld).unwrap();
        let leases = ProjectileLeases::from_authored(
            &graphs,
            component.snapshot().unwrap(),
            0,
            1000,
            |instance, raw, config| {
                component.evaluate_state_config(instance, raw, config, &mut EmptyWorld)
            },
        )
        .unwrap();
        assert_eq!(
            leases
                .blocks()
                .chunks_exact(2)
                .map(|windows| windows[0].definition)
                .collect::<Vec<_>>(),
            [CANISTER, VISOR, HELIX]
        );
        assert_eq!(leases.entity(0x0400_0000_0000_03d4, 1000, 0), None);
        assert_eq!(leases.entity(HELIX, 1234, 0), Some(0xa090_04d2));
    }

    #[test]
    fn only_a_delivered_owner_introduction_confirms_an_alias_and_release_waits_for_creates() {
        let frame = 1000;
        let shot = shot(frame);
        let leases = ProjectileLeases::new(0, frame, [CANISTER, VISOR, HELIX]).unwrap();
        let original = leases.entity(HELIX, frame, 0).unwrap();
        let alias = leases.entity(HELIX, frame + 4, 0).unwrap();
        let mut tracker = OwnerProjectiles::new(BODY, leases);
        let token = tracker.admit(shot, HELIX, 80).unwrap();
        assert!(tracker.admit(shot, HELIX, 80).is_none());
        let identity = |instance, state| {
            (instance == shot.instance && state == shot.raw_state)
                .then_some((shot.activation, shot.activation_ms))
        };
        tracker.owner_sent(2, frame + 2, identity, |_, _| true);
        tracker.owner_sent(3, frame + 4, identity, |_, _| true);
        tracker.owner_sent(4, frame + 5, identity, |_, _| false);
        assert!(tracker.advance(frame + 5, frame + 5).is_empty());
        tracker.owner_arrival(2, false);
        tracker.owner_arrival(3, true);
        tracker.owner_arrival(4, true);
        assert_eq!(
            tracker.advance(frame + 5, frame + 5),
            [
                ProjectileDelivery::Confirm {
                    token,
                    entity: original,
                    definition: HELIX
                },
                ProjectileDelivery::Confirm {
                    token,
                    entity: alias,
                    definition: HELIX
                },
            ]
        );
        tracker.flight_ended(token, 2, frame + 5);
        tracker.create_arrived(token, original);
        assert!(tracker.advance(frame + 12, frame + 12).is_empty());
        assert!(tracker.confirmation_current(token, alias));
        tracker.create_arrived(token, alias);
        let explosion_frame = Some(frame + 13 + RELEASE_AHEAD);
        assert_eq!(
            tracker.advance(frame + 13, frame + 13),
            [
                ProjectileDelivery::Destroy {
                    token,
                    entity: original,
                    explosion_frame
                },
                ProjectileDelivery::Destroy {
                    token,
                    entity: alias,
                    explosion_frame
                },
            ]
        );
        assert!(!tracker.confirmation_current(token, original));
        assert!(tracker.advance(frame + 14, frame + 14).is_empty());
    }

    #[test]
    fn lost_reports_fall_back_and_unacknowledged_confirmation_destroys_without_explosion() {
        let frame = 1000;
        let shot = shot(frame);
        let leases = ProjectileLeases::new(0, frame, [HELIX]).unwrap();
        let entity = leases.entity(HELIX, frame, 0).unwrap();
        let mut tracker = OwnerProjectiles::new(BODY, leases);
        let token = tracker.admit(shot, HELIX, 20).unwrap();
        tracker.owner_sent(
            2,
            frame + 1,
            |_, _| Some((shot.activation, shot.activation_ms)),
            |_, _| true,
        );
        tracker.owner_arrival(2, false);
        tracker.flight_ended(token, 2, frame + 2);
        let late = frame + UNREPORTED_TICKS - 1;
        assert!(tracker.advance(late, late).is_empty());
        assert_eq!(
            tracker.advance(frame + UNREPORTED_TICKS, frame + UNREPORTED_TICKS),
            [ProjectileDelivery::Confirm {
                token,
                entity,
                definition: HELIX
            }]
        );
        let grace = frame + 8 + DELIVERY_GRACE;
        assert!(tracker.advance(grace - 1, grace - 1).is_empty());
        assert_eq!(
            tracker.advance(grace, grace),
            [ProjectileDelivery::Destroy {
                token,
                entity,
                explosion_frame: None
            }]
        );
        tracker.create_arrived(token, entity);
        assert!(!tracker.confirmation_current(token, entity));
        assert!(tracker.retire().is_empty());
    }

    #[test]
    fn field_schedule_follows_the_python_clock() {
        let mut field = FieldSchedule::new();
        let step = field.step(100, 100);
        assert_eq!(
            step,
            FieldStep {
                confirm: true,
                ..FieldStep::default()
            }
        );
        let start = 100 + FIELD_DELAY;
        let step = field.step(start, start);
        assert!(step.opened && step.deliver && step.confirm && !step.expired);
        assert_eq!(step.heal, 0.0);
        // Skipped ticks heal for every tick after the start.
        let step = field.step(start + 3, start + 1);
        assert!(!step.opened);
        assert_eq!(step.heal, HEAL_PER_SECOND * 16.0 / 1000.0 * 3.0);
        assert!(!field.step(start + 281, start + 281).faded);
        let step = field.step(start + 282, start + 282);
        assert!(step.faded_now && step.faded && step.deliver);
        let step = field.step(start + 283, start + 283);
        assert!(!step.faded_now && step.faded);
        assert_eq!(FIELD_TICKS, 313);
        let step = field.step(start + FIELD_TICKS, start + FIELD_TICKS - 1);
        assert!(step.expired && !step.confirm && !step.deliver);
        assert_eq!(step.heal, HEAL_PER_SECOND * 16.0 / 1000.0);
        assert_eq!(field_centre([1.0, 2.0, 3.0]), [1.0, 3.0, 3.0]);
    }

    #[test]
    fn seen_ids_wrap_within_their_span() {
        let mut ids = SeenIds::new();
        assert_eq!((ids.next(), ids.next()), (SEEN_FIRST, SEEN_FIRST + 1));
        ids.next = SEEN_SPAN - 1;
        assert_eq!(
            (ids.next(), ids.next()),
            (SEEN_FIRST + SEEN_SPAN - 1, SEEN_FIRST)
        );
    }

    fn observed(deadline: u32) -> ObservedProjectile {
        ObservedProjectile::new(
            BODY,
            SEEN_FIRST,
            HELIX,
            &shot(1000),
            0x101,
            [1.0, 2.0, 3.0],
            [0.0, 0.0, 1.0],
            deadline,
        )
        .unwrap()
    }

    fn destroy(out: &ObservedOut) -> &EntityUpdate {
        let ObservedOut::Destroy(update) = out else {
            panic!("destroy, got {out:?}")
        };
        assert_eq!(
            (update.entity, &update.operation),
            (SEEN_FIRST, &Some(Operation::Destroy))
        );
        update
    }

    #[test]
    fn observed_projectile_creates_launches_and_explodes() {
        let mut seen = observed(2000);
        assert_eq!(seen.open(false), None);
        assert!(seen.advance(1001, true, false, true).is_empty());
        let out = seen.advance(1002, true, true, true);
        assert_eq!(
            out,
            [ObservedOut::Create(projectile_wire::confirmation(
                SEEN_FIRST, HELIX
            ))]
        );
        // The launch waits for the create and the volley.
        assert!(seen.advance(1003, true, true, true).is_empty());
        seen.creation_arrived();
        assert!(seen.advance(1004, true, true, false).is_empty());
        let out = seen.advance(1005, true, true, true);
        let [ObservedOut::Launch(launch)] = out.as_slice() else {
            panic!("launch, got {out:?}")
        };
        let entities = [SEEN_FIRST];
        let record = projectile_wire::ShotRecord {
            frame: 1000,
            instance: 11,
            state: 30,
            activation: 2,
            ordinal: 0,
            volleys: 1,
            origin: [1.0, 2.0, 3.0],
            direction: [0.0, 0.0, 1.0],
            entities: &entities,
        };
        assert_eq!(launch.entity, BODY);
        assert_eq!(
            launch.ch2.as_ref().unwrap().as_bytes(),
            projectile_wire::shot_events(&[record]).unwrap().as_bytes()
        );
        assert!(seen.advance(1006, true, true, true).is_empty());
        seen.launch_arrived();
        seen.finish(1010, true, false);
        let out = seen.advance(1010, true, true, true);
        assert_eq!(out.len(), 2);
        assert_eq!(
            destroy(&out[0]).ch2.as_ref().unwrap().as_bytes(),
            projectile_wire::release(1010 + RELEASE_AHEAD, 1)
                .unwrap()
                .as_bytes()
        );
        assert_eq!(out[1], ObservedOut::Done);
    }

    #[test]
    fn observed_projectile_retires_without_a_release() {
        // The deadline passes before the launch arrived.
        let mut seen = observed(1050);
        assert!(seen.open(true).is_some());
        seen.creation_arrived();
        assert!(matches!(
            seen.advance(1001, true, true, true).as_slice(),
            [ObservedOut::Launch(_)]
        ));
        let out = seen.advance(1050, true, true, true);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], ObservedOut::CancelLaunch);
        assert_eq!(destroy(&out[1]).ch2, None);
        assert_eq!(out[2], ObservedOut::Done);

        // The shooter's stream went away after an exploding, launched shot.
        let mut seen = observed(2000);
        assert!(seen.open(true).is_some());
        seen.creation_arrived();
        seen.advance(1001, true, true, true);
        seen.launch_arrived();
        seen.finish(1002, true, false);
        let out = seen.advance(1002, false, true, true);
        assert_eq!(destroy(&out[0]).ch2, None);
        assert_eq!(out[1], ObservedOut::Done);

        // A finish without explosion or drain retires at once.
        let mut seen = observed(2000);
        assert!(seen.open(true).is_some());
        seen.finish(1003, false, false);
        let out = seen.advance(1003, true, true, true);
        assert_eq!(destroy(&out[0]).ch2, None);
        assert_eq!(out[1], ObservedOut::Done);

        // Nothing reached the client.
        let mut seen = observed(1050);
        assert_eq!(seen.advance(1050, true, false, false), [ObservedOut::Done]);
    }

    /// projectiles.py `_field`: the field opens FIELD_DELAY after its first
    /// update, heals from the tick after its opening over the retained
    /// frames, delivers with the canisters that arrived, fades at 4.5 s and
    /// ends at FIELD_TICKS, when it is no longer live.
    #[test]
    fn a_field_runs_on_its_clock_and_ends_with_its_shot() {
        let frame = 1000;
        let feet = [1.0, 2.0, 3.0];
        let leases = ProjectileLeases::new(0, frame, [CANISTER]).unwrap();
        let canister = leases.entity(CANISTER, frame, 0).unwrap();
        let mut tracker = OwnerProjectiles::new(BODY, leases);
        let token = tracker
            .admit_field(shot(frame), CANISTER, 10, feet)
            .unwrap();
        assert_eq!(tracker.fields().collect::<Vec<_>>(), [token]);
        let heal = |steps: u32| HEAL_PER_SECOND * f64::from(FRAME_MS) / 1000.0 * f64::from(steps);
        let deliver = |parents: Vec<u32>, faded| ProjectileEvent::FieldDeliver {
            token,
            feet,
            parents,
            faded,
        };
        assert!(tracker.advance(frame, frame).is_empty());
        assert!(tracker.take_events().is_empty());
        let start = frame + FIELD_DELAY;
        tracker.advance(start, start);
        assert_eq!(tracker.take_events(), [deliver(Vec::new(), false)]);
        // Five frames at once: the retained ones heal.
        tracker.advance(start + 5, start + 1);
        assert_eq!(
            tracker.take_events(),
            [
                ProjectileEvent::FieldHeal {
                    token,
                    centre: field_centre(feet),
                    amount: heal(5),
                },
                deliver(Vec::new(), false)
            ]
        );
        // No owner frame reported it: the canister's fallback confirmation.
        let confirm = frame + UNREPORTED_TICKS;
        assert_eq!(
            tracker.advance(confirm, start + 6),
            [ProjectileDelivery::Confirm {
                token,
                entity: canister,
                definition: CANISTER,
            }]
        );
        tracker.take_events();
        tracker.create_arrived(token, canister);
        // Only this update's own frame is retained.
        let faded = start + 282; // 4.512 s
        tracker.advance(faded, faded);
        assert_eq!(
            tracker.take_events(),
            [
                ProjectileEvent::FieldHeal {
                    token,
                    centre: field_centre(feet),
                    amount: heal(1),
                },
                ProjectileEvent::FieldFaded { token },
                deliver(vec![canister], true)
            ]
        );
        // The field ends before its expiry tick.
        let expires = start + FIELD_TICKS;
        assert_eq!(
            tracker.advance(expires, faded + 1),
            [ProjectileDelivery::Destroy {
                token,
                entity: canister,
                explosion_frame: None,
            }]
        );
        assert_eq!(
            tracker.take_events(),
            [ProjectileEvent::FieldHeal {
                token,
                centre: field_centre(feet),
                amount: heal(expires - 1 - faded),
            }]
        );
        assert_eq!(tracker.fields().count(), 0);
    }

    /// projectiles.py `_deliver_field` and `FieldArrived` for one client:
    /// one create per shot and parent; its frame when the create arrived,
    /// once, and again without st5 when the field fades; its destroy when
    /// the shot is no longer live, current until it arrived.
    #[test]
    fn field_copies_follow_their_shot() {
        let mut plain = BitWriter::new();
        plain.bits(0b101, 3).unwrap();
        let mut faded = BitWriter::new();
        faded.bit(true);
        let frames = Arc::new(FieldFrames {
            plain: plain.clone(),
            faded: faded.clone(),
        });
        let body = BodyId {
            entity: BODY,
            generation: 1,
        };
        let shot = (body, ProjectileToken(1));
        let mut ids = FieldIds::default();
        let mut copies = FieldCopies::default();
        let field = FieldDeliver {
            shot,
            parents: &[None],
            feet: [1.0, 2.0, 3.0],
            skin: SkinTheme::default(),
            faded: false,
            frames: &frames,
        };
        let created = copies.deliver(&field, &mut ids);
        assert_eq!(
            created,
            [(
                projectile_wire::field_create(
                    FIELD_FIRST,
                    None,
                    [1.0, 2.0, 3.0],
                    SkinTheme::default()
                ),
                FieldDelivery::Create
            )]
        );
        assert!(copies.deliver(&field, &mut ids).is_empty());
        assert!(copies.current(FieldDelivery::Create, FIELD_FIRST));

        let mut stream = Stream::new(FIELD_FIRST);
        let expected = stream.full(&plain).unwrap();
        assert_eq!(
            copies.created(FIELD_FIRST).unwrap(),
            Some((expected.update, FieldDelivery::Frame { last: 1 }))
        );
        assert_eq!(copies.created(FIELD_FIRST).unwrap(), None);
        copies.frame_arrived(FIELD_FIRST, 1);
        let expected = stream.full(&faded).unwrap();
        assert_eq!(
            copies.fade(shot).unwrap(),
            [(expected.update, FieldDelivery::Frame { last: 2 })]
        );

        assert!(copies.end(|live| live == shot).is_empty());
        let gone: [_; 1] = copies.end(|_| false).try_into().unwrap();
        let [(destroy, FieldDelivery::Destroy)] = gone else {
            panic!("one destroy");
        };
        assert_eq!(
            (destroy.entity, destroy.operation),
            (FIELD_FIRST, Some(Operation::Destroy))
        );
        assert!(!copies.current(FieldDelivery::Create, FIELD_FIRST));
        assert!(copies.current(FieldDelivery::Destroy, FIELD_FIRST));
        copies.destroy_arrived(FIELD_FIRST);
        assert!(!copies.current(FieldDelivery::Destroy, FIELD_FIRST));
        let next = FieldDeliver {
            shot: (body, ProjectileToken(2)),
            ..field
        };
        assert_eq!(copies.deliver(&next, &mut ids)[0].0.entity, FIELD_FIRST + 1);
    }
}
