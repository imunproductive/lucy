//! Owner controller and remote team-entry frames for the 104319 hero select.
//!
//! Research (graphs and client): each team of a game mode names its
//! controller graph. The modes the server runs, the Practice Range (`005A`)
//! included, use 0C90, whose state 119 starts hero select (288A, whose state
//! 0 -> 288B); its HUD graph is client-only. (The solo Range mode `0018`,
//! which Python ran, had 13C1, which started the HUD 20E4 in its state 42
//! and 288A in state 43.) 288B presents the screen 008C.05A ("CHOOSE YOUR
//! HERO") while v2438 (hero select open) and v31435 (what 288B's client-only
//! nodes read) are on. The skin selector shows while 288B's v25377
//! ("assembling heroes") is on. v8535 feeds the screen's
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
//! hero : v940`). Retail OW2 traffic works the same way.
//!
//! The owner's frames are the controller graph's own: the server runs 0C90
//! on the player's controller entity ([`orpheus_script::controller`]),
//! routes the client's picks and opens (21616) to its 288B, and projects its
//! snapshot ([`Controller::controller_frame`]): the instance tree as the VM
//! made it (0C90, 288A, 0215, 288B, 288D in its entries' order), the states
//! that run, and the variables (#2438e, the pick #13161e/#2114e, 0215's
//! pulser and #10122, ...). On top, the frames carry what the graphs get
//! from content the server doesn't run ([`ControllerFrame`]):
//!
//! - #940e, the hero he plays: 0C90 sets it in StateHero (st0) and its
//!   respawn path, which don't run (IDA list, section 17); the server's
//!   possession says it.
//! - #31435e: only 288B's client-only nodes write it; the frames carry it
//!   equal to the open state, as Python's hand-built frames did.
//! - 288B's #25377 (the skin selector) and #8535 (the assemble title hidden):
//!   288B st33-st35 set #25377 while they hold the mode graph's `06AF`
//!   (`0D0B`/`0D42`) and the client's global `02DF`; neither reaches the
//!   server's 288B. The skin selector shows in the PvP modes while heroes
//!   are assembled, in the Practice Range whenever the screen is open; only
//!   the Practice Range hides the assemble title (match.py `send_controller`).
//! - The screen stays shown open while heroes assemble (match.py
//!   `game_message`): 288B closes it on a pick (st95), and what keeps it
//!   open in retail during the countdown is the mode graph's.
//! - 0C90's #4513/#4514/#4515 (rid220, the last hero switch) go only in a
//!   switch notice's frame (`crate::notices`): the graph keeps them, and the
//!   client posts the chat line on every frame that sets the three.

use std::{io, sync::Arc};

use orpheus_content::maps::PRACTICE_RANGE_MODE;
use orpheus_core::bits::BitWriter;
use orpheus_script::{
    codecs::CodecTable,
    controller::{
        ControllerScript, ControllerWorld, HERO_SELECT_SCREEN, HeroSwitch, MULTIKILLS,
        PVP_CONTROLLER,
    },
    graph::{Graph, GraphStore},
    snapshot::{SnapshotEncoder, SnapshotError, View},
    timeline::FRAME_MS,
    vm::{InitFault, Unknowns},
    wire::{Instance, Layout, Value, Variables, WireError, remote_full},
};

const HERO_SELECT_HOST: u32 = 0x288a;
const TEAM_ENTRY: u32 = 0x288d;

const ASSEMBLING: u32 = 25377;
const HIDE_ASSEMBLE: u32 = 8535;
const OPEN_SELECT: u32 = 2438;
const SELECT_SHOWN: u32 = 31435;
const CURRENT_HERO: u32 = 940;
const PICKED: u32 = 2114;
const PICKED_HERO: u32 = 13161;

/// 0C90's hero switch variables (rid220 from 037F.025), which its
/// client-only Watch posts as the chat line (`crate::notices`).
const SWITCH_OLD_HERO: u32 = 4513;
const SWITCH_NEW_HERO: u32 = 4514;
const SWITCH_PLAYER: u32 = 4515;

/// The mode's controller graphs and how the players' controller scripts run
/// and are projected.
#[derive(Clone, Debug)]
pub struct Controller {
    entry: Layout,
    practice: bool,
    mode: u64,
    /// How the controller's script treats content the VM doesn't know
    /// (the players' setting), and its frames a state they can't write.
    unknowns: Unknowns,
    codecs: Arc<CodecTable>,
}

/// What an owner frame carries besides the controller script's snapshot
/// (module notes).
#[derive(Clone, Copy, Debug, Default)]
pub struct ControllerFrame {
    /// Show hero select open whatever 288B's #2438e says (heroes assemble).
    pub keep_open: bool,
    /// 288B's #25377: the skin selector.
    pub skins: bool,
    /// 288B's #8535: the assemble title hidden.
    pub hide_assemble: bool,
    /// A hero switch notice: 0C90's #4513-#4515 in this frame only.
    pub notice: Option<HeroSwitch>,
    /// #940e, the hero the player plays.
    pub hero: Option<u64>,
}

impl Controller {
    pub fn load(graphs: &GraphStore, mode_guid: u64, codecs: Arc<CodecTable>) -> io::Result<Self> {
        for index in [
            PVP_CONTROLLER,
            HERO_SELECT_HOST,
            HERO_SELECT_SCREEN,
            MULTIKILLS,
        ] {
            graph(graphs, index)?;
        }
        Ok(Self {
            entry: layout(graph(graphs, TEAM_ENTRY)?)?,
            practice: mode_guid == PRACTICE_RANGE_MODE,
            mode: mode_guid,
            unknowns: Unknowns::for_build(),
            codecs,
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
    /// `team` who may play `heroes` (each with its role identifier).
    pub fn start_script(
        &self,
        graphs: Arc<GraphStore>,
        entity: u32,
        team: u8,
        heroes: Vec<(u64, Option<u64>)>,
        tick: u32,
    ) -> Result<ControllerScript, InitFault> {
        let mut world = ControllerWorld::new(graphs, self.mode, entity, self.unknowns);
        world.team = Some(i64::from(team));
        world.heroes = heroes;
        ControllerScript::start(world, PVP_CONTROLLER, tick)
    }

    /// The owner's full frame at command frame `command_frame`: the
    /// controller script's snapshot as the owner sees it, with `input` on
    /// top (module notes). A state or variable the projection can't write
    /// fails it (or is left out, tolerating).
    pub fn controller_frame(
        &self,
        script: &ControllerScript,
        command_frame: u32,
        input: ControllerFrame,
    ) -> Result<BitWriter, SnapshotError> {
        let snapshot = script.snapshot().ok_or(SnapshotError::DifferentStream)?;
        let encoder =
            SnapshotEncoder::new(script.graphs(), &self.codecs).with_unknowns(self.unknowns);
        let mut projected = encoder.project(snapshot, View::Owner)?;
        let open = input.keep_open || script.hero_select_state().is_some_and(|state| state.open);
        projected.set_entity_variable(OPEN_SELECT, Some(Value::Bool(open)));
        projected.set_entity_variable(SELECT_SHOWN, Some(Value::Bool(open)));
        projected.set_entity_variable(
            CURRENT_HERO,
            input.hero.filter(|&hero| hero != 0).map(Value::Asset),
        );
        let instances: Vec<_> = projected.instance_graphs().collect();
        for (id, graph) in instances {
            match graph {
                PVP_CONTROLLER => {
                    let notice = input.notice;
                    for (variable, value) in [
                        (
                            SWITCH_OLD_HERO,
                            notice.map(|switch| Value::Asset(switch.old)),
                        ),
                        (
                            SWITCH_NEW_HERO,
                            notice.map(|switch| Value::Asset(switch.new)),
                        ),
                        (
                            SWITCH_PLAYER,
                            notice.map(|switch| Value::Entity(switch.player)),
                        ),
                    ] {
                        projected.set_instance_variable(id, variable, value);
                    }
                }
                HERO_SELECT_SCREEN => {
                    projected.set_instance_variable(
                        id,
                        ASSEMBLING,
                        input.skins.then_some(Value::Bool(true)),
                    );
                    projected.set_instance_variable(
                        id,
                        HIDE_ASSEMBLE,
                        input.hide_assemble.then_some(Value::Bool(true)),
                    );
                }
                _ => {}
            }
        }
        let frame_ms = u32::try_from(FRAME_MS).map_err(|_| SnapshotError::FrameTime)?;
        encoder.owner_full(&projected, command_frame, frame_ms)
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
}
