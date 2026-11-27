//! The entities the players' shots make during the match, each run on its
//! own graphs in the server's spawned-entity world
//! (`lucy_script::placed`): the Biotic Fields their canisters leave,
//! and the server projectiles whose entity starts graphs (Moira's Biotic
//! Orbs).
//!
//! When a field opens (`ProjectileEvent::FieldOpened`) the room spawns
//! `03DC.003` where the shooter stood, owned by his controller; its initial
//! graph `0258` does the rest: st0 TrackTargets finds the allies within
//! `#422` (5 m) of a point 1 m above it, st7 ApplyAura puts `025F` on each
//! (its graph `0B3C` heals the body by `#420 * #567` over `#567` seconds
//! while it is in the field, through the body's own ModifyHealth), st1
//! waits `#567 - 0.5` s and ends st5, st4 waits 0.5 s and the field removes
//! itself (`STU_523EE38D`), which ends its shot. The field's observer frame
//! is what every client's copies of the field show
//! (`projectiles::FieldCopies`).
//!
//! The field stands where the shooter's feet were when he fired
//! (projectiles.py `Shot.where`; assumption: the canister lands there).
//!
//! A server projectile ([`ServerFlight`]: one the shooter's client doesn't
//! shoot itself, `PosedProjectile::server_shown`) whose entity definition
//! starts graphs (`game_spawns_174.json`: the healing orb `1E60.003` starts
//! `10FF`, the damaging `1E61.003` `1100`) is spawned where it leaves, owned
//! by its shooter's controller and shot by his body (component 50,
//! `STU_2B2E2F6D`), and moves with its flight each tick. Its graph searches
//! the bodies around it and puts its aura on them (the orbs' `02A1.053` and
//! `02D5.053`, whose graphs `1102`/`1101` heal or hurt the body through its
//! own ModifyHealth, a player's or a bot's, with the orb as the change's
//! source); what they dealt comes back to it as the source's healing done
//! or damage dealt (`WorldDelivery::dealt`), and once its pool is spent it
//! removes itself, which ends the flight. A flight that ends first (its
//! lifetime, its last bounce) removes it. Its observer frame goes to every
//! client's copy of the projectile (`ObservedProjectile::set_frame`): the
//! client begins the orbs' remote speed state `STU_D3F71FCF` only from it.
//! That state's scale (0.275 while the orb has targets) also slows the
//! server's flight ([`SpawnedEntities::flight_speeds`]; assumption, its
//! client reader is not known).

use std::{collections::BTreeMap, sync::Arc};

use lucy_script::{
    codecs::CodecTable,
    graph::GraphStore,
    mode::EVERY_ENTITY,
    placed::{MapEntityWorld, ProjectileShooter, Spawn},
    snapshot::{ProjectedSnapshot, SnapshotEncoder, View},
    vm::{
        AuraChange, GameMessage, MapRays, MessageEnd, OwnedEntity, TargetCandidate, TrackedStat,
        Unknowns,
    },
};

use crate::{placed_script::PlacedFrame, projectiles::ShotKey};

/// The initial graphs of each entity the game spawns during the match, by
/// entity definition (`game_spawns_174.json`: the Biotic Field `03DC`
/// starts `0258`; the projectile entities' too, the Biotic Orbs `1E60`
/// `10FF` and `1E61` `1100`).
pub type SpawnGraphs = BTreeMap<u64, Vec<u32>>;

/// A server projectile in flight this tick
/// (`WorldDelivery::server_flights`): its shot, its shooter's hero, its
/// entity definition and where it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ServerFlight {
    pub shot: ShotKey,
    pub hero: u64,
    pub definition: u64,
    pub position: [f64; 3],
}

/// One entity the room runs.
struct Spawned {
    shot: ShotKey,
    entity: u32,
    /// A projectile in flight (a Biotic Orb), not an entity a shot left
    /// (a Biotic Field).
    projectile: bool,
    /// Its frame: the projection it was made from and the frame.
    frame: Option<(ProjectedSnapshot, PlacedFrame)>,
}

/// One room's spawned entities.
pub struct SpawnedEntities {
    graphs: Arc<GraphStore>,
    codecs: CodecTable,
    /// The field's initial graphs (`0258`).
    field_graphs: Vec<u32>,
    /// Every spawned entity's initial graphs, by definition: the server
    /// projectiles' are looked up here.
    spawn_graphs: Arc<SpawnGraphs>,
    world: MapEntityWorld,
    spawned: Vec<Spawned>,
    /// An entity's gaps were logged (once a room).
    gaps_logged: bool,
    /// The server projectiles whose graphs didn't start (logged once).
    failed: Vec<ShotKey>,
    /// The shots whose fields removed themselves since the room last
    /// looked.
    ended: Vec<ShotKey>,
    /// The server projectiles that removed themselves since the room last
    /// looked.
    ended_flights: Vec<ShotKey>,
}

impl SpawnedEntities {
    pub fn new(
        graphs: Arc<GraphStore>,
        field_graphs: Vec<u32>,
        spawn_graphs: Arc<SpawnGraphs>,
        mode: u64,
        ruleset: u64,
        mode_random: f32,
        map_rays: Option<Arc<dyn MapRays>>,
    ) -> Self {
        let world = MapEntityWorld::start(
            Arc::clone(&graphs),
            mode,
            ruleset,
            mode_random,
            Unknowns::for_build(),
            [],
            None,
            map_rays,
            None,
            0,
        );
        Self {
            graphs,
            codecs: CodecTable::client(),
            field_graphs,
            spawn_graphs,
            world,
            spawned: Vec::new(),
            gaps_logged: false,
            failed: Vec::new(),
            ended: Vec::new(),
            ended_flights: Vec::new(),
        }
    }

    /// The entities a body's controller owns, for the target candidates
    /// ([`TargetCandidate::spawned`]): its fields, and the projectiles the
    /// body shot.
    pub fn owned_by(&self, body: u32) -> Vec<OwnedEntity> {
        self.spawned
            .iter()
            .filter(|spawned| spawned.shot.0.entity == body)
            .map(|spawned| OwnedEntity {
                entity: spawned.entity,
                projectile: spawned.projectile,
            })
            .collect()
    }

    /// One update at `tick`: the server projectiles move to where their
    /// `flights` are (those no longer in flight go), the running graphs
    /// advance with the bodies as `candidates` lists them, then the fields
    /// that opened start (`opened`: the shot, the field's entity and where
    /// it stands), and so do the new flights whose entity starts graphs,
    /// each with an entity of `next_id`; every field's frame is projected.
    pub(crate) fn advance(
        &mut self,
        tick: u32,
        candidates: Vec<TargetCandidate>,
        opened: impl IntoIterator<Item = (ShotKey, u32, [f64; 3])>,
        flights: &[ServerFlight],
        mut next_id: impl FnMut() -> u32,
    ) {
        // An entity that removed itself since (its message entry, between
        // the room's updates) is gone from the world already; it ends with
        // the destroyed below.
        let world = &mut self.world;
        self.spawned.retain(|spawned| {
            if !spawned.projectile {
                return true;
            }
            match flights.iter().find(|flight| flight.shot == spawned.shot) {
                Some(flight) => {
                    world.set_position(spawned.entity, flight.position);
                    true
                }
                None => {
                    world.remove(spawned.entity);
                    false
                }
            }
        });
        self.failed
            .retain(|shot| flights.iter().any(|flight| flight.shot == *shot));
        self.world.set_players(candidates);
        self.world.advance(tick);
        let field_graphs = self.field_graphs.clone();
        for (shot, entity, position) in opened {
            if !self.start(shot, entity, &field_graphs, position, None) {
                eprintln!("the Biotic Field's graphs {field_graphs:04X?} don't start");
                self.ended.push(shot);
            }
        }
        for flight in flights {
            let known = self
                .spawned
                .iter()
                .any(|spawned| spawned.shot == flight.shot);
            if known || self.failed.contains(&flight.shot) {
                continue;
            }
            let Some(graphs) = self.spawn_graphs.get(&flight.definition).cloned() else {
                continue;
            };
            let shooter = ProjectileShooter {
                body: flight.shot.0.entity,
                hero: flight.hero,
            };
            let entity = next_id();
            if !self.start(flight.shot, entity, &graphs, flight.position, Some(shooter)) {
                eprintln!(
                    "the projectile entity {:016X}'s graphs {graphs:04X?} don't start",
                    flight.definition
                );
                self.failed.push(flight.shot);
            }
        }
        let destroyed = self.world.take_destroyed();
        let (ended, ended_flights) = (&mut self.ended, &mut self.ended_flights);
        self.spawned.retain(|spawned| {
            let gone = destroyed.contains(&spawned.entity);
            if gone {
                if spawned.projectile {
                    ended_flights.push(spawned.shot);
                } else {
                    ended.push(spawned.shot);
                }
            }
            !gone
        });
        self.project();
    }

    /// Spawn `entity` of `shot` on `graphs` at `position`, owned by the
    /// shooter's controller; a projectile with its `shooter`.
    /// False: its graphs don't start.
    fn start(
        &mut self,
        shot: ShotKey,
        entity: u32,
        graphs: &[u32],
        position: [f64; 3],
        shooter: Option<ProjectileShooter>,
    ) -> bool {
        let started = self.world.spawn(Spawn {
            entity,
            graphs,
            position,
            owner: shot.0.entity & !0xff,
            shooter,
        });
        if started {
            self.spawned.push(Spawned {
                shot,
                entity,
                projectile: shooter.is_some(),
                frame: None,
            });
        }
        started
    }

    /// The fields of the shots that no longer run (their owner's body
    /// went) end: their graphs end and take their auras off.
    pub(crate) fn end_unless(&mut self, live: &[ShotKey]) {
        let world = &mut self.world;
        self.spawned.retain(|spawned| {
            if spawned.projectile || live.contains(&spawned.shot) {
                return true;
            }
            world.remove(spawned.entity);
            false
        });
    }

    /// The shots whose fields removed themselves since the last call.
    pub(crate) fn take_ended(&mut self) -> Vec<ShotKey> {
        std::mem::take(&mut self.ended)
    }

    /// The server projectiles that removed themselves since the last call
    /// (a Biotic Orb whose pool is spent): their flights end.
    pub(crate) fn take_ended_flights(&mut self) -> Vec<ShotKey> {
        std::mem::take(&mut self.ended_flights)
    }

    /// The auras the entities put on bodies or took off.
    pub fn take_auras(&mut self) -> Vec<AuraChange> {
        self.world.take_auras()
    }

    /// The entities' game messages and message ends since the last call
    /// (`0258` st8's `0EEB.025` to its owner's player entity, a Biotic
    /// Orb's `060E.025` to its shooter), for the match to route
    /// ([`crate::messages`]).
    pub fn take_messages(&mut self) -> (Vec<GameMessage>, Vec<MessageEnd>) {
        (self.world.take_messages(), self.world.take_message_ends())
    }

    /// The stats the entities' graphs counted since the last call (`0258`
    /// rid14 counts "Biotic Fields Deployed" `035C` for its owner's player
    /// entity), for the match's stat tables.
    pub fn take_tracked_stats(&mut self) -> Vec<TrackedStat> {
        self.world.take_tracked_stats()
    }

    /// Whether a spawned entity's running graphs are entity `entity`'s.
    pub fn runs(&self, entity: u32) -> bool {
        self.world
            .entity(entity)
            .is_some_and(|placed| placed.running())
    }

    /// The spawned entities whose graphs run now.
    pub fn entities(&self) -> Vec<u32> {
        self.world
            .entities()
            .iter()
            .filter(|placed| placed.running())
            .map(|placed| placed.entity)
            .collect()
    }

    /// A game message to the spawned entity it names, or to every one but
    /// its sender ([`EVERY_ENTITY`]): their entries run now.
    pub fn receive_message(&mut self, message: &GameMessage) {
        if message.target == EVERY_ENTITY {
            self.world
                .receive_everywhere(message.sender, message.message, &message.parameters);
        } else {
            self.world.receive_message(
                message.target,
                message.sender,
                message.message,
                &message.parameters,
            );
        }
    }

    /// A persistent sender let its message to the entity `end.target` go.
    pub fn end_message(&mut self, end: &MessageEnd) {
        self.world.end_message(end.target, end.sender, end.message);
    }

    /// A field's latest frame; none while its graph doesn't run.
    pub(crate) fn frame(&self, shot: ShotKey) -> Option<&PlacedFrame> {
        let field = self
            .spawned
            .iter()
            .find(|spawned| spawned.shot == shot && !spawned.projectile)?;
        field.frame.as_ref().map(|(_, frame)| frame)
    }

    /// The latest frame of each server projectile whose graphs run, for
    /// its copies (`ObservedProjectile::set_frame`).
    pub(crate) fn flight_frames(&self) -> impl Iterator<Item = (ShotKey, &PlacedFrame)> {
        self.spawned
            .iter()
            .filter(|spawned| spawned.projectile)
            .filter_map(|spawned| Some((spawned.shot, &spawned.frame.as_ref()?.1)))
    }

    /// The speed scale each server projectile's graphs give it now
    /// (`STU_D3F71FCF`: a Biotic Orb flies at 0.275 while it has targets),
    /// for its flight.
    pub(crate) fn flight_speeds(&self) -> impl Iterator<Item = (ShotKey, f64)> + '_ {
        self.spawned
            .iter()
            .filter(|spawned| spawned.projectile)
            .filter_map(|spawned| {
                let placed = self.world.entity(spawned.entity)?;
                Some((spawned.shot, placed.speed_scale()))
            })
    }

    /// Project each running entity's observer frame; a new one when it
    /// changed. Unlike a map placeable's, a spawned entity with
    /// assumptions still sends its frame (the clients show nothing of a
    /// field without one): they are logged once a room. Today's: the
    /// TrackTargets' line of sight (`m_3D667CE3`) and ApplyAura's lifetime.
    fn project(&mut self) {
        let encoder =
            SnapshotEncoder::new(&self.graphs, &self.codecs).with_unknowns(self.world.unknowns());
        for field in &mut self.spawned {
            let Some(placed) = self.world.entity(field.entity) else {
                continue;
            };
            if !placed.complete() && !std::mem::replace(&mut self.gaps_logged, true) {
                eprintln!(
                    "the spawned entity {:08X} runs with gaps: {:?}",
                    field.entity,
                    gaps(placed)
                );
            }
            let snapshot = match placed.snapshot() {
                Some(snapshot) if snapshot.initialization_complete() => snapshot,
                _ => {
                    field.frame = None;
                    continue;
                }
            };
            let projected = match encoder.project(snapshot, View::Observer) {
                Ok(projected) => projected,
                Err(error) => {
                    eprintln!(
                        "the spawned entity {:08X}'s frame does not project ({error:?})",
                        field.entity
                    );
                    field.frame = None;
                    continue;
                }
            };
            if field
                .frame
                .as_ref()
                .is_some_and(|(known, _)| *known == projected)
            {
                continue;
            }
            match encoder.full(&projected) {
                Ok(bits) => {
                    let serial = field
                        .frame
                        .as_ref()
                        .map_or(1, |(_, frame)| frame.serial + 1);
                    let frame = PlacedFrame {
                        serial,
                        bits: Arc::new(bits),
                    };
                    field.frame = Some((projected, frame));
                }
                Err(error) => {
                    eprintln!(
                        "the spawned entity {:08X}'s frame does not encode ({error:?})",
                        field.entity
                    );
                    field.frame = None;
                }
            }
        }
    }
}

/// What kept a spawned entity's graph from completing.
fn gaps(placed: &lucy_script::placed::PlacedEntity) -> BTreeMap<String, usize> {
    let mut gaps = BTreeMap::new();
    if let Some(fault) = placed.fault() {
        *gaps.entry(fault.to_owned()).or_default() += 1;
    }
    if let Some(snapshot) = placed.snapshot() {
        for assumption in &snapshot.assumptions {
            *gaps.entry(assumption.class.to_string()).or_default() += 1;
        }
        for diagnostic in &snapshot.diagnostics {
            *gaps.entry(format!("{:?}", diagnostic.issue)).or_default() += 1;
        }
    }
    gaps
}

#[cfg(test)]
mod tests {
    use lucy_script::{vm::AuraSource, wire::Value};

    use super::*;
    use crate::{
        projectiles::{FIELD_FIRST, ProjectileToken},
        replication::BodyId,
    };

    fn body(entity: u32, team: u8, feet: [f64; 3]) -> TargetCandidate {
        TargetCandidate {
            entity,
            team: Some(team),
            dead: false,
            controller: Some(entity & !0xff),
            player: Some(entity & !0xff),
            ai: false,
            ai_control: None,
            feet,
            height: 1.8,
            radius: 0.4,
            yaw: 0.0,
            tags: None,
            health: None,
            hardpoints: None,
            spawned: Vec::new(),
            movement_parent: None,
        }
    }

    /// Soldier: 76's Biotic Field on its graph `0258`: at its opening it
    /// puts `025F` on himself and his ally within 5 m of the point 1 m
    /// above it (TrackTargets `#422`, `m_05BE62DD`), not on the enemy 2 m
    /// away nor the ally 8 m away, with `0258`'s heal a second (`#420`, 35
    /// for a human owner) and seconds (`#567`, 5), dealt by the field. Its
    /// observer frame goes from the start: st2 (the ring, counter 1), st3
    /// (the canister), st5 (fading) and st8 (`0EEB.025` to its owner's
    /// player entity, `STU_0340A42A`), with the sync variables `#420`,
    /// `#567`, `#422`, `#375` (the targets) and `#2712`. A frame `F` runs
    /// the events due in its 16 ms (`Component::advance_frame`), so st1's
    /// Wait (`#567 - 0.5` = 4.5 s) ends st5 in frame 281 (4496-4511 ms), a
    /// new frame without st5; st4's 0.5 s later (5 s) the field removes
    /// itself in frame 312 (4992-5007 ms), which takes both auras off and
    /// ends its shot. At its opening rid14 counts "Biotic Fields Deployed"
    /// (`035C`) once for `STU_0340A42A(owner())`: Soldier: 76's player
    /// entity, whose stats the match credits.
    #[test]
    #[ignore = "requires the derived 104319 data"]
    fn the_field_runs_its_graph_from_aura_to_removal() {
        const SOLDIER: u32 = 0x1000_0010;
        const ALLY: u32 = 0x1000_0110;
        const OPEN: u32 = 100;
        let data = lucy_testkit::runtime_dir().join("data");
        let graphs = Arc::new(GraphStore::load_runtime(&data, "game_graphs_174.json.gz").unwrap());
        let mut fields = SpawnedEntities::new(
            Arc::clone(&graphs),
            vec![0x0258],
            Arc::default(),
            0,
            0,
            0.0,
            None,
        );
        let shot = (
            BodyId {
                entity: SOLDIER,
                generation: 1,
            },
            ProjectileToken::server(1),
        );
        let candidates = |fields: &SpawnedEntities| {
            let mut soldier = body(SOLDIER, 1, [0.0; 3]);
            soldier.spawned = fields.owned_by(SOLDIER);
            vec![
                soldier,
                body(ALLY, 1, [3.0, 0.0, 0.0]),
                body(0x1000_0210, 2, [0.0, 0.0, 2.0]),
                body(0x1000_0310, 1, [8.0, 0.0, 0.0]),
            ]
        };
        fields.advance(
            OPEN,
            candidates(&fields),
            [(shot, FIELD_FIRST, [0.0; 3])],
            &[],
            || unreachable!("no flight"),
        );
        let source = AuraSource {
            entity: FIELD_FIRST,
            instance: 1,
            state: 7,
            activation: 1,
        };
        let apply = |target| AuraChange::Apply {
            source: Some(source),
            target,
            aura: 0x04a0_0000_0000_025f,
            instigator: FIELD_FIRST,
            creator: FIELD_FIRST,
            parameters: vec![
                (0x0a40_0000_0000_002a, Value::Float(35.0)),
                (0x0a40_0000_0000_002b, Value::Integer(5)),
            ],
        };
        assert_eq!(fields.take_auras(), [apply(SOLDIER), apply(ALLY)]);
        assert_eq!(
            fields.take_tracked_stats(),
            [TrackedStat {
                stats: vec![0x0860_0000_0000_035c],
                amount: 1.0,
                targets: vec![SOLDIER & !0xff],
            }]
        );
        let projected = |fields: &SpawnedEntities| fields.spawned[0].frame.clone().unwrap().0;
        let states = |fields: &SpawnedEntities| -> Vec<u16> {
            let projected = projected(fields);
            assert_eq!(projected.instance(1), Some((0x0258, None)));
            (0..64)
                .filter(|&state| projected.state(1, state).is_some())
                .collect()
        };
        assert_eq!(fields.frame(shot).map(|frame| frame.serial), Some(1));
        assert_eq!(states(&fields), [2, 3, 5, 8]);
        assert_eq!(
            projected(&fields).instance_variable(1, 375),
            Some(&Value::Array(vec![
                Value::Entity(SOLDIER),
                Value::Entity(ALLY)
            ]))
        );
        let mut changed = Vec::new();
        let mut ended = None;
        for tick in OPEN + 1..OPEN + 400 {
            fields.advance(tick, candidates(&fields), [], &[], || {
                unreachable!("no flight")
            });
            let serial = fields.frame(shot).map(|frame| frame.serial);
            if serial.is_some_and(|serial| serial > 1 + changed.len() as u64) {
                assert_eq!(states(&fields), [2, 3, 8], "{tick}");
                changed.push(tick - OPEN);
            }
            let auras = fields.take_auras();
            if !fields.take_ended().is_empty() {
                let remove = |target| AuraChange::Remove { source, target };
                assert_eq!(auras, [remove(SOLDIER), remove(ALLY)]);
                ended = Some(tick - OPEN);
                break;
            }
            assert!(auras.is_empty(), "{tick}: {auras:?}");
        }
        assert_eq!(changed, [281]);
        assert_eq!(ended, Some(312));
        assert!(fields.owned_by(SOLDIER).is_empty());
    }

    /// One of Moira's Biotic Orbs, for [`orb_spends_its_pool`].
    struct Orb {
        definition: u64,
        graph: u32,
        /// The other body's team: Moira's (1) for the healing orb.
        team: u8,
        aura: u64,
        rate: Value,
        heal: bool,
        /// The 50s its pool takes.
        pool: usize,
        /// Its speed scale state `STU_D3F71FCF`.
        slow: u16,
    }

    /// A Biotic Orb on its graph as its flight carries it 1 m above Moira:
    /// spawned where the flight is, it puts its aura (`002A` its rate a
    /// second) on the other body 2 m away at once (the healing orb only on
    /// a hurt ally), the orb itself the applier. With a target, its
    /// BooleanSwitch on `#375` begins `STU_D3F71FCF(0.275)`: its frame
    /// (which the clients' copies of the orb take, as the client begins
    /// this remote state only from the server's frames) holds it, and its
    /// flight slows to 0.275. It counts what its aura dealt (`0012.025`
    /// damage dealt, `0044.025` healing done) against its pool of 200 or
    /// 300 × `#12619`/`#12618` (Moira's ruleset defaults, 1.0, read through
    /// its shooter `STU_2B2E2F6D`), and once it is spent it removes itself,
    /// which ends its flight, and tells its shooter (`060E.025`).
    fn orb_spends_its_pool(orb: Orb) {
        use lucy_content::maps::{PRACTICE_RANGE_MODE, PRACTICE_RANGE_RULESET};
        const MOIRA: u32 = 0x1000_0010;
        const MOIRA_HERO: u64 = 0x02e0_0000_0000_01a2;
        const OTHER: u32 = 0x1000_0210;
        const ORB: u32 = 0xa00f_0000;
        const ORB_DONE: u64 = 0x0240_0000_0000_060e;
        let data = lucy_testkit::runtime_dir().join("data");
        let graphs = Arc::new(GraphStore::load_runtime(&data, "game_graphs_174.json.gz").unwrap());
        let spawn_graphs = Arc::new(SpawnGraphs::from([(orb.definition, vec![orb.graph])]));
        let mut spawned = SpawnedEntities::new(
            Arc::clone(&graphs),
            Vec::new(),
            spawn_graphs,
            PRACTICE_RANGE_MODE,
            PRACTICE_RANGE_RULESET,
            0.0,
            None,
        );
        let shot = (
            BodyId {
                entity: MOIRA,
                generation: 1,
            },
            ProjectileToken::server(1),
        );
        let candidates = |spawned: &SpawnedEntities| {
            let mut moira = body(MOIRA, 1, [0.0; 3]);
            moira.spawned = spawned.owned_by(MOIRA);
            moira.health = Some([200.0, 200.0]);
            let mut other = body(OTHER, orb.team, [2.0, 0.0, 0.0]);
            other.health = Some([100.0, 200.0]);
            vec![moira, other]
        };
        let flight = [ServerFlight {
            shot,
            hero: MOIRA_HERO,
            definition: orb.definition,
            position: [0.0, 1.0, 0.0],
        }];
        spawned.advance(100, candidates(&spawned), [], &flight, || ORB);
        assert_eq!(
            spawned.owned_by(MOIRA),
            [OwnedEntity {
                entity: ORB,
                projectile: true
            }]
        );
        let auras = spawned.take_auras();
        let [
            AuraChange::Apply {
                target,
                aura,
                instigator,
                parameters,
                ..
            },
        ] = auras.as_slice()
        else {
            panic!("one aura: {auras:?}");
        };
        assert_eq!((*target, *aura, *instigator), (OTHER, orb.aura, ORB));
        assert_eq!(parameters, &[(0x0a40_0000_0000_002a, orb.rate)]);
        let frame = &spawned.spawned[0].frame.as_ref().unwrap().0;
        assert!(frame.state(1, orb.slow).is_some());
        assert_eq!(
            spawned.flight_speeds().collect::<Vec<_>>(),
            [(shot, f64::from(0.275_f32))]
        );
        let last = 100 + orb.pool as u32;
        for tick in 101..=last {
            let dealt = if orb.heal {
                lucy_script::ultimate::Dealt::Healing
            } else {
                lucy_script::ultimate::Dealt::Damage { critical: false }
            };
            let (message, parameters) = lucy_script::ultimate::dealt_message(
                OTHER,
                50.0,
                &lucy_script::weapons::DamageTags::NONE,
                dealt,
                &[],
            );
            spawned.receive_message(&GameMessage {
                sender: ORB,
                target: ORB,
                message,
                parameters,
            });
            spawned.advance(tick, candidates(&spawned), [], &flight, || ORB);
            let (messages, _) = spawned.take_messages();
            let ended = spawned.take_ended_flights();
            if tick < last {
                assert!(messages.is_empty() && ended.is_empty(), "{tick}");
                continue;
            }
            assert_eq!(ended, [shot]);
            let [done] = messages.as_slice() else {
                panic!("{messages:?}");
            };
            assert_eq!((done.message, done.target), (ORB_DONE, MOIRA));
        }
        assert!(spawned.owned_by(MOIRA).is_empty());
    }

    /// The damaging orb (`1E61.003`, graph `1100`): `02D5` (50 a second)
    /// on the enemy within 4 m, a pool of 200 (four 50s), `STU_D3F71FCF`
    /// st3.
    #[test]
    #[ignore = "requires the derived 104319 data"]
    fn the_damaging_orb_hurts_the_enemy_near_it_until_its_pool_is_spent() {
        orb_spends_its_pool(Orb {
            definition: 0x0400_0000_0000_1e61,
            graph: 0x1100,
            team: 2,
            aura: 0x04a0_0000_0000_02d5,
            rate: Value::Integer(50),
            heal: false,
            pool: 4,
            slow: 3,
        });
    }

    /// The healing orb (`1E60.003`, graph `10FF`): `02A1` (65 a second) on
    /// the hurt ally within 5 m, a pool of 300 (six 50s), `STU_D3F71FCF`
    /// st4.
    #[test]
    #[ignore = "requires the derived 104319 data"]
    fn the_healing_orb_heals_the_hurt_ally_near_it_until_its_pool_is_spent() {
        orb_spends_its_pool(Orb {
            definition: 0x0400_0000_0000_1e60,
            graph: 0x10ff,
            team: 1,
            aura: 0x04a0_0000_0000_02a1,
            rate: Value::Integer(65),
            heal: true,
            pool: 6,
            slow: 4,
        });
    }
}
