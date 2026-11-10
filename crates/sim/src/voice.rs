//! A player's spawn bark (`ow174/game/voicelines.py`).
//!
//! When a player spawns, the server begins one of the hero's voice-stimulus
//! states on his own body statescript, and the client that runs the state
//! plays the line. No message of its own is needed: a networked state that is
//! on rides the body's ordinary statescript frames. The hero's voice graphs are
//! subscripts of the body, so they are live instances of its component by the
//! spawn. Which line retail plays is unverified: the first networked stimulus
//! is played unless [`PREFERRED_STIMULUS`] names one for the hero.

use std::time::Duration;

use orpheus_script::{
    graph::GraphStore,
    vm::{Component, InitFault, World},
};
use serde_json::Value as Json;

/// The statescript state that plays a bark on the client (STU_40E05F6B, the
/// holder of the client's `STUConfigVarVoiceStimulus`; `VOICE_CLASS`).
pub const VOICE_CLASS: &str = "STU_40E05F6B";

/// Spawn barks are on (`VOICE_LINES`).
pub const VOICE_LINES: bool = true;

/// How long the stimulus state stays on before the server ends it
/// (`BARK_SECONDS`): long enough for any line.
pub const BARK: Duration = Duration::from_secs(4);

/// Hero GUID to the voice-stimulus GUID (0x0EE0…) to play on spawn instead of
/// the first networked one (`PREFERRED_STIMULUS`). Empty until lines are
/// picked in the game.
pub const PREFERRED_STIMULUS: &[(u64, u64)] = &[];

/// Instance, raw state index and stimulus GUID of every networked voice
/// stimulus of the body's live instances, in the component's instance order
/// (creation order, so instance id order) then state index order
/// (`_candidates`).
pub fn candidates(component: &Component, graphs: &GraphStore) -> Vec<(u16, u16, Option<u64>)> {
    let Some(snapshot) = component.snapshot() else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for (&id, instance) in &snapshot.instances {
        let Some(graph) = graphs.graph(instance.graph) else {
            continue;
        };
        for index in 0..graph.states.len() {
            let Some(node) = graph
                .state(index)
                .filter(|node| node.class == VOICE_CLASS && node.is_networked())
            else {
                continue;
            };
            let Ok(raw) = u16::try_from(index) else {
                continue;
            };
            let stimulus = node
                .fields
                .get("m_stimulus")
                .filter(|config| config.is_object())
                .and_then(|config| config.get("m_voiceStimulus"))
                .and_then(guid);
            found.push((id, raw, stimulus));
        }
    }
    found
}

/// A GUID written as a `0x` hex string or a plain integer (`_guid`).
fn guid(value: &Json) -> Option<u64> {
    match value {
        Json::String(text) => u64::from_str_radix(text.strip_prefix("0x")?, 16).ok(),
        Json::Number(number) => number.as_u64(),
        _ => None,
    }
}

/// The stimulus to play: the one `preferred` names for the hero, else the
/// first candidate (`_pick`).
fn pick(
    candidates: &[(u16, u16, Option<u64>)],
    hero: u64,
    preferred: &[(u64, u64)],
) -> Option<(u16, u16)> {
    let first = candidates.first()?;
    let wanted = preferred
        .iter()
        .find(|&&(for_hero, _)| for_hero == hero)
        .map(|&(_, stimulus)| stimulus);
    let chosen = wanted
        .and_then(|wanted| {
            candidates
                .iter()
                .find(|&&(_, _, stimulus)| stimulus == Some(wanted))
        })
        .unwrap_or(first);
    Some((chosen.0, chosen.1))
}

/// A bark begun at a spawn, due to end at `end`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpawnBark {
    pub end: Duration,
    pub instance: u16,
    pub raw_state: u16,
}

/// Play the hero's spawn line on this body: begin the picked voice stimulus
/// now (`VoiceLines.spawned`). None when barks are off, the hero has no
/// networked stimulus, or the picked one is already on.
pub fn begin_spawn_bark(
    component: &mut Component,
    graphs: &GraphStore,
    hero: u64,
    now: Duration,
    world: &mut impl World,
) -> Result<Option<SpawnBark>, InitFault> {
    if !VOICE_LINES {
        return Ok(None);
    }
    let Some((instance, raw_state)) =
        pick(&candidates(component, graphs), hero, PREFERRED_STIMULUS)
    else {
        return Ok(None);
    };
    if !component.begin_state(instance, raw_state, world)? {
        return Ok(None);
    }
    Ok(Some(SpawnBark {
        end: now + BARK,
        instance,
        raw_state,
    }))
}

impl SpawnBark {
    /// End the bark once it is due (`VoiceLines.expire`): true when due, and
    /// the state is finished if it is still on; false before. A body that went
    /// away is the caller's to drop: its states went with it.
    pub fn expire(
        &self,
        now: Duration,
        component: &mut Component,
        world: &mut impl World,
    ) -> Result<bool, InitFault> {
        if now < self.end {
            return Ok(false);
        }
        component.end_state(self.instance, self.raw_state, true, world)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use orpheus_script::vm::{EmptyWorld, MAX_INIT_WORK, Root};
    use orpheus_script::wire::Variables;
    use serde_json::json;

    use super::*;

    const HERO: u64 = 0x02e0_0000_0000_0001;

    fn voice(position: u16, state: u16, server_only: bool, stimulus: &str) -> Json {
        json!({
            "pos":position,"class":VOICE_CLASS,"uid":position,"rid":position,"remote":-1,
            "state":state,"client_only":false,"server_only":server_only,"fields":{
                "m_2BBEEAB8":true,
                "m_stimulus":{"$":"STUConfigVarVoiceStimulus","m_voiceStimulus":stimulus}
            }
        })
    }

    /// One body graph with two networked voice stimuli (states 0 and 2) and a
    /// server-only one (state 1); nothing begins them.
    fn fixture() -> (Arc<GraphStore>, Component) {
        let graph = json!({
            "index":42,"items":4,"states":[1,2,3],"entries":[0],"remote":[],
            "sync_vars":[],"containers":{},"unknown_nodes":[],"fields":{},
            "header":{"m_nodesBitCount":4,"m_statesBitCount":4,
                "m_remoteSyncNodesBitCount":4,"m_syncVarsBitCount":4},
            "nodes":[
                {"pos":0,"class":"STUStatescriptEntry","uid":0,"rid":0,"remote":-1,
                    "state":null,"client_only":false,"server_only":false,"fields":{
                    "m_outPlug":{"$":"Plug","links":[],"m_FF3DAF1E":[]}}},
                voice(1, 0, false, "0x0EE0000000000001"),
                voice(2, 1, true, "0x0EE0000000000003"),
                voice(3, 2, false, "0x0EE0000000000002"),
            ]
        });
        let bundle = json!({"version":1,"missing":[],"graphs":[graph],"bodies":{}});
        let graphs =
            Arc::new(GraphStore::from_json(&serde_json::to_vec(&bundle).unwrap()).unwrap());
        let component = Component::start(
            Arc::clone(&graphs),
            0xa000_0101,
            Variables::new(),
            vec![Root::new(1, 42)],
            100,
            &mut EmptyWorld,
            MAX_INIT_WORK,
        )
        .unwrap();
        (graphs, component)
    }

    fn on(component: &Component, raw: u16) -> bool {
        component.snapshot().unwrap().instances[&1]
            .states
            .get(&raw)
            .is_some_and(|state| state.active)
    }

    #[test]
    fn candidates_are_the_networked_voice_stimuli_and_a_preferred_one_wins() {
        let (graphs, component) = fixture();
        let found = candidates(&component, &graphs);
        assert_eq!(
            found,
            [
                (1, 0, Some(0x0ee0_0000_0000_0001)),
                (1, 2, Some(0x0ee0_0000_0000_0002))
            ]
        );
        assert_eq!(pick(&found, HERO, PREFERRED_STIMULUS), Some((1, 0)));
        assert_eq!(
            pick(&found, HERO, &[(HERO, 0x0ee0_0000_0000_0002)]),
            Some((1, 2))
        );
        assert_eq!(
            pick(&found, HERO, &[(HERO + 1, 0x0ee0_0000_0000_0002)]),
            Some((1, 0))
        );
        assert_eq!(pick(&[], HERO, PREFERRED_STIMULUS), None);
        assert_eq!(guid(&json!(5)), Some(5));
        assert_eq!(guid(&json!("0xZZ")), None);
        assert_eq!(guid(&Json::Null), None);
    }

    #[test]
    fn spawn_bark_turns_the_stimulus_on_until_it_is_due() {
        let (graphs, mut component) = fixture();
        let now = Duration::from_secs(10);
        let bark = begin_spawn_bark(&mut component, &graphs, HERO, now, &mut EmptyWorld)
            .unwrap()
            .unwrap();
        assert_eq!(
            bark,
            SpawnBark {
                end: now + BARK,
                instance: 1,
                raw_state: 0
            }
        );
        assert!(on(&component, 0));
        // The same stimulus is already on: no second bark.
        assert_eq!(
            begin_spawn_bark(&mut component, &graphs, HERO, now, &mut EmptyWorld).unwrap(),
            None
        );
        let early = now + BARK - Duration::from_millis(1);
        assert!(!bark.expire(early, &mut component, &mut EmptyWorld).unwrap());
        assert!(on(&component, 0));
        assert!(
            bark.expire(now + BARK, &mut component, &mut EmptyWorld)
                .unwrap()
        );
        assert!(!on(&component, 0));
        // Due again after it already ended: nothing left to end.
        assert!(
            bark.expire(now + BARK, &mut component, &mut EmptyWorld)
                .unwrap()
        );
        assert!(component.snapshot().unwrap().initialization_complete());
    }
}
