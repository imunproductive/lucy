//! The in-game HUD mount on a server-created UI entity.
//!
//! Ported from AipNooBest's live-tested `hud-display-fix` branch (`860bab45`,
//! MIT; `ow174/game/heroselect.py`, `docs/HERO-WEAPONS.md`):
//!
//! - The HUD widgets draw only for a UI entity registered in the game world,
//!   so the client's own boot UI entity never shows them in a match. The
//!   server creates its own (definition `0x04000000000001F1`).
//! - Its owner frame mounts the retail chain: root `041A` (BooleanSwitch
//!   state 20, launcher state 25) → layer machine `120B`, whose native state
//!   15 (`STU_501733D7`) builds `HUD_Root`; `20E4` rides beside them, and the
//!   ammo widget `20E7` and hero panel `183C` (under `120B`'s launcher 22).
//! - `HUD_Root` builds when `120B` hears the match-start message
//!   `.025:0C47`, which no content graph sends. The server rings it with a
//!   remote full frame that lights the listener's remote-sync entry, together
//!   with the ammo widget's own listener.
//! - The widgets then go and come back once, so their presenters begin again
//!   with the layers there to mount into.
//!
//! Every state is found in the graph data by class and reference; the frames
//! use the client codec table. The ring frame uses this repository's remote
//! full layout (X, A and the class payload per lit entry), not the branch's
//! own H = 0 writer, which omits the A bit and adds instance event lists.

use std::io;

use orpheus_core::bits::BitWriter;
use orpheus_script::{
    codecs::{CodecTable, PayloadMode, StatePayload},
    graph::{Graph, GraphStore, asset_guid},
    wire::{Instance, Layout, Value, Variables, WireError, owner_full, remote_full},
};
use orpheus_wire::entity::{Create, EntityUpdate, Operation};

/// The entity id the branch used live.
pub const WORLD_UI_ENTITY: u32 = 0x63f1;
const WORLD_UI_DEFINITION: u64 = 0x0400_0000_0000_01f1;

const ROOT: u32 = 0x041a;
const MACHINE: u32 = 0x120b;
const HUD: u32 = 0x20e4;
const AMMO: u32 = 0x20e7;
const PANEL: u32 = 0x183c;
const START_MESSAGE: u64 = 0x0240_0000_0000_0c47;

const SUBSCRIPT: &str = "STU_37D754C8";
const SWITCH: &str = "STUStatescriptStateBooleanSwitch";
const PRESENTER: &str = "STUStatescriptStateUXPresenter";
const MESSAGE: &str = "STUStatescriptStateGameMessageEntry";
const LAYER_MACHINE: &str = "STU_501733D7";
const SHARED_VARS: &str = "STU_9D7BF987";

/// Entity variables the mounted screens wait on: `20E4`'s begin variables,
/// the layer gate term `09BA`, the dispatcher's preload `401F`, the mode
/// screen's `09FC`, the hero panel's nine, and the three identities the ammo
/// widget's presenters bind. A presenter whose bound variable is missing
/// asserts in the client.
const MOUNT_VARIABLES: [u32; 17] = [
    17906, 18405, 0x09ba, 0x401f, 0x09fc, 0x0520, 0x4580, 0x2f47, 0x57a8, 0x2c2a, 0x43ce, 0x2613,
    0x099b, 0x1a3c, 0x2ad4, 0x73ed, 0x001f,
];
/// The ammo widget's presence variables and the integers the branch sent:
/// two unnamed, the number `0035`, the magazine `00C6` and the charge
/// `001C`. A float here crashed the client.
const AMMO_PRESENCE: [(u32, i64); 5] = [(18036, 0), (18037, 0), (0x35, 30), (0xc6, 30), (0x1c, 30)];

/// Instance ids on the UI entity.
const ROOT_ID: u16 = 1;
const MACHINE_ID: u16 = 2;
const HUD_ID: u16 = 3;
const AMMO_ID: u16 = 4;
const PANEL_ID: u16 = 5;

#[derive(Clone, Debug)]
pub struct HudMount {
    root: Layout,
    machine: Layout,
    hud: Layout,
    ammo: Layout,
    panel: Layout,
    /// Owner slots with their payloads after the active bit.
    root_states: Vec<(usize, BitWriter)>,
    machine_states: Vec<(usize, BitWriter)>,
    hud_state: usize,
    ammo_states: Vec<(usize, BitWriter)>,
    panel_state: usize,
    /// `041A`'s launcher state, the parent of `120B`.
    root_launcher: u16,
    /// `120B`'s launcher state, the parent of `183C`.
    machine_launcher: u16,
    ammo_presence: Vec<(usize, Value)>,
    ring: BitWriter,
}

impl HudMount {
    pub fn load(graphs: &GraphStore, codecs: &CodecTable) -> io::Result<Self> {
        let root = graph(graphs, ROOT)?;
        let machine = graph(graphs, MACHINE)?;
        let hud = graph(graphs, HUD)?;
        let ammo = graph(graphs, AMMO)?;
        let panel = graph(graphs, PANEL)?;
        let owner = |class: &str, payload: StatePayload| {
            owner_payload(codecs, class, payload).map_err(invalid)
        };

        let switch = |current| StatePayload::from([("current".into(), Value::Bool(current))]);
        let root_gate = owner_slot(root, 20, SWITCH)?;
        let (root_launch, root_launcher) = launcher(root, MACHINE)?;
        let root_states = vec![
            (root_gate, owner(SWITCH, switch(false))?),
            (
                root_launch,
                owner(
                    SUBSCRIPT,
                    StatePayload::from([("child".into(), Value::Integer(i64::from(MACHINE_ID)))]),
                )?,
            ),
        ];
        let machine_states = vec![(
            only_slot(machine, LAYER_MACHINE)?,
            owner(LAYER_MACHINE, StatePayload::new())?,
        )];
        let (_, machine_launcher) = launcher(machine, PANEL)?;
        let hud_state = only_slot(hud, SHARED_VARS)?;
        let mut ammo_states = Vec::new();
        for (wire, class) in owner_classes(ammo) {
            match class {
                PRESENTER => ammo_states.push((wire, owner(PRESENTER, StatePayload::new())?)),
                SWITCH => ammo_states.push((wire, owner(SWITCH, switch(false))?)),
                _ => {}
            }
        }
        if ammo_states.len() != 4 {
            return Err(invalid("the ammo widget 20E7 changed shape"));
        }
        let panel_state = only_slot(panel, SHARED_VARS)?;
        let ammo_presence = AMMO_PRESENCE
            .iter()
            .map(|&(var, value)| {
                ammo.presence_bit(var)
                    .map(|bit| (bit, Value::Integer(value)))
                    .ok_or_else(|| invalid(format!("20E7 lacks presence variable {var:#x}")))
            })
            .collect::<io::Result<_>>()?;

        let mut mount = Self {
            root: layout(root)?,
            machine: layout(machine)?,
            hud: layout(hud)?,
            ammo: layout(ammo)?,
            panel: layout(panel)?,
            root_states,
            machine_states,
            hud_state,
            ammo_states,
            panel_state,
            root_launcher,
            machine_launcher,
            ammo_presence,
            ring: BitWriter::new(),
        };
        let lit = remote_message(codecs).map_err(invalid)?;
        let mut instances = mount.instances(true, false);
        for (id, graph, message) in [
            (MACHINE_ID, machine, Some(START_MESSAGE)),
            (AMMO_ID, ammo, None),
        ] {
            let slot = listener(graph, message)?;
            instances
                .iter_mut()
                .find(|instance| instance.id == id)
                .expect("mounted instance")
                .states
                .insert(slot, lit.clone());
        }
        mount.ring = remote_full(&instances, &variables()).map_err(invalid)?;
        Ok(mount)
    }

    /// The UI entity's create: no components; the definition alone makes it
    /// a world entity.
    pub fn create() -> EntityUpdate {
        EntityUpdate {
            entity: WORLD_UI_ENTITY,
            operation: Some(Operation::Create(Box::new(Create::new(
                WORLD_UI_DEFINITION,
                Vec::new(),
            )))),
            ..EntityUpdate::default()
        }
    }

    /// The owner frame of the mount, with or without the two widgets. A full
    /// frame destroys the instances it does not list.
    pub fn owner_frame(&self, command_frame: u32, widgets: bool) -> Result<BitWriter, WireError> {
        owner_full(command_frame, &self.instances(widgets, true), &variables())
    }

    /// The remote full frame that rings `120B`'s and `20E7`'s `.0C47`
    /// listeners.
    pub fn ring_frame(&self) -> &BitWriter {
        &self.ring
    }

    fn instances(&self, widgets: bool, owner: bool) -> Vec<Instance> {
        let states = |instance: &mut Instance, states: &[(usize, BitWriter)]| {
            if owner {
                instance.states.extend(states.iter().cloned());
            }
        };
        let mut root = Instance::new(ROOT_ID, self.root, None);
        states(&mut root, &self.root_states);
        let mut machine = Instance::new(
            MACHINE_ID,
            self.machine,
            Some((ROOT_ID, self.root_launcher)),
        );
        states(&mut machine, &self.machine_states);
        let mut hud = Instance::new(HUD_ID, self.hud, None);
        states(&mut hud, &[(self.hud_state, BitWriter::new())]);
        let mut instances = vec![root, machine, hud];
        if widgets {
            let mut ammo = Instance::new(AMMO_ID, self.ammo, None);
            ammo.presence.extend(self.ammo_presence.iter().cloned());
            states(&mut ammo, &self.ammo_states);
            let mut panel = Instance::new(
                PANEL_ID,
                self.panel,
                Some((MACHINE_ID, self.machine_launcher)),
            );
            states(&mut panel, &[(self.panel_state, BitWriter::new())]);
            instances.extend([ammo, panel]);
        }
        instances
    }
}

fn variables() -> Variables {
    MOUNT_VARIABLES
        .iter()
        .map(|&var| (var, Value::Bool(true)))
        .collect()
}

fn owner_payload(
    codecs: &CodecTable,
    class: &str,
    payload: StatePayload,
) -> Result<BitWriter, String> {
    let codec = codecs
        .codec(class)
        .ok_or_else(|| format!("no codec for {class}"))?;
    codec
        .payload(
            &payload,
            PayloadMode::Owner {
                full: true,
                frame_time: 0,
                frame_ms: 0,
            },
        )
        .map_err(|error| format!("{class}: {error}"))
}

/// A lit `GameMessageEntry` remote-sync entry in a remote full frame.
fn remote_message(codecs: &CodecTable) -> Result<BitWriter, String> {
    let codec = codecs
        .codec(MESSAGE)
        .ok_or_else(|| format!("no codec for {MESSAGE}"))?;
    let payload = codec
        .payload(&StatePayload::new(), PayloadMode::Remote { full: true })
        .map_err(|error| format!("{MESSAGE}: {error}"))?;
    let mut bits = BitWriter::new();
    bits.bit(true); // X: present in this full frame
    bits.bit(true); // A: begun
    bits.append(&payload);
    Ok(bits)
}

fn graph(graphs: &GraphStore, index: u32) -> io::Result<&Graph> {
    graphs
        .graph(index)
        .ok_or_else(|| invalid(format!("missing HUD graph {index:04X}")))
}

fn layout(graph: &Graph) -> io::Result<Layout> {
    Layout::try_from(graph).map_err(invalid)
}

/// Each owner slot with its node's class.
fn owner_classes(graph: &Graph) -> impl Iterator<Item = (usize, &str)> {
    graph
        .owner_positions()
        .iter()
        .enumerate()
        .filter_map(|(wire, &pos)| Some((wire, graph.node(pos)?.class.as_str())))
}

fn owner_slot(graph: &Graph, raw: u16, class: &str) -> io::Result<usize> {
    graph
        .state(usize::from(raw))
        .filter(|node| node.class == class)
        .and_then(|_| graph.owner_slot(usize::from(raw)))
        .ok_or_else(|| {
            invalid(format!(
                "HUD graph {:04X} state {raw} is not a networked {class}",
                graph.index
            ))
        })
}

fn only_slot(graph: &Graph, class: &str) -> io::Result<usize> {
    let mut slots = owner_classes(graph).filter(|&(_, found)| found == class);
    match (slots.next(), slots.next()) {
        (Some((wire, _)), None) => Ok(wire),
        _ => Err(invalid(format!(
            "HUD graph {:04X} has no single networked {class}",
            graph.index
        ))),
    }
}

/// The networked launcher whose subgraph is `child`: its owner slot and raw
/// state index.
fn launcher(graph: &Graph, child: u32) -> io::Result<(usize, u16)> {
    graph
        .states
        .iter()
        .enumerate()
        .filter_map(|(raw, pos)| Some((raw, graph.node((*pos)?)?)))
        .find(|(_, node)| {
            node.class == SUBSCRIPT
                && references(&node.fields, 0x0580_0000_0000_0000 | u64::from(child))
        })
        .and_then(|(raw, _)| Some((graph.owner_slot(raw)?, u16::try_from(raw).ok()?)))
        .ok_or_else(|| {
            invalid(format!(
                "HUD graph {:04X} has no networked launcher of {child:04X}",
                graph.index
            ))
        })
}

/// The remote-sync slot of the graph's `GameMessageEntry`, which listens for
/// `message` when one is given.
fn listener(graph: &Graph, message: Option<u64>) -> io::Result<usize> {
    let mut found = graph
        .remote_positions()
        .iter()
        .enumerate()
        .filter(|&(_, &pos)| {
            graph.node(pos).is_some_and(|node| {
                node.class == MESSAGE
                    && message.is_none_or(|message| references(&node.fields, message))
            })
        });
    match (found.next(), found.next()) {
        (Some((slot, _)), None) => Ok(slot),
        _ => Err(invalid(format!(
            "HUD graph {:04X} has no single remote message listener",
            graph.index
        ))),
    }
}

fn references(value: &serde_json::Value, guid: u64) -> bool {
    match value {
        serde_json::Value::Array(items) => items.iter().any(|item| references(item, guid)),
        serde_json::Value::Object(fields) => fields.values().any(|item| references(item, guid)),
        value => asset_guid(value) == Some(guid),
    }
}

fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}
