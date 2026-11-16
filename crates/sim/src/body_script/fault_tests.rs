//! A failed weapon simulation cannot keep dealing damage through the live
//! command route: the port of `tests/test_game_script_faults.py`.

use std::{sync::Mutex, time::Duration};

use super::*;
use crate::{
    egress::{DeliveryReport, PacketDelivery, RecordSource},
    instance::RETAIL_QUANTA,
    replication::WorldDelivery,
    scene::{PlayerAppearance, PlayerCard},
    scene_assets::{PreparedScene, SceneAssets},
};
use orpheus_content::{
    maps::GameMap,
    parameters::{CLIENT_GAME_MESSAGE, PICK_HERO},
};
use orpheus_core::tick::FIRST_TICK;
use orpheus_wire::{
    commands::FIRE,
    jam::codec::{Fields, Value as Field},
    messages::Message,
};
use serde_json::json;

/// `EVENTS_PER_TIME` in `ow174/game/script/runtime.py`: the events one
/// instance may take at one time.
const EVENTS_PER_TIME: usize = 1000;
/// A message the rifle graph takes and does nothing with.
const HARMLESS: u64 = 0x0240_0000_0000_0099;

const SHOOTER: u32 = 0xa000_0100;
const OTHER: u32 = 0xa000_0200;
const CONTROL: u32 = 0xa000_0300;

/// What the test sees of one body's script.
#[derive(Default)]
struct Probe {
    /// Before the next frame, post more harmless events than the VM takes
    /// at one time.
    overload: bool,
    /// The last frame the script advanced to.
    frame: u32,
    /// Whether the rifle's volley state is active.
    volley_active: bool,
}

/// Every body script started, by body, with its probe.
type Probes = Arc<Mutex<Vec<(u32, Arc<Mutex<Probe>>)>>>;

/// A body script the test can overload and watch.
struct ProbedBody {
    body: PlayerBodyRuntime,
    probe: Arc<Mutex<Probe>>,
}

impl OwnerBodyRuntime for ProbedBody {
    fn snapshot(&self) -> Option<&Snapshot> {
        self.body.snapshot()
    }

    fn encoder(&self) -> SnapshotEncoder<'_> {
        self.body.encoder()
    }

    fn advance(&mut self, command: Command, mover: &Mover, step: BodyStep) -> io::Result<()> {
        let mut probe = self.probe.lock().unwrap();
        if std::mem::take(&mut probe.overload) {
            // Harmless, valid events overload the rifle's instance before
            // the frame's own input.
            for _ in 0..EVENTS_PER_TIME + 2 {
                let entries = self
                    .body
                    .component
                    .queue_message(1, HARMLESS, Variables::new())
                    .unwrap();
                assert_eq!(entries, 1);
            }
        }
        self.body.advance(command, mover, step)?;
        probe.frame = command.frame;
        probe.volley_active = self
            .body
            .snapshot()
            .and_then(|snapshot| snapshot.instances[&1].states.get(&1))
            .is_some_and(|volley| volley.active);
        Ok(())
    }

    fn frame_ms(&self) -> u32 {
        self.body.frame_ms()
    }

    fn vitals(&self) -> Option<&Vitals> {
        self.body.vitals()
    }

    fn take_health_update(&mut self) -> Option<EntityComponent> {
        self.body.take_health_update()
    }

    fn projectile_leases(&self) -> Option<&ProjectileLeases> {
        self.body.projectile_leases()
    }

    fn movement_modifiers(&self) -> Option<&BTreeMap<ModifierKey, Modifier>> {
        self.body.movement_modifiers()
    }

    fn take_shots(&mut self, command: Option<Command>, mover: &Mover) -> Vec<PosedShot> {
        self.body.take_shots(command, mover)
    }

    fn observer_activity(&self) -> Option<ObserverActivity> {
        self.body.observer_activity()
    }

    fn vitals_mut(&mut self) -> Option<&mut Vitals> {
        self.body.vitals_mut()
    }

    fn health_changed(&mut self) -> io::Result<()> {
        self.body.health_changed()
    }

    fn receive_message(
        &mut self,
        sender: u32,
        message: u64,
        parameters: Variables,
    ) -> io::Result<usize> {
        self.body.receive_message(sender, message, parameters)
    }

    fn take_messages(&mut self) -> Vec<GameMessage> {
        self.body.take_messages()
    }

    fn move_requests(&self) -> Vec<MoveRequest> {
        self.body.move_requests()
    }

    fn finish_move(&mut self, request: MoveRequest, arrived: bool, frame: u32) -> io::Result<()> {
        self.body.finish_move(request, arrived, frame)
    }

    fn take_motions(&mut self) -> Vec<Motion> {
        self.body.take_motions()
    }
}

/// A hitscan rifle on fire (logical button 20) whose volley reports a shot
/// every frame, and an entry for the harmless message.
fn rifle() -> PlayerBodyFactory {
    let plug = |links: Json| json!({"$":"Plug","links":links,"m_FF3DAF1E":[]});
    let graph = json!({
        "index":42,"items":4,"states":[1,2],"entries":[0,3],"remote":[],
        "sync_vars":[],"containers":{},"unknown_nodes":[],"fields":{"m_680A2CB2":[0]},
        "header":{"m_nodesBitCount":4,"m_statesBitCount":4,
            "m_remoteSyncNodesBitCount":4,"m_syncVarsBitCount":4},
        "nodes":[
            {"pos":0,"class":"STUStatescriptEntry","uid":0,"rid":0,"remote":-1,
                "state":null,"client_only":false,"server_only":false,"fields":{
                "m_outPlug":plug(json!([[1,"m_beginPlug"]]))}},
            {"pos":1,"class":"STUStatescriptStateLogicalButton","uid":1,"rid":1,"remote":-1,
                "state":0,"client_only":false,"server_only":true,"fields":{
                "m_2BBEEAB8":true,"m_logicalButton":20,
                "m_onGoingDownPlug":plug(json!([[2,"m_beginPlug"]]))}},
            {"pos":2,"class":"STUStatescriptStateWeaponVolley","uid":2,"rid":2,"remote":-1,
                "state":1,"client_only":false,"server_only":true,"fields":{
                "m_2BBEEAB8":true,"m_A17BE89B":30,"m_numShotsPerSecond":62.5,
                "m_FEC435C6":180,
                "m_6396149F":{"m_modifyHealth":{"m_amount":20.0},"m_A872C70E":2}}},
            {"pos":3,"class":"STUStatescriptEntryGameMessage","uid":3,"rid":3,"remote":-1,
                "state":null,"client_only":false,"server_only":false,"fields":{
                "m_gameMessage":{"m_gameMessage":format!("0x{HARMLESS:016X}")},
                "m_outPlug":plug(json!([]))}}
        ]
    });
    fixture_graph_factory(graph, Variables::new())
}

/// Send everything the client may get now and let all of it arrive.
fn deliver(world: &mut WorldDelivery) {
    let ready = world.ready();
    let records: Vec<_> = ready.iter().map(|item| item.tracked()).collect();
    let sources: Vec<_> = (0..ready.len()).map(RecordSource::Queued).collect();
    let indexes = WorldDelivery::sent_indices(&ready, &sources);
    drop(ready);
    world.sent(&indexes);
    world.acknowledge(
        DeliveryReport {
            requeued_reliable: 0,
            packets: vec![PacketDelivery {
                sequence: 0,
                arrived: true,
                records,
            }],
        },
        Duration::ZERO,
    );
}

struct Player {
    world: WorldDelivery,
    owner: u32,
    slot: i8,
    team: u8,
    card: PlayerCard,
}

/// The player's appearance in the match: the first spawn of his team.
fn appearance(owner: u32, slot: i8, team: u8, card: &PlayerCard) -> PlayerAppearance<'_> {
    PlayerAppearance {
        entity: owner,
        slot,
        team,
        spawn: 0,
        room: orpheus_content::maps::SpawnChoice::First,
        card,
    }
}

impl Player {
    /// The player joins and picks Soldier: 76 through his client's hero
    /// select message.
    fn join(scene: &PreparedScene, owner: u32, slot: i8, team: u8, tick: u32) -> Self {
        let mut player = Self {
            world: WorldDelivery::new(owner).unbuffered(),
            owner,
            slot,
            team,
            card: PlayerCard::default(),
        };
        let viewer = appearance(owner, slot, team, &player.card);
        let world = &mut player.world;
        world.start(scene, &viewer, tick).unwrap();
        deliver(world);
        world.present(scene, tick).unwrap();
        deliver(world);
        let pick = Message {
            id: CLIENT_GAME_MESSAGE,
            fields: Fields::from([
                (
                    0x78,
                    Field::Struct(Fields::from([(0, Field::Unsigned(u64::from(owner)))])),
                ),
                (0x80, Field::Blob(SOLDIER.to_le_bytes().to_vec())),
                (0xa8, Field::Unsigned(PICK_HERO)),
                (0xb0, Field::Signed(3)),
            ]),
        };
        world.queue_game_message(tick, Duration::ZERO, pick);
        world
            .advance_input(scene, &viewer, RETAIL_QUANTA, 16)
            .unwrap();
        assert_eq!(world.current_body(), Some(owner | 1));
        assert!(world.combat_body_active());
        player
    }

    /// The player's command for one frame; the shots it hands to combat.
    fn command(&mut self, scene: &PreparedScene, command: Command) -> Vec<PosedShot> {
        let viewer = appearance(self.owner, self.slot, self.team, &self.card);
        self.world.queue_commands(command.frame, &[command]);
        self.world
            .advance_input(scene, &viewer, RETAIL_QUANTA, 16)
            .unwrap();
        assert!(!self.world.has_input_work());
        self.world.take_shots()
    }
}

/// `tests/test_game_script_faults.py::ScriptFaultIntegrationTests::test_exhausted_weapon_stops_the_body_before_combat_consumes_its_shots`:
/// a rifle instance that takes more events at one time than the VM allows
/// stops the whole body script before its active volley's due shot reaches
/// combat; later fire does nothing. Another player's body goes on.
#[test]
#[ignore = "requires a derived 104319 Practice Range scene"]
fn an_exhausted_weapon_stops_the_body_before_combat_consumes_its_shots() {
    let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    let mut scene = assets.prepare(GameMap::practice_range()).unwrap();
    let probes: Probes = Arc::default();
    let factory = rifle();
    let started = Arc::clone(&probes);
    scene.owner_body_factory = Some(Arc::new(move |_, spawn| {
        let probe = Arc::new(Mutex::new(Probe::default()));
        started
            .lock()
            .unwrap()
            .push((spawn.entity, Arc::clone(&probe)));
        let body = factory.start(spawn)?;
        Ok(Some(Box::new(ProbedBody { body, probe })))
    }));
    let probe = |entity: u32| {
        let probes = probes.lock().unwrap();
        let [(_, probe)] = probes
            .iter()
            .filter(|(body, _)| *body == entity)
            .collect::<Vec<_>>()[..]
        else {
            panic!("one script for body {entity:08X}");
        };
        Arc::clone(probe)
    };

    let tick = FIRST_TICK;
    let mut shooter = Player::join(&scene, SHOOTER, 0, 0, tick);
    let mut other = Player::join(&scene, OTHER, 1, 1, tick);
    // The same rifle, never overloaded: what the shooter's script would do.
    let mut control = Player::join(&scene, CONTROL, 2, 0, tick);
    let fire = |frame| Command {
        frame,
        buttons: FIRE,
        ..Command::default()
    };
    let idle = |frame| Command {
        frame,
        ..Command::default()
    };

    let frame = tick + 1;
    assert_eq!(shooter.command(&scene, fire(frame)).len(), 1);
    assert_eq!(control.command(&scene, fire(frame)).len(), 1);
    assert!(other.command(&scene, idle(frame)).is_empty());
    assert!(probe(SHOOTER | 1).lock().unwrap().volley_active);
    assert!(probe(CONTROL | 1).lock().unwrap().volley_active);

    // The active volley has a shot due in the next frame.
    let frame = frame + 1;
    assert_eq!(control.command(&scene, idle(frame)).len(), 1);
    probe(SHOOTER | 1).lock().unwrap().overload = true;
    assert!(shooter.command(&scene, idle(frame)).is_empty());
    assert!(!shooter.world.combat_body_active());
    assert_eq!(shooter.world.current_body(), Some(SHOOTER | 1));

    // The failure belongs to that body: another player's commands run.
    assert!(other.command(&scene, idle(frame)).is_empty());
    assert!(other.world.combat_body_active());
    assert_eq!(probe(OTHER | 1).lock().unwrap().frame, frame);

    // The stopped body fires nothing.
    assert!(shooter.command(&scene, fire(frame + 1)).is_empty());
    assert!(!shooter.world.combat_body_active());
}
