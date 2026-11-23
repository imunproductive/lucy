//! The host's controls of a match (the dashboard's match page,
//! `refs/reports/Dashboard.md` §4-5): its flow as the dashboard shows it,
//! the flow controls its mode allows, and the kick and stop of its
//! players.
//!
//! A flow control runs the Workshop action that does it in retail, on the
//! mode entity's script (`ModeScript::start_workshop_action`): Set Match
//! Time `1ABC` sends `08D8` {`017E` seconds}, Pause `1D53` and Unpause
//! `1D54` send `06D5`, Declare Team Victory `1384` and Declare Match Draw
//! `1D55` send `09A3` {`008D` team, -1 for a draw}, Declare Round Victory
//! `1E2D` and Round Draw `272A` send `0EAB`, Go To Assemble Heroes `1EF9`
//! sends `0F00`, and Disable / Enable Built-In Game Mode Completion `1ABA`
//! / `1AB9` hold `#18600e` and let it go (`lucy script query show`). The
//! mode's own graph takes the message where it has an entry for it
//! ([`ModeRunner::takes`]), so a mode offers what its graph takes. Where
//! the match flow stands in for the graph (Escort and Hybrid, a graph that
//! doesn't run whole), the flow takes Set Match Time and the declared
//! winner as the graph's entries would (`MatchFlow::receive`): from the
//! action's own message on a Workshop match's mode entity, else directly.
//! Restart Match `2FD8` is the server's own `STU_5E019216` (not run here),
//! and `08D9`/`08DA` have no Workshop action in 1.74; neither is offered.

use std::time::Duration;

use lucy_script::wire::{Value, Variables};
use lucy_sim::{
    match_flow::{DECLARE_VICTORY_MESSAGE, DECLARED_TEAM, Mode, Phase, SET_MATCH_TIME_MESSAGE},
    mode_script::{ModeRunner, takes_set_match_time},
    workshop::{Constant, constant_parameter},
};

use super::RoomState;

/// What the host can tell a match's flow to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    SetTime,
    SkipPhase,
    Pause,
    Unpause,
    DeclareWinner,
    Draw,
    RoundWinner,
    RoundDraw,
    Assemble,
    Completion,
}

impl Control {
    pub const ALL: [Self; 10] = [
        Self::SetTime,
        Self::SkipPhase,
        Self::Pause,
        Self::Unpause,
        Self::DeclareWinner,
        Self::Draw,
        Self::RoundWinner,
        Self::RoundDraw,
        Self::Assemble,
        Self::Completion,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::SetTime => "set_time",
            Self::SkipPhase => "skip_phase",
            Self::Pause => "pause",
            Self::Unpause => "unpause",
            Self::DeclareWinner => "declare_winner",
            Self::Draw => "draw",
            Self::RoundWinner => "round_winner",
            Self::RoundDraw => "round_draw",
            Self::Assemble => "assemble",
            Self::Completion => "completion",
        }
    }

    /// The message its action sends the mode entity.
    fn message(self) -> u64 {
        match self {
            Self::SetTime | Self::SkipPhase => SET_MATCH_TIME_MESSAGE,
            Self::Pause | Self::Unpause => PAUSE_MESSAGE,
            Self::DeclareWinner | Self::Draw => DECLARE_VICTORY_MESSAGE,
            Self::RoundWinner | Self::RoundDraw => ROUND_VICTORY_MESSAGE,
            Self::Assemble => ASSEMBLE_MESSAGE,
            Self::Completion => COMPLETION_MESSAGE,
        }
    }
}

/// A flow control with its values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FlowAction {
    /// The current phase has this many seconds left (0 to 3599).
    SetTime(f64),
    /// The current phase ends: Set Match Time 1, as C32S2's "Match time"
    /// rule skips assembling heroes and setup.
    SkipPhase,
    Pause,
    Unpause,
    /// Team 0 or 1 wins the match.
    DeclareWinner(u8),
    Draw,
    /// Team 0 or 1 wins the round.
    RoundWinner(u8),
    RoundDraw,
    /// Back to assembling heroes.
    Assemble,
    /// The mode's own end at clock 0 on (true) or off.
    Completion(bool),
}

impl FlowAction {
    pub fn control(self) -> Control {
        match self {
            Self::SetTime(_) => Control::SetTime,
            Self::SkipPhase => Control::SkipPhase,
            Self::Pause => Control::Pause,
            Self::Unpause => Control::Unpause,
            Self::DeclareWinner(_) => Control::DeclareWinner,
            Self::Draw => Control::Draw,
            Self::RoundWinner(_) => Control::RoundWinner,
            Self::RoundDraw => Control::RoundDraw,
            Self::Assemble => Control::Assemble,
            Self::Completion(_) => Control::Completion,
        }
    }

    /// The Workshop action graph that does it and its parameters.
    fn action(self) -> (u32, Vec<Value>) {
        let number = |seconds| vec![constant_parameter(Constant::Number(seconds))];
        let team = |team| vec![constant_parameter(Constant::Team(team))];
        match self {
            Self::SetTime(seconds) => (SET_MATCH_TIME, number(seconds)),
            Self::SkipPhase => (SET_MATCH_TIME, number(SKIP_SECONDS)),
            Self::Pause => (PAUSE, Vec::new()),
            Self::Unpause => (UNPAUSE, Vec::new()),
            Self::DeclareWinner(winner) => (DECLARE_TEAM_VICTORY, team(winner)),
            Self::Draw => (DECLARE_MATCH_DRAW, Vec::new()),
            Self::RoundWinner(winner) => (DECLARE_ROUND_VICTORY, team(winner)),
            Self::RoundDraw => (DECLARE_ROUND_DRAW, Vec::new()),
            Self::Assemble => (GO_TO_ASSEMBLE_HEROES, Vec::new()),
            Self::Completion(true) => (ENABLE_COMPLETION, Vec::new()),
            Self::Completion(false) => (DISABLE_COMPLETION, Vec::new()),
        }
    }

    /// The message to the mode entity the flow takes in the graph's place
    /// ([`lucy_sim::match_flow::MatchFlow::receive`]), as the action sends
    /// it.
    fn stand_in(self) -> Option<(u64, Variables)> {
        let parameter = |key, value| Variables::from([(key, value)]);
        Some(match self {
            Self::SetTime(seconds) => (
                SET_MATCH_TIME_MESSAGE,
                parameter(SET_MATCH_TIME_SECONDS, Value::Float(seconds)),
            ),
            Self::SkipPhase => (
                SET_MATCH_TIME_MESSAGE,
                parameter(SET_MATCH_TIME_SECONDS, Value::Float(SKIP_SECONDS)),
            ),
            Self::DeclareWinner(team) => (
                DECLARE_VICTORY_MESSAGE,
                parameter(DECLARED_TEAM, Value::Integer(i64::from(team))),
            ),
            Self::Draw => (
                DECLARE_VICTORY_MESSAGE,
                parameter(DECLARED_TEAM, Value::Integer(-1)),
            ),
            _ => return None,
        })
    }
}

/// Why a flow control did nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlowRefusal {
    /// No such match runs.
    NoMatch,
    /// The match's mode, or its phase now, doesn't allow it (its
    /// [`FlowState::controls`] lack it).
    NotAllowed,
    /// The action's graph didn't run.
    Failed(String),
}

/// What drives a match's flow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Driven {
    /// The mode's own graph ([`ModeRunner::holds_rooms`]).
    Graph,
    /// The match flow, in the graph's place.
    Flow,
    /// No flow: the Practice Range.
    None,
}

/// A match's flow as the host sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct FlowState {
    pub driven: Driven,
    /// The mode's own graph while it drives the match (`0F4A`).
    pub mode_graph: Option<u32>,
    /// None before the flow starts, and in the Range.
    pub phase: Option<Phase>,
    /// Seconds on the match clock.
    pub clock: f64,
    pub paused: bool,
    pub round: u8,
    pub attackers: Option<u8>,
    pub objective: u32,
    pub overtime: bool,
    pub winner: Option<u8>,
    /// The mode's own end at clock 0 is off (`#18600e`).
    pub completion_off: bool,
    /// The match took nobody more: decided, failed or emptied.
    pub ended: bool,
    pub controls: Vec<Control>,
}

/// Set Match Time `1ABC`'s seconds key (`0A4:017E`).
const SET_MATCH_TIME_SECONDS: u32 = 0x017e;
/// [`FlowAction::SkipPhase`]'s Set Match Time (C32S2's rule sets 1 s).
const SKIP_SECONDS: f64 = 1.0;
const PAUSE_MESSAGE: u64 = 0x0240_0000_0000_06d5;
const ROUND_VICTORY_MESSAGE: u64 = 0x0240_0000_0000_0eab;
const ASSEMBLE_MESSAGE: u64 = 0x0240_0000_0000_0f00;
/// What `1AB9` sends to end `1ABA`'s hold (no mode graph takes it; the
/// control depends on `#18600e`, which every mode graph reads).
const COMPLETION_MESSAGE: u64 = 0x0240_0000_0000_0d68;

const SET_MATCH_TIME: u32 = 0x1abc;
const PAUSE: u32 = 0x1d53;
const UNPAUSE: u32 = 0x1d54;
const DECLARE_TEAM_VICTORY: u32 = 0x1384;
const DECLARE_MATCH_DRAW: u32 = 0x1d55;
const DECLARE_ROUND_VICTORY: u32 = 0x1e2d;
const DECLARE_ROUND_DRAW: u32 = 0x272a;
const GO_TO_ASSEMBLE_HEROES: u32 = 0x1ef9;
const ENABLE_COMPLETION: u32 = 0x1ab9;
const DISABLE_COMPLETION: u32 = 0x1aba;

impl RoomState {
    /// The match's flow as the host sees it.
    pub(super) fn flow_state(&self) -> FlowState {
        let room = &self.room;
        let mode = room.mode.as_ref();
        let graph = mode.and_then(ModeRunner::match_state);
        let flow = room.flow.as_ref();
        let driven = match (flow, mode) {
            (None, _) => Driven::None,
            (Some(_), Some(mode)) if mode.holds_rooms() => Driven::Graph,
            (Some(_), _) => Driven::Flow,
        };
        let mut state = FlowState {
            driven,
            mode_graph: mode.and_then(ModeRunner::mode_graph),
            phase: None,
            clock: 0.0,
            paused: mode.is_some_and(ModeRunner::paused),
            round: flow.map_or(0, |flow| flow.round()),
            attackers: None,
            objective: 0,
            overtime: false,
            winner: None,
            completion_off: mode.is_some_and(ModeRunner::completion_off),
            ended: room.ended,
            controls: Vec::new(),
        };
        match (driven, graph, flow) {
            (Driven::Graph, Some(graph), _) => {
                state.phase = Some(graph.phase);
                state.clock = graph.clock;
                state.attackers = graph.attackers;
                state.objective = graph.objective;
                state.overtime = graph.overtime;
                state.winner = graph.winner;
            }
            (Driven::Flow, _, Some(flow)) => {
                state.phase = Some(flow.phase());
                state.clock = flow.clock();
                state.attackers = flow.attackers();
                state.overtime = flow.in_overtime();
            }
            _ => {}
        }
        state.controls = self.controls(&state);
        state
    }

    /// What the match's mode and phase allow now.
    fn controls(&self, state: &FlowState) -> Vec<Control> {
        let room = &self.room;
        let on = matches!(
            state.phase,
            Some(Phase::Assemble | Phase::Setup | Phase::Round)
        );
        let (Some(flow), true) = (room.flow.as_ref(), on && !room.ended && !room.closed()) else {
            return Vec::new();
        };
        let teams = !room.map.free_for_all && flow.mode() != Mode::Deathmatch;
        match room.mode.as_ref() {
            Some(mode) if state.driven == Driven::Graph => Control::ALL
                .into_iter()
                .filter(|&control| match control {
                    Control::Pause => mode.takes(control.message()) && !state.paused,
                    Control::Unpause => mode.takes(control.message()) && state.paused,
                    Control::DeclareWinner | Control::Draw | Control::RoundWinner => {
                        teams && mode.takes(control.message())
                    }
                    Control::Completion => true,
                    control => mode.takes(control.message()),
                })
                .collect(),
            _ if takes_set_match_time(room.map.mode_guid) => {
                let mut controls = vec![Control::SetTime, Control::SkipPhase];
                if teams {
                    controls.extend([Control::DeclareWinner, Control::Draw]);
                }
                controls
            }
            _ => Vec::new(),
        }
    }

    /// The host's flow control: its Workshop action on the mode entity's
    /// script; with none (the hand-built mode frame), the flow takes the
    /// action's message in the graph's place.
    pub(super) fn run_flow(&mut self, action: FlowAction) -> Result<(), FlowRefusal> {
        let state = self.flow_state();
        if !state.controls.contains(&action.control()) {
            return Err(FlowRefusal::NotAllowed);
        }
        if let FlowAction::Completion(on) = action
            && on != state.completion_off
        {
            // Already so: `1ABA` again would hold `#18600e` twice.
            return Err(FlowRefusal::NotAllowed);
        }
        let room = &mut self.room;
        let (graph, parameters) = action.action();
        match room.mode.as_mut().and_then(ModeRunner::script_mut) {
            Some(script) => match script.start_workshop_action(graph, parameters) {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return Err(FlowRefusal::Failed(format!(
                        "Workshop action graph {graph:04X} is not in the bundle"
                    )));
                }
                Err(error) => {
                    return Err(FlowRefusal::Failed(format!(
                        "Workshop action graph {graph:04X} stopped ({error})"
                    )));
                }
            },
            None => {
                let (Some(flow), Some((message, parameters))) =
                    (room.flow.as_mut(), action.stand_in())
                else {
                    return Err(FlowRefusal::NotAllowed);
                };
                flow.receive(message, &parameters);
            }
        }
        tracing::info!(
            "[game] match {:04X}: the host's {:?}",
            self.id[0] & 0xFFFF,
            action
        );
        Ok(())
    }

    /// The host takes the player of `account` out: his game goes home
    /// (20304), as a released seat's does. False when he isn't seated.
    pub(super) fn kick(&mut self, account: u32, now: Duration) -> bool {
        let found = self.seats.iter().find_map(|(&connection, &slot)| {
            let player = &self.room.players[slot];
            (!player.ai && !player.gone && player.card.account[0] == u64::from(account))
                .then_some(connection)
        });
        let Some(connection) = found else {
            return false;
        };
        self.release(connection, now);
        true
    }

    /// The host stops the match: every player's game goes home now
    /// (20304), as for released seats; how many there were.
    pub(super) fn stop(&mut self, now: Duration) -> usize {
        let seated: Vec<u32> = self
            .seats
            .iter()
            .filter(|&(_, &slot)| !self.room.players[slot].ai)
            .map(|(&connection, _)| connection)
            .collect();
        for &connection in &seated {
            self.release(connection, now);
        }
        seated.len()
    }

    /// A seat's player leaves, as `GameService::release_seat` takes him
    /// out: a game not connected yet now, a connected one by 20304.
    pub(super) fn release(&mut self, connection: u32, now: Duration) {
        if self.links.waiting(connection) {
            self.take_out_waiting(connection);
        } else {
            self.send_home(connection, now);
        }
    }
}
