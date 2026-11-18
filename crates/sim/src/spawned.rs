//! The Biotic Fields the players' canisters leave, each run on its own
//! graphs in the server's spawned-entity world (`orpheus_script::placed`).
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

use std::{collections::BTreeMap, sync::Arc};

use orpheus_script::{
    codecs::CodecTable,
    graph::GraphStore,
    placed::{MapEntityWorld, Spawn},
    snapshot::{ProjectedSnapshot, SnapshotEncoder, View},
    vm::{AuraChange, TargetCandidate, Unknowns},
};

use crate::{placed_script::PlacedFrame, projectiles::ShotKey};

/// One field the room runs.
struct Field {
    shot: ShotKey,
    entity: u32,
    /// Its frame: the projection it was made from and the frame.
    frame: Option<(ProjectedSnapshot, PlacedFrame)>,
}

/// One room's Biotic Fields.
pub struct SpawnedFields {
    graphs: Arc<GraphStore>,
    codecs: CodecTable,
    /// The field's initial graphs (`0258`).
    field_graphs: Vec<u32>,
    world: MapEntityWorld,
    fields: Vec<Field>,
    /// A field's gaps were logged (once a room).
    gaps_logged: bool,
    /// The shots whose fields removed themselves since the room last
    /// looked.
    ended: Vec<ShotKey>,
}

impl SpawnedFields {
    pub fn new(
        graphs: Arc<GraphStore>,
        field_graphs: Vec<u32>,
        mode: u64,
        mode_random: f32,
    ) -> Self {
        let world = MapEntityWorld::start(
            Arc::clone(&graphs),
            mode,
            mode_random,
            Unknowns::for_build(),
            [],
            None,
            0,
        );
        Self {
            graphs,
            codecs: CodecTable::client(),
            field_graphs,
            world,
            fields: Vec::new(),
            ended: Vec::new(),
            gaps_logged: false,
        }
    }

    /// The field entities a body's controller owns, for the target
    /// candidates ([`TargetCandidate::spawned`]).
    pub fn owned_by(&self, body: u32) -> Vec<u32> {
        self.fields
            .iter()
            .filter(|field| field.shot.0.entity == body)
            .map(|field| field.entity)
            .collect()
    }

    /// One update at `tick`: the running fields' graphs advance with the
    /// bodies as `candidates` lists them, then the fields that opened
    /// start (`opened`: the shot, the field's entity and where it stands),
    /// and every field's frame is projected.
    pub(crate) fn advance(
        &mut self,
        tick: u32,
        candidates: Vec<TargetCandidate>,
        opened: impl IntoIterator<Item = (ShotKey, u32, [f64; 3])>,
    ) {
        self.world.set_players(candidates);
        self.world.advance(tick);
        for (shot, entity, position) in opened {
            let spawned = self.world.spawn(Spawn {
                entity,
                graphs: &self.field_graphs,
                position,
                owner: shot.0.entity & !0xff,
            });
            if spawned {
                self.fields.push(Field {
                    shot,
                    entity,
                    frame: None,
                });
            } else {
                eprintln!(
                    "the Biotic Field's graphs {:04X?} don't start",
                    self.field_graphs
                );
                self.ended.push(shot);
            }
        }
        // `0258` st8 sends its owner `0EEB.025`; the match routes no
        // spawned entity's message yet (docs/dev/loose-ends.md).
        self.world.take_messages();
        let destroyed = self.world.take_destroyed();
        let ended = &mut self.ended;
        self.fields.retain(|field| {
            let gone = destroyed.contains(&field.entity);
            if gone {
                ended.push(field.shot);
            }
            !gone
        });
        self.project();
    }

    /// The fields of the shots that no longer run (their owner's body
    /// went) end: their graphs end and take their auras off.
    pub(crate) fn end_unless(&mut self, live: &[ShotKey]) {
        let world = &mut self.world;
        self.fields.retain(|field| {
            if live.contains(&field.shot) {
                return true;
            }
            world.remove(field.entity);
            false
        });
    }

    /// The shots whose fields removed themselves since the last call.
    pub(crate) fn take_ended(&mut self) -> Vec<ShotKey> {
        std::mem::take(&mut self.ended)
    }

    /// The auras the fields put on bodies or took off.
    pub fn take_auras(&mut self) -> Vec<AuraChange> {
        self.world.take_auras()
    }

    /// A field's latest frame; none while its graph doesn't run.
    pub(crate) fn frame(&self, shot: ShotKey) -> Option<&PlacedFrame> {
        let field = self.fields.iter().find(|field| field.shot == shot)?;
        field.frame.as_ref().map(|(_, frame)| frame)
    }

    /// Project each running field's observer frame; a new one when it
    /// changed. Unlike a map placeable's, a field with assumptions still
    /// sends its frame (the clients show nothing of a field without one):
    /// they are logged once a room. Today's: the TrackTargets' line of
    /// sight (`m_3D667CE3`) and ApplyAura's lifetime.
    fn project(&mut self) {
        let encoder =
            SnapshotEncoder::new(&self.graphs, &self.codecs).with_unknowns(self.world.unknowns());
        for field in &mut self.fields {
            let Some(placed) = self.world.entity(field.entity) else {
                continue;
            };
            if !placed.complete() && !std::mem::replace(&mut self.gaps_logged, true) {
                eprintln!(
                    "the Biotic Field {:08X} runs with gaps: {:?}",
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
                        "the Biotic Field {:08X}'s frame does not project ({error:?})",
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
                        "the Biotic Field {:08X}'s frame does not encode ({error:?})",
                        field.entity
                    );
                    field.frame = None;
                }
            }
        }
    }
}

/// What kept a field's graph from completing.
fn gaps(placed: &orpheus_script::placed::PlacedEntity) -> BTreeMap<String, usize> {
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
    use orpheus_script::{vm::AuraSource, wire::Value};

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
            ai: false,
            feet,
            height: 1.8,
            radius: 0.4,
            yaw: 0.0,
            tags: None,
            spawned: Vec::new(),
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
    /// ends its shot.
    #[test]
    #[ignore = "requires the derived 104319 data"]
    fn the_field_runs_its_graph_from_aura_to_removal() {
        const SOLDIER: u32 = 0x1000_0010;
        const ALLY: u32 = 0x1000_0110;
        const OPEN: u32 = 100;
        let data = orpheus_testkit::runtime_dir().join("data");
        let graphs = Arc::new(GraphStore::load_runtime(&data, "game_graphs_174.json.gz").unwrap());
        let mut fields = SpawnedFields::new(Arc::clone(&graphs), vec![0x0258], 0, 0.0);
        let shot = (
            BodyId {
                entity: SOLDIER,
                generation: 1,
            },
            ProjectileToken::server(1),
        );
        let candidates = |fields: &SpawnedFields| {
            let mut soldier = body(SOLDIER, 1, [0.0; 3]);
            soldier.spawned = fields.owned_by(SOLDIER);
            vec![
                soldier,
                body(ALLY, 1, [3.0, 0.0, 0.0]),
                body(0x1000_0210, 2, [0.0, 0.0, 2.0]),
                body(0x1000_0310, 1, [8.0, 0.0, 0.0]),
            ]
        };
        fields.advance(OPEN, candidates(&fields), [(shot, FIELD_FIRST, [0.0; 3])]);
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
            parameters: vec![
                (0x0a40_0000_0000_002a, Value::Float(35.0)),
                (0x0a40_0000_0000_002b, Value::Integer(5)),
            ],
        };
        assert_eq!(fields.take_auras(), [apply(SOLDIER), apply(ALLY)]);
        let projected = |fields: &SpawnedFields| fields.fields[0].frame.clone().unwrap().0;
        let states = |fields: &SpawnedFields| -> Vec<u16> {
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
            fields.advance(tick, candidates(&fields), []);
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
}
