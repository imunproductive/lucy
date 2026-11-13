//! Owner controller and remote team-entry frames for the 104319 hero select.
//!
//! Research (graphs and client): each team of a game mode names its
//! controller graph. The modes the server runs, the Practice Range (`005A`)
//! included, use 0C90, whose state 119 starts hero select (288A, whose state
//! 0 -> 288B); its HUD graph is client-only. (The solo Range mode `0018`,
//! which Python ran, had 13C1, which started the HUD 20E4 in its state 42
//! and 288A in state 43.) 288B presents the screen 008C.05A ("CHOOSE YOUR
//! HERO") while v2438 (hero select open, the server's) and v31435 (what 288B's
//! client-only nodes read) are on. The skin selector shows while 288B's v25377
//! ("assembling heroes") is on; with it on the client leaves the screen open
//! after a pick, so the server closes it. v8535 feeds the screen's
//! IsAssemblingHeroesHidden ("ASSEMBLE YOUR TEAM" and its timer); only a
//! client-only Stack writes it (on 06A7.025), so a value from the server
//! stays. A chosen skin goes to the lobby (24500). The team list: 288A's state
//! 1 starts 288D, whose client-only nodes post their entity (134F.025) to the
//! local player's 288B unless it is an enemy; every other player's controller
//! entity gets 288D as its only root instance, and the list reads his name
//! from his card (20308). The list shows v940 (the hero he plays, set by
//! 0C90 once spawned), else the pick v13161 while v2114 is on, else
//! "still choosing". The F1 hero info (288A's client-only 2931) shows v940's
//! hero while hero select is closed (2931 st3: `v14152 = v31435 ? browsed
//! hero : v940`). v8419 is m_allowHeroSelect. Retail OW2 traffic works the
//! same way. From a player's first multikill on, his frames also carry the
//! multikill graph `0215` ([`crate::announcer`]) under 0C90 st116/st117.

use std::{collections::BTreeMap, io, sync::Arc};

use crate::announcer::MultikillPulse;
use orpheus_content::maps::PRACTICE_RANGE_MODE;
use orpheus_core::bits::BitWriter;
use orpheus_script::{
    controller::{ControllerScript, ControllerWorld, PVP_CONTROLLER},
    graph::{Graph, GraphStore},
    vm::{InitFault, Unknowns},
    wire::{Instance, Layout, Value, Variables, WireError, owner_full, remote_full, subscript},
};

const PVP: u32 = PVP_CONTROLLER;
/// 0C90's state that runs hero select (288A).
const START_HERO_SELECT: u16 = 119;
/// The multikill graph (`crate::announcer`), 0C90 st116 → st117.
const MULTIKILLS: u32 = 0x0215;
const MULTIKILL_SWITCH: u16 = 116;
const MULTIKILL_HOST: u16 = 117;
const MULTIKILL_PULSER: u16 = 21;
const MULTIKILL_SHARED: u16 = 31;
const MULTIKILL_STIMULUS: u32 = 10122;
/// 0C90's hero select state (owner state 1).
const HERO_SELECT_STATE: u16 = 1;
const HERO_SELECT_HOST: u32 = 0x288a;
const HERO_SELECT: u32 = 0x288b;
const TEAM_ENTRY: u32 = 0x288d;

const ALLOW_SELECT: u32 = 8419;
const ASSEMBLING: u32 = 25377;
const HIDE_ASSEMBLE: u32 = 8535;
const OPEN_SELECT: u32 = 2438;
const SELECT_SHOWN: u32 = 31435;
const CURRENT_HERO: u32 = 940;
const PICKED: u32 = 2114;
const PICKED_HERO: u32 = 13161;

#[derive(Clone, Copy, Debug)]
struct StateSlot {
    index: u16,
    wire: usize,
}

/// The `0215` instance (`crate::announcer`) under 0C90's BooleanSwitch st116
/// and SubScript st117, and its pulser, shared variables and #10122.
#[derive(Clone, Copy, Debug)]
struct MultikillSlots {
    layout: Layout,
    switch: usize,
    host: StateSlot,
    pulser: usize,
    shared_vars: usize,
    stimulus: usize,
}

/// Loaded layouts keep filtered owner positions separate from raw parent state IDs.
#[derive(Clone, Copy, Debug)]
pub struct Controller {
    root: Layout,
    host: Layout,
    screen: Layout,
    entry: Layout,
    multikills: MultikillSlots,
    allow: usize,
    hero_select: usize,
    start_host: StateSlot,
    start_screen: StateSlot,
    start_entry: StateSlot,
    assembling: usize,
    practice: bool,
    mode: u64,
    /// How the controller's script treats content the VM doesn't know
    /// (the players' setting).
    unknowns: Unknowns,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ControllerFrame<'a> {
    pub select_open: bool,
    pub skins: bool,
    pub hide_assemble: bool,
    /// Extra presence bits of the root controller, such as notice switches.
    pub presence: Option<&'a BTreeMap<usize, Value>>,
    pub hero: Option<u64>,
    /// `0215`'s pulser and #10122 once the player's multikills pulsed it;
    /// the controller's frames carry its instance from then on.
    pub multikills: Option<MultikillPulse>,
}

impl Controller {
    pub fn load(graphs: &GraphStore, mode_guid: u64) -> io::Result<Self> {
        let root = graph(graphs, PVP)?;
        let host = graph(graphs, HERO_SELECT_HOST)?;
        let screen = graph(graphs, HERO_SELECT)?;
        let entry = graph(graphs, TEAM_ENTRY)?;
        let multikills = {
            let graph = graph(graphs, MULTIKILLS)?;
            MultikillSlots {
                layout: layout(graph)?,
                switch: owner_slot(root, MULTIKILL_SWITCH)?.wire,
                host: owner_slot(root, MULTIKILL_HOST)?,
                pulser: owner_slot(graph, MULTIKILL_PULSER)?.wire,
                shared_vars: owner_slot(graph, MULTIKILL_SHARED)?.wire,
                stimulus: presence_bit(graph, MULTIKILL_STIMULUS)?,
            }
        };
        Ok(Self {
            root: layout(root)?,
            host: layout(host)?,
            screen: layout(screen)?,
            entry: layout(entry)?,
            multikills,
            allow: presence_bit(root, ALLOW_SELECT)?,
            hero_select: owner_slot(root, HERO_SELECT_STATE)?.wire,
            start_host: owner_slot(root, START_HERO_SELECT)?,
            start_screen: owner_slot(host, 0)?,
            start_entry: owner_slot(host, 1)?,
            assembling: presence_bit(screen, ASSEMBLING)?,
            practice: mode_guid == PRACTICE_RANGE_MODE,
            mode: mode_guid,
            unknowns: Unknowns::for_build(),
        })
    }

    /// The players' setting for content the VM doesn't know.
    pub fn with_unknowns(self, unknowns: Unknowns) -> Self {
        Self { unknowns, ..self }
    }

    /// Whether this is the Practice Range: no match flow, the skin selector
    /// whenever hero select is open, and no assemble title.
    pub fn practice(&self) -> bool {
        self.practice
    }

    /// Start `0C90` on the player's controller entity `entity` at world
    /// tick `tick` ([`orpheus_script::controller`]): the player of team
    /// `team` who may play `heroes`. Its frames are still built here; the
    /// script runs the multikills (`crate::announcer`).
    pub fn start_script(
        &self,
        graphs: Arc<GraphStore>,
        entity: u32,
        team: u8,
        heroes: Vec<u64>,
        tick: u32,
    ) -> Result<ControllerScript, InitFault> {
        let mut world = ControllerWorld::new(graphs, self.mode, entity, self.unknowns);
        world.team = Some(i64::from(team));
        world.heroes = heroes;
        ControllerScript::start(world, PVP, tick)
    }

    /// Owner full frame: root controller, 288A, 288B and team entry.
    pub fn controller_frame(
        &self,
        command_frame: u32,
        input: ControllerFrame<'_>,
    ) -> Result<BitWriter, WireError> {
        let mut root = Instance::new(1, self.root, None);
        root.presence.insert(self.allow, Value::Bool(true));
        if let Some(extra) = input.presence {
            root.presence
                .extend(extra.iter().map(|(&slot, value)| (slot, value.clone())));
        }
        root.states.insert(self.hero_select, BitWriter::new());
        root.states.insert(self.start_host.wire, subscript(2));

        let entry_id = 4;
        let mut host = Instance::new(2, self.host, Some((1, self.start_host.index)));
        host.states.insert(self.start_screen.wire, subscript(3));
        host.states
            .insert(self.start_entry.wire, subscript(entry_id));
        let mut screen = Instance::new(3, self.screen, Some((2, self.start_screen.index)));
        if input.skins {
            screen.presence.insert(self.assembling, Value::Bool(true));
        }
        if input.hide_assemble {
            screen.variables.insert(HIDE_ASSEMBLE, Value::Bool(true));
        }
        let entry = Instance::new(entry_id, self.entry, Some((2, self.start_entry.index)));

        let mut instances = Vec::with_capacity(5);
        // 0215 under 0C90 st117, the next instance (`crate::announcer`).
        let multikills = input
            .multikills
            .map(|pulse| {
                let slots = self.multikills;
                let id = entry_id + 1;
                let mut on = BitWriter::new();
                on.bit(true);
                root.states.insert(slots.switch, on);
                root.states.insert(slots.host.wire, subscript(id));
                let mut instance = Instance::new(id, slots.layout, Some((1, slots.host.index)));
                let mut pulser = BitWriter::new();
                pulser.bit(pulse.fresh);
                pulser.bits(u64::from(pulse.count), 8)?;
                instance.states.insert(slots.pulser, pulser);
                instance.states.insert(slots.shared_vars, BitWriter::new());
                if let Some(stimulus) = pulse.stimulus {
                    instance
                        .presence
                        .insert(slots.stimulus, Value::Asset(stimulus));
                }
                Ok::<_, WireError>(instance)
            })
            .transpose()?;
        instances.extend([root, host, screen, entry]);
        instances.extend(multikills);

        let mut variables = Variables::from([
            (OPEN_SELECT, Value::Bool(input.select_open)),
            (SELECT_SHOWN, Value::Bool(input.select_open)),
        ]);
        if let Some(hero) = input.hero.filter(|&hero| hero != 0) {
            variables.insert(CURRENT_HERO, Value::Asset(hero));
        }
        owner_full(command_frame, &instances, &variables)
    }

    /// A remote 288D root and the three entity variables used by team selection.
    pub fn team_entry_frame(&self, hero: Option<u64>) -> Result<BitWriter, WireError> {
        team_entry(self.entry, hero)
    }
}

fn team_entry(layout: Layout, hero: Option<u64>) -> Result<BitWriter, WireError> {
    let entry = Instance::new(1, layout, None);
    let hero = hero.filter(|&hero| hero != 0);
    let mut variables = Variables::new();
    if let Some(hero) = hero {
        variables.insert(CURRENT_HERO, Value::Asset(hero));
    }
    variables.insert(PICKED, Value::Bool(hero.is_some()));
    if let Some(hero) = hero {
        variables.insert(PICKED_HERO, Value::Asset(hero));
    }
    // Bool and Asset have the same representation in compact and explicit frames.
    remote_full(&[entry], &variables)
}

fn graph(graphs: &GraphStore, index: u32) -> io::Result<&Graph> {
    graphs.graph(index).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("missing controller graph {index:04X}"),
        )
    })
}

fn layout(graph: &Graph) -> io::Result<Layout> {
    Layout::try_from(graph).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn owner_slot(graph: &Graph, index: u16) -> io::Result<StateSlot> {
    let wire = graph.owner_slot(usize::from(index)).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "controller graph {:04X} lacks owner state {index}",
                graph.index
            ),
        )
    })?;
    Ok(StateSlot { index, wire })
}

fn presence_bit(graph: &Graph, id: u32) -> io::Result<usize> {
    graph.presence_bit(id).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "controller graph {:04X} lacks presence variable {id}",
                graph.index
            ),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOLDIER: u64 = 0x02e0_0000_0000_006e;

    fn assert_frame(frame: &BitWriter, bits: usize, hex: &str) {
        assert_eq!(frame.bit_len(), bits);
        let actual: String = frame
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(actual, hex);
    }

    /// An instance an owner frame lists: id, graph and parent (instance,
    /// raw state), as the client reads it.
    type Listed = (u16, u16, Option<(u32, u32)>);

    fn instances(frame: &BitWriter) -> Vec<Listed> {
        let mut bits = orpheus_core::bits::BitReader::new(frame.as_bytes());
        assert!(!bits.bit().unwrap());
        assert!(bits.bit().unwrap(), "an owner frame");
        assert!(!bits.bit().unwrap());
        bits.w_var().unwrap();
        let mut listed = Vec::new();
        let mut id = 0;
        loop {
            let step = bits.w_u16().unwrap();
            if step == 0 {
                return listed;
            }
            id += step;
            let graph = bits.bits(16).unwrap() as u16;
            assert_eq!(bits.bits(2).unwrap(), 0);
            let parent = bits
                .bit()
                .unwrap()
                .then(|| (bits.w_var().unwrap(), bits.w_var().unwrap()));
            assert!(!bits.bit().unwrap());
            listed.push((id, graph, parent));
        }
    }

    #[test]
    fn team_entry_exact_wire_bytes_with_and_without_pick() {
        let layout = Layout {
            graph: TEAM_ENTRY as u16,
            owner_slots: 0,
            remote_slots: 0,
            presence_slots: 0,
        };
        let selected = team_entry(layout, Some(SOLDIER)).unwrap();
        assert_frame(&selected, 167, "88461400631dc8e002dc248400953693c105b80100");
        let choosing = team_entry(layout, None).unwrap();
        assert_frame(&choosing, 69, "884614001342000000");
        assert_eq!(team_entry(layout, Some(0)).unwrap(), choosing);
    }

    #[test]
    #[ignore = "requires the game data derived from a 1.74 install (just test-game <install>)"]
    fn the_controller_frames_match_reference_graphs_and_bytes() {
        let runtime = orpheus_testkit::runtime_dir();
        let graphs =
            GraphStore::load(runtime.join("data").join("game_graphs_174.json.gz")).unwrap();
        let practice = Controller::load(&graphs, PRACTICE_RANGE_MODE).unwrap();
        let pvp = Controller::load(&graphs, 0).unwrap();
        assert!(practice.practice() && !pvp.practice());
        let open = ControllerFrame {
            select_open: true,
            skins: true,
            ..ControllerFrame::default()
        };
        // The Practice Range (005A) has the PvP controller 0C90: the same
        // frames. (Python's Range, mode 0018, had 13C1.)
        assert_eq!(
            practice.controller_frame(123456, open),
            pvp.controller_frame(123456, open)
        );
        let extra = BTreeMap::from([(19, Value::Bool(false)), (0, Value::Bool(true))]);
        for (controller, command_frame, input, bits, expected) in [
            (
                pvp,
                123456,
                open,
                485,
                "1a483c00404032805044a10077c4220a0900201a5148400030c304a8653d080000004200080000000000000000000000001200c01a113804000000c006",
            ),
            (
                pvp,
                123458,
                ControllerFrame {
                    presence: Some(&extra),
                    hero: Some(SOLDIER),
                    ..ControllerFrame::default()
                },
                534,
                "5a483c00404032805044a10077c4220a0900201a5148400030c304a0653d20d6810c2ec00d801000000100040000000000000000000000000900608d080c000000800d",
            ),
        ] {
            let frame = controller.controller_frame(command_frame, input).unwrap();
            assert_frame(&frame, bits, expected);
        }
        // A multikill pulse adds `0215` after the team entry, under 0C90
        // st117.
        let pulse = ControllerFrame {
            hero: Some(SOLDIER),
            multikills: Some(MultikillPulse {
                count: 1,
                fresh: true,
                stimulus: Some(0x0ee0_0000_0000_058a),
            }),
            ..ControllerFrame::default()
        };
        let quiet = ControllerFrame {
            multikills: None,
            ..pulse
        };
        for controller in [practice, pvp] {
            let parent = Some((1, u32::from(MULTIKILL_HOST)));
            let without = instances(&controller.controller_frame(123459, quiet).unwrap());
            let with = instances(&controller.controller_frame(123459, pulse).unwrap());
            let entry = without.last().unwrap();
            assert_eq!(entry.1, TEAM_ENTRY as u16);
            assert_eq!(with[..without.len()], without[..]);
            assert_eq!(
                with[without.len()..],
                [(entry.0 + 1, MULTIKILLS as u16, parent)]
            );
        }
        assert_frame(
            &practice.team_entry_frame(Some(SOLDIER)).unwrap(),
            167,
            "88461400631dc8e002dc248400953693c105b80100",
        );
        assert_eq!(
            pvp.controller_frame(0, open),
            Err(WireError::ZeroCommandFrame)
        );
    }
}
