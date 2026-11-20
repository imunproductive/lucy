//! The rule engine: the native half of the Workshop (Workshop.md §5). It
//! decides when rules run, keeps their execution state, evaluates values,
//! and starts each action's graph through [`Actions`], waiting for it to
//! finish before the next action.
//!
//! The engine runs itself: the control-flow actions with no graph (If,
//! Else If, Else, While, End, For, Break, Continue, Call Subroutine, Start
//! Rule) and the actions whose graphs only compute a result for the native
//! engine and have no client-visible node (Workshop.md §2: Wait `1370` and
//! Wait Until `2E19` (a `StateWait` and the engine message `0D65`), Skip
//! and Skip If (`STU_DC325C52`), Abort… (`STU_ADAFD7FF`), Loop…
//! (`STU_99ED4486`), and Set/Modify Global/Player Variable [At Index]
//! (`STU_0BDBC7B2` writes the entity variable, which [`Variables`] is)).
//! Every other action is its graph.
//!
//! When rules run is not in the client's data; the engine follows the
//! Workshop's documented behaviour [assumptions, each cited]:
//! - Rules are processed in their order each server tick; a triggered
//!   rule runs its actions at once, until a Wait or an action still
//!   running.
//! - An Ongoing rule runs when all its conditions are true and at least
//!   one has been false since it last ran, and never while it is already
//!   running (the Workshop forum's "Tips and Tricks from a programmer",
//!   us.forums.blizzard.com/en/overwatch/t/355900: "A rule will only
//!   perform the actions one time when all conditions become true…", "A
//!   rule will not activate a second time if it is still performing
//!   actions"). Ongoing - Global has one instance; Ongoing - Each Player
//!   one per player matching the event's team and player filters.
//! - A player event (Player Died…) runs the rule for that player when its
//!   conditions are true at that moment, unless it is already running for
//!   that player.
//! - Wait's behaviours, Loop, Skip, Abort, If…End, While, For, Break,
//!   Continue, Call Subroutine and Start Rule are as their catalog
//!   descriptions (`054/0155`) say. Wait's time is clamped to
//!   0.016-100000 s as its graph does (`1370`) and lasts whole ticks,
//!   rounded up; Wait Until checks its condition at once and then every
//!   tick.
//! - Disabled rules, conditions and actions are left out, so Skip counts
//!   only enabled actions.

use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

use crate::{
    catalog::{Catalog, Id},
    eval::{self, Rng, Scope, arithmetic, compare},
    host::{ActionCall, ActionHandle, Context, GameEvent, Host, Param},
    logic::{self, Action, Arg, Condition, Event, Logic, WorkshopSetting, actions, events},
    value::{EntityId, Value},
};

/// Actions one execution may run in one tick before it is stopped: the
/// retail server closes a game whose rules loop without waiting
/// ("excessive Workshop script load", `07C:C8F3`).
const STEP_LIMIT: usize = 100_000;
/// Nested Call Subroutine depth before an execution is stopped.
const CALL_DEPTH: usize = 256;

/// Wait's behaviours (`STU_7BF2036D` enumVal, `07C:787C`-`787E`).
const IGNORE_CONDITION: u8 = 0;
const ABORT_WHEN_FALSE: u8 = 1;
const RESTART_WHEN_TRUE: u8 = 2;

/// Start Rule's "If Already Executing" (enum `6F55`).
const RESTART_RULE: Id = 0x6F57;

/// The Modify Variable operations (enum `4A28`).
mod modify {
    use crate::catalog::Id;

    pub const ADD: Id = 0x4A29;
    pub const SUBTRACT: Id = 0x4A2A;
    pub const MULTIPLY: Id = 0x4A2B;
    pub const DIVIDE: Id = 0x4A2C;
    pub const MODULO: Id = 0x4A2D;
    pub const RAISE_TO_POWER: Id = 0x4A2E;
    pub const MIN: Id = 0x4A5A;
    pub const MAX: Id = 0x4A5B;
    pub const APPEND_TO_ARRAY: Id = 0x4A2F;
    pub const REMOVE_FROM_ARRAY_BY_VALUE: Id = 0x4A30;
    pub const REMOVE_FROM_ARRAY_BY_INDEX: Id = 0x56A3;
}

/// Where control goes from an action (resolved once per rule).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flow {
    Plain,
    /// If / Else If: the next Else If, Else or End of the chain, and the
    /// chain's End.
    Branch {
        next: usize,
        end: usize,
    },
    Else {
        end: usize,
    },
    Loop {
        end: usize,
    },
    /// The If, While or For it closes, if any.
    End {
        opener: Option<usize>,
    },
    /// Break and Continue: the innermost While or For.
    Exit {
        loop_start: Option<usize>,
    },
}

struct Compiled {
    name: String,
    event: Event,
    conditions: Vec<Condition>,
    actions: Vec<Arc<Action>>,
    flow: Vec<Flow>,
}

impl Compiled {
    fn new(rule: &crate::logic::Rule) -> Self {
        let actions: Vec<Arc<Action>> = rule
            .actions
            .iter()
            .filter(|action| !action.disabled)
            .cloned()
            .map(Arc::new)
            .collect();
        let flow = flow(&actions);
        Self {
            name: rule.name.clone(),
            event: rule.event.clone(),
            conditions: rule
                .conditions
                .iter()
                .filter(|condition| !condition.disabled)
                .cloned()
                .collect(),
            actions,
            flow,
        }
    }
}

/// The block structure of an action list.
fn flow(list: &[Arc<Action>]) -> Vec<Flow> {
    let mut flow = vec![Flow::Plain; list.len()];
    // Open blocks: whether a loop, and the If chain's members (or the loop).
    let mut open: Vec<(bool, Vec<usize>)> = Vec::new();
    let close = |flow: &mut Vec<Flow>, (looping, members): (bool, Vec<usize>), end: usize| {
        for (position, &member) in members.iter().enumerate() {
            flow[member] = if looping {
                Flow::Loop { end }
            } else if list[member].index == actions::ELSE {
                Flow::Else { end }
            } else {
                Flow::Branch {
                    next: members.get(position + 1).copied().unwrap_or(end),
                    end,
                }
            };
        }
    };
    for (at, action) in list.iter().enumerate() {
        match action.index {
            actions::IF => open.push((false, vec![at])),
            actions::ELSE_IF | actions::ELSE => {
                if let Some((false, members)) = open.last_mut() {
                    members.push(at);
                }
            }
            actions::WHILE | actions::FOR_GLOBAL_VARIABLE | actions::FOR_PLAYER_VARIABLE => {
                open.push((true, vec![at]))
            }
            actions::END => {
                let opener = open.pop();
                flow[at] = Flow::End {
                    opener: opener.as_ref().map(|(_, members)| members[0]),
                };
                if let Some(block) = opener {
                    close(&mut flow, block, at);
                }
            }
            actions::BREAK | actions::CONTINUE => {
                flow[at] = Flow::Exit {
                    loop_start: open
                        .iter()
                        .rev()
                        .find(|(looping, _)| *looping)
                        .map(|(_, members)| members[0]),
                };
            }
            _ => {}
        }
    }
    // A block the list leaves open ends after its last action.
    while let Some(block) = open.pop() {
        close(&mut flow, block, list.len());
    }
    flow
}

/// A rule instance: the rule and its event player (none for global).
type Key = (usize, Option<EntityId>);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Block {
    Ready,
    Wait {
        until: u64,
        behavior: u8,
        conditions: bool,
    },
    WaitUntil {
        deadline: u64,
    },
    Action(ActionHandle),
}

#[derive(Clone, Copy, Debug)]
struct Frame {
    rule: usize,
    pc: usize,
}

struct Execution {
    key: Key,
    context: Context,
    frames: Vec<Frame>,
    block: Block,
}

enum Step {
    Continue,
    Blocked,
    Done,
}

/// The rules of one match and their state.
pub struct Engine {
    catalog: Arc<Catalog>,
    rules: Vec<Compiled>,
    settings: Vec<WorkshopSetting>,
    tick_seconds: f64,
    tick: u64,
    /// Ongoing instances: whether a condition has been false since the
    /// rule last ran.
    armed: HashMap<Key, bool>,
    executions: Vec<Execution>,
    events: Vec<GameEvent>,
    players: Vec<EntityId>,
    rng: Rng,
    unsupported: BTreeSet<Id>,
    faults: Vec<String>,
}

impl Engine {
    /// The enabled rules of `logic` (normalised), ticking every
    /// `tick_seconds` seconds.
    pub fn new(logic: &Logic, catalog: Arc<Catalog>, tick_seconds: f64, seed: u64) -> Self {
        Self {
            catalog,
            rules: logic
                .rules
                .iter()
                .filter(|rule| !rule.disabled)
                .map(Compiled::new)
                .collect(),
            settings: logic.workshop_settings.clone(),
            tick_seconds,
            tick: 0,
            armed: HashMap::new(),
            executions: Vec::new(),
            events: Vec::new(),
            players: Vec::new(),
            rng: Rng::new(seed),
            unsupported: BTreeSet::new(),
            faults: Vec::new(),
        }
    }

    /// Report a player event; rules for it run on the next tick.
    pub fn fire(&mut self, event: GameEvent) {
        self.events.push(event);
    }

    /// How many rule executions are running (waiting included).
    pub fn running(&self) -> usize {
        self.executions.len()
    }

    /// Whether rule `name` is running for `player` (none: global).
    pub fn is_running(&self, name: &str, player: Option<EntityId>) -> bool {
        self.executions
            .iter()
            .any(|execution| execution.key.1 == player && self.rules[execution.key.0].name == name)
    }

    /// Values the engine met and does not evaluate, by name.
    pub fn unsupported(&self) -> Vec<String> {
        self.unsupported
            .iter()
            .map(|&id| {
                self.catalog
                    .value(id)
                    .map_or_else(|| format!("{id:04X}"), |def| def.name.clone())
            })
            .collect()
    }

    /// Executions the engine stopped (runaway loops, deep recursion).
    pub fn faults(&self) -> &[String] {
        &self.faults
    }

    /// Seconds since the engine started (Total Time Elapsed).
    pub fn elapsed(&self) -> f64 {
        self.tick as f64 * self.tick_seconds
    }

    /// One server tick.
    pub fn tick<H: Host + ?Sized>(&mut self, host: &mut H) {
        self.tick += 1;
        self.sync_players(&*host);
        let events = std::mem::take(&mut self.events);
        for rule in 0..self.rules.len() {
            let event = self.rules[rule].event.clone();
            match event.kind {
                events::ONGOING_GLOBAL => self.ongoing(rule, None, &*host),
                events::ONGOING_EACH_PLAYER => {
                    let players: Vec<EntityId> = self
                        .players
                        .iter()
                        .copied()
                        .filter(|&player| matches_filter(&event, player, &*host))
                        .collect();
                    // A player that stops matching loses the instance.
                    self.executions.retain(|execution| {
                        execution.key.0 != rule
                            || execution
                                .key
                                .1
                                .is_none_or(|player| players.contains(&player))
                    });
                    for player in players {
                        self.ongoing(rule, Some(player), &*host);
                    }
                }
                events::SUBROUTINE => {}
                kind => {
                    for fired in events.iter().filter(|fired| fired.kind == kind) {
                        let Some(player) = fired.context.event_player else {
                            continue;
                        };
                        let key = (rule, Some(player));
                        if matches_filter(&event, player, &*host)
                            && !self.running_key(key)
                            && self.conditions(rule, &fired.context, &*host)
                        {
                            self.start(key, fired.context.clone());
                        }
                    }
                }
            }
            self.run_rule(rule, host);
        }
    }

    fn running_key(&self, key: Key) -> bool {
        self.executions.iter().any(|execution| execution.key == key)
    }

    /// Players who joined or left since the last tick: Player Joined Match
    /// and Player Left Match, and the end of a leaving player's rules.
    fn sync_players<H: Host + ?Sized>(&mut self, host: &H) {
        let players = host.players();
        for &player in &players {
            if !self.players.contains(&player) {
                self.events
                    .insert(0, joined_or_left(events::PLAYER_JOINED_MATCH, player));
            }
        }
        for &player in &self.players {
            if !players.contains(&player) {
                self.executions
                    .retain(|execution| execution.key.1 != Some(player));
                self.armed.retain(|key, _| key.1 != Some(player));
                self.events
                    .push(joined_or_left(events::PLAYER_LEFT_MATCH, player));
            }
        }
        self.players = players;
    }

    /// An Ongoing instance: run on the rising edge of its conditions.
    fn ongoing<H: Host + ?Sized>(&mut self, rule: usize, player: Option<EntityId>, host: &H) {
        let key = (rule, player);
        let context = Context {
            event_player: player,
            ..Context::default()
        };
        let now = self.conditions(rule, &context, host);
        let running = self.running_key(key);
        let armed = self.armed.entry(key).or_insert(true);
        if !now {
            *armed = true;
        } else if *armed && !running {
            *armed = false;
            self.start(key, context);
        }
    }

    fn start(&mut self, key: Key, context: Context) {
        self.executions.push(Execution {
            key,
            context,
            frames: vec![Frame { rule: key.0, pc: 0 }],
            block: Block::Ready,
        });
    }

    /// Whether every enabled condition of `rule` holds.
    fn conditions<H: Host + ?Sized>(&mut self, rule: usize, context: &Context, host: &H) -> bool {
        let catalog = Arc::clone(&self.catalog);
        let settings = std::mem::take(&mut self.settings);
        let elapsed = self.elapsed();
        let mut scope = Scope::new(
            &catalog,
            context,
            &settings,
            elapsed,
            &mut self.rng,
            &mut self.unsupported,
        );
        let holds = self.rules[rule].conditions.iter().all(|condition| {
            let left = eval::evaluate(&mut scope, host, &condition.left);
            let right = eval::evaluate(&mut scope, host, &condition.right);
            compare(&left, condition.op, &right) != condition.invert
        });
        self.settings = settings;
        holds
    }

    fn evaluate<H: Host + ?Sized>(&mut self, context: &Context, host: &H, arg: &Arg) -> Value {
        let catalog = Arc::clone(&self.catalog);
        let settings = std::mem::take(&mut self.settings);
        let elapsed = self.elapsed();
        let mut scope = Scope::new(
            &catalog,
            context,
            &settings,
            elapsed,
            &mut self.rng,
            &mut self.unsupported,
        );
        let value = eval::evaluate(&mut scope, host, arg);
        self.settings = settings;
        value
    }

    /// An action's parameters in `context`, as [`ActionCall::params`]
    /// gives them: a parameter that names Local Player once per player.
    /// The server evaluates a running action's reevaluated parameters
    /// again with it.
    pub fn evaluate_params<H: Host + ?Sized>(
        &mut self,
        host: &H,
        action: &Action,
        context: &Context,
    ) -> Vec<Param> {
        action
            .args
            .iter()
            .map(|arg| {
                let value = self.evaluate(context, host, arg);
                if !names(arg, eval::ids::LOCAL_PLAYER) {
                    return Param::Value(value);
                }
                let viewers = host
                    .players()
                    .into_iter()
                    .map(|player| {
                        let context = Context {
                            local_player: Some(player),
                            ..context.clone()
                        };
                        (player, self.evaluate(&context, host, arg))
                    })
                    .collect();
                Param::PerViewer { value, viewers }
            })
            .collect()
    }

    /// Advance every execution of `rule` (its own, and those Start Rule
    /// began on it) until each waits or ends.
    fn run_rule<H: Host + ?Sized>(&mut self, rule: usize, host: &mut H) {
        let mut at = 0;
        while at < self.executions.len() {
            if self.executions[at].key.0 != rule {
                at += 1;
                continue;
            }
            if self.run(at, host) {
                self.executions.remove(at);
            } else {
                at += 1;
            }
        }
    }

    /// Run one execution; whether it ended.
    fn run<H: Host + ?Sized>(&mut self, at: usize, host: &mut H) -> bool {
        let mut steps = 0;
        loop {
            match self.unblock(at, host) {
                Step::Continue => {}
                Step::Blocked => return false,
                Step::Done => return true,
            }
            steps += 1;
            if steps > STEP_LIMIT {
                let rule = &self.rules[self.executions[at].key.0].name;
                self.faults.push(format!(
                    "rule {rule:?} ran {STEP_LIMIT} actions in one tick"
                ));
                return true;
            }
            match self.step(at, host) {
                Step::Continue => {}
                Step::Blocked => return false,
                Step::Done => return true,
            }
        }
    }

    /// Whether the execution's wait is over.
    fn unblock<H: Host + ?Sized>(&mut self, at: usize, host: &mut H) -> Step {
        match self.executions[at].block {
            Block::Ready => Step::Continue,
            Block::Action(handle) => {
                if host.finished(handle) {
                    self.executions[at].block = Block::Ready;
                    Step::Continue
                } else {
                    Step::Blocked
                }
            }
            Block::Wait {
                until,
                behavior,
                conditions: before,
            } => {
                if behavior != IGNORE_CONDITION {
                    let (rule, context) = (
                        self.executions[at].key.0,
                        self.executions[at].context.clone(),
                    );
                    let now = self.conditions(rule, &context, &*host);
                    if behavior == ABORT_WHEN_FALSE && !now {
                        return Step::Done;
                    }
                    if behavior == RESTART_WHEN_TRUE && now && !before {
                        let execution = &mut self.executions[at];
                        execution.frames = vec![Frame { rule, pc: 0 }];
                        execution.block = Block::Ready;
                        return Step::Continue;
                    }
                    if let Block::Wait { conditions, .. } = &mut self.executions[at].block {
                        *conditions = now;
                    }
                }
                if self.tick >= until {
                    self.executions[at].block = Block::Ready;
                    Step::Continue
                } else {
                    Step::Blocked
                }
            }
            Block::WaitUntil { deadline } => {
                let (frame, context) = {
                    let execution = &self.executions[at];
                    (
                        *execution.frames.last().expect("a waiting frame"),
                        execution.context.clone(),
                    )
                };
                // The Wait Until is the action before the frame's pc.
                let action = Arc::clone(&self.rules[frame.rule].actions[frame.pc - 1]);
                let done = self.evaluate(&context, &*host, &action.args[0]).truthy();
                if done || self.tick >= deadline {
                    self.executions[at].block = Block::Ready;
                    Step::Continue
                } else {
                    Step::Blocked
                }
            }
        }
    }

    /// Run the action at the execution's pc.
    fn step<H: Host + ?Sized>(&mut self, at: usize, host: &mut H) -> Step {
        let Some(&frame) = self.executions[at].frames.last() else {
            return Step::Done;
        };
        let rule = &self.rules[frame.rule];
        let Some(action) = rule.actions.get(frame.pc).cloned() else {
            // The end of a list: return from a subroutine, or finish.
            let execution = &mut self.executions[at];
            execution.frames.pop();
            return if execution.frames.is_empty() {
                Step::Done
            } else {
                Step::Continue
            };
        };
        let flow = rule.flow[frame.pc];
        let context = self.executions[at].context.clone();
        let root = self.executions[at].key.0;
        let arg = |index: usize| action.args.get(index).cloned().unwrap_or(Arg::Bool(false));
        let next = frame.pc + 1;
        match action.index {
            actions::WAIT => {
                let seconds = self
                    .evaluate(&context, &*host, &arg(0))
                    .number()
                    .clamp(0.016, 100_000.0);
                let ticks = ((seconds / self.tick_seconds) - 1e-9).ceil().max(1.0) as u64;
                let behavior = match arg(1) {
                    Arg::WaitBehavior(behavior) => behavior,
                    _ => IGNORE_CONDITION,
                };
                let conditions =
                    behavior != IGNORE_CONDITION && self.conditions(root, &context, &*host);
                self.jump(at, next);
                self.executions[at].block = Block::Wait {
                    until: self.tick + ticks,
                    behavior,
                    conditions,
                };
                Step::Blocked
            }
            actions::WAIT_UNTIL => {
                let timeout = self
                    .evaluate(&context, &*host, &arg(1))
                    .number()
                    .clamp(0.0, 1_000_000.0);
                self.jump(at, next);
                if self.evaluate(&context, &*host, &arg(0)).truthy() {
                    return Step::Continue;
                }
                let ticks = ((timeout / self.tick_seconds) - 1e-9).ceil().max(0.0) as u64;
                self.executions[at].block = Block::WaitUntil {
                    deadline: self.tick + ticks,
                };
                Step::Blocked
            }
            actions::SKIP | actions::SKIP_IF => {
                let (condition, count) = if action.index == actions::SKIP {
                    (true, self.evaluate(&context, &*host, &arg(0)))
                } else {
                    (
                        self.evaluate(&context, &*host, &arg(0)).truthy(),
                        self.evaluate(&context, &*host, &arg(1)),
                    )
                };
                let skip = if condition {
                    count.number().clamp(0.0, 99.0) as usize
                } else {
                    0
                };
                self.jump(at, next + skip);
                Step::Continue
            }
            actions::ABORT => Step::Done,
            actions::ABORT_IF => {
                if self.evaluate(&context, &*host, &arg(0)).truthy() {
                    Step::Done
                } else {
                    self.jump(at, next);
                    Step::Continue
                }
            }
            actions::ABORT_IF_CONDITION_IS_TRUE | actions::ABORT_IF_CONDITION_IS_FALSE => {
                let holds = self.conditions(root, &context, &*host);
                if holds == (action.index == actions::ABORT_IF_CONDITION_IS_TRUE) {
                    Step::Done
                } else {
                    self.jump(at, next);
                    Step::Continue
                }
            }
            actions::LOOP
            | actions::LOOP_IF
            | actions::LOOP_IF_CONDITION_IS_TRUE
            | actions::LOOP_IF_CONDITION_IS_FALSE => {
                let again = match action.index {
                    actions::LOOP => true,
                    actions::LOOP_IF => self.evaluate(&context, &*host, &arg(0)).truthy(),
                    actions::LOOP_IF_CONDITION_IS_TRUE => self.conditions(root, &context, &*host),
                    _ => !self.conditions(root, &context, &*host),
                };
                // The current action list restarts: the subroutine's, in a
                // called subroutine [assumption].
                self.jump(at, if again { 0 } else { next });
                Step::Continue
            }
            actions::IF => {
                let target = if self.evaluate(&context, &*host, &arg(0)).truthy() {
                    next
                } else {
                    self.branch(&context, &*host, frame.rule, frame.pc)
                };
                self.jump(at, target);
                Step::Continue
            }
            // Reached from the end of the branch before: leave the chain.
            actions::ELSE_IF | actions::ELSE => {
                let target = match flow {
                    Flow::Branch { end, .. } | Flow::Else { end } => end + 1,
                    _ => next,
                };
                self.jump(at, target);
                Step::Continue
            }
            actions::WHILE => {
                let target = match (flow, self.evaluate(&context, &*host, &arg(0)).truthy()) {
                    (Flow::Loop { end }, false) => end + 1,
                    _ => next,
                };
                self.jump(at, target);
                Step::Continue
            }
            actions::FOR_GLOBAL_VARIABLE | actions::FOR_PLAYER_VARIABLE => {
                let target = self.for_start(&context, host, &action, flow, next);
                self.jump(at, target);
                Step::Continue
            }
            actions::END => {
                let target = match flow {
                    Flow::End {
                        opener: Some(opener),
                    } => self.loop_end(&context, host, frame.rule, opener, next),
                    _ => next,
                };
                self.jump(at, target);
                Step::Continue
            }
            actions::BREAK | actions::CONTINUE => {
                let target = match flow {
                    Flow::Exit {
                        loop_start: Some(start),
                    } => {
                        let end = match self.rules[frame.rule].flow[start] {
                            Flow::Loop { end } => end,
                            _ => next,
                        };
                        if action.index == actions::BREAK {
                            end + 1
                        } else {
                            // Continue: as if the loop's End was reached.
                            self.loop_end(&context, host, frame.rule, start, end + 1)
                        }
                    }
                    _ => next,
                };
                self.jump(at, target);
                Step::Continue
            }
            actions::CALL_SUBROUTINE => {
                self.jump(at, next);
                if let Some(subroutine) = self.subroutine(&arg(0)) {
                    let frames = &mut self.executions[at].frames;
                    if frames.len() >= CALL_DEPTH {
                        let name = &self.rules[subroutine].name;
                        self.faults
                            .push(format!("subroutine {name:?} called {CALL_DEPTH} deep"));
                        return Step::Done;
                    }
                    frames.push(Frame {
                        rule: subroutine,
                        pc: 0,
                    });
                }
                Step::Continue
            }
            actions::START_RULE => {
                self.jump(at, next);
                if let Some(subroutine) = self.subroutine(&arg(0)) {
                    let key = (subroutine, context.event_player);
                    let restart = matches!(arg(1), Arg::Enum(RESTART_RULE));
                    match self
                        .executions
                        .iter_mut()
                        .find(|execution| execution.key == key)
                    {
                        Some(running) if restart => {
                            running.context = context.clone();
                            running.frames = vec![Frame {
                                rule: subroutine,
                                pc: 0,
                            }];
                            running.block = Block::Ready;
                        }
                        Some(_) => {}
                        None => self.start(key, context.clone()),
                    }
                }
                Step::Continue
            }
            actions::SET_GLOBAL_VARIABLE
            | actions::SET_PLAYER_VARIABLE
            | actions::MODIFY_GLOBAL_VARIABLE
            | actions::MODIFY_PLAYER_VARIABLE
            | actions::SET_GLOBAL_VARIABLE_AT_INDEX
            | actions::SET_PLAYER_VARIABLE_AT_INDEX
            | actions::MODIFY_GLOBAL_VARIABLE_AT_INDEX
            | actions::MODIFY_PLAYER_VARIABLE_AT_INDEX => {
                self.assign(&context, host, &action);
                self.jump(at, next);
                Step::Continue
            }
            index => {
                self.jump(at, next);
                let Some(graph) = self.catalog.action(index).and_then(|entry| entry.graph) else {
                    return Step::Continue;
                };
                let params = self.evaluate_params(&*host, &action, &context);
                let handle = host.start(ActionCall {
                    action: index,
                    graph,
                    params,
                    source: Arc::clone(&action),
                    context,
                });
                if host.finished(handle) {
                    Step::Continue
                } else {
                    self.executions[at].block = Block::Action(handle);
                    Step::Blocked
                }
            }
        }
    }

    fn jump(&mut self, at: usize, pc: usize) {
        if let Some(frame) = self.executions[at].frames.last_mut() {
            frame.pc = pc;
        }
    }

    /// Where an If or Else If whose condition is false leads: the first
    /// later Else If whose condition holds, an Else, or past the End.
    fn branch<H: Host + ?Sized>(
        &mut self,
        context: &Context,
        host: &H,
        rule: usize,
        from: usize,
    ) -> usize {
        let mut at = from;
        loop {
            let (next, end) = match self.rules[rule].flow[at] {
                Flow::Branch { next, end } => (next, end),
                _ => return at + 1,
            };
            if next >= end {
                return end + 1;
            }
            let action = Arc::clone(&self.rules[rule].actions[next]);
            match action.index {
                actions::ELSE => return next + 1,
                actions::ELSE_IF => {
                    let condition = action.args.first().cloned().unwrap_or(Arg::Bool(false));
                    if self.evaluate(context, host, &condition).truthy() {
                        return next + 1;
                    }
                    at = next;
                }
                _ => return end + 1,
            }
        }
    }

    /// The start of a For: set the control variable; past the End when the
    /// range is already done.
    fn for_start<H: Host + ?Sized>(
        &mut self,
        context: &Context,
        host: &mut H,
        action: &Action,
        flow: Flow,
        next: usize,
    ) -> usize {
        let player_loop = action.index == actions::FOR_PLAYER_VARIABLE;
        let offset = usize::from(player_loop);
        let start = self
            .evaluate(context, &*host, &action.args[offset + 1])
            .number();
        let (target, stop, step) = self.for_parts(context, &*host, action);
        let Some(target) = target else { return next };
        self.write(host, target, Value::Number(start));
        match flow {
            Flow::Loop { end } if passed(start, stop, step) => end + 1,
            _ => next,
        }
    }

    /// The End of a While (back to its check) or a For (step the control
    /// variable, then loop or leave); `next` for an If.
    fn loop_end<H: Host + ?Sized>(
        &mut self,
        context: &Context,
        host: &mut H,
        rule: usize,
        opener: usize,
        next: usize,
    ) -> usize {
        let action = Arc::clone(&self.rules[rule].actions[opener]);
        match action.index {
            actions::WHILE => opener,
            actions::FOR_GLOBAL_VARIABLE | actions::FOR_PLAYER_VARIABLE => {
                let (target, stop, step) = self.for_parts(context, &*host, &action);
                let Some(target) = target else { return next };
                let value = self.read(&*host, target).number() + step;
                self.write(host, target, Value::Number(value));
                if passed(value, stop, step) {
                    next
                } else {
                    opener + 1
                }
            }
            _ => next,
        }
    }

    /// A For's control variable, Range Stop and Step (evaluated each time
    /// the loop checks them [assumption]).
    fn for_parts<H: Host + ?Sized>(
        &mut self,
        context: &Context,
        host: &H,
        action: &Action,
    ) -> (Option<Target>, f64, f64) {
        let player_loop = action.index == actions::FOR_PLAYER_VARIABLE;
        let offset = usize::from(player_loop);
        let variable = match action.args.get(offset) {
            Some(Arg::Variable(index)) => *index,
            _ => return (None, 0.0, 0.0),
        };
        let target = if player_loop {
            match self.evaluate(context, host, &action.args[0]).entity() {
                Some(player) => Target::Player(player, variable),
                None => return (None, 0.0, 0.0),
            }
        } else {
            Target::Global(variable)
        };
        let stop = self
            .evaluate(context, host, &action.args[offset + 2])
            .number();
        let step = self
            .evaluate(context, host, &action.args[offset + 3])
            .number();
        (Some(target), stop, step)
    }

    fn subroutine(&self, arg: &Arg) -> Option<usize> {
        let Arg::Subroutine(index) = arg else {
            return None;
        };
        self.rules.iter().position(|rule| {
            rule.event.kind == events::SUBROUTINE && rule.event.slot == i32::from(*index)
        })
    }

    /// Set or Modify Global/Player Variable [At Index].
    fn assign<H: Host + ?Sized>(&mut self, context: &Context, host: &mut H, action: &Action) {
        let player_action = matches!(
            action.index,
            actions::SET_PLAYER_VARIABLE
                | actions::MODIFY_PLAYER_VARIABLE
                | actions::SET_PLAYER_VARIABLE_AT_INDEX
                | actions::MODIFY_PLAYER_VARIABLE_AT_INDEX
        );
        let indexed = matches!(
            action.index,
            actions::SET_GLOBAL_VARIABLE_AT_INDEX
                | actions::SET_PLAYER_VARIABLE_AT_INDEX
                | actions::MODIFY_GLOBAL_VARIABLE_AT_INDEX
                | actions::MODIFY_PLAYER_VARIABLE_AT_INDEX
        );
        let modify = matches!(
            action.index,
            actions::MODIFY_GLOBAL_VARIABLE
                | actions::MODIFY_PLAYER_VARIABLE
                | actions::MODIFY_GLOBAL_VARIABLE_AT_INDEX
                | actions::MODIFY_PLAYER_VARIABLE_AT_INDEX
        );
        let mut args = action.args.iter();
        let players = if player_action {
            match args.next() {
                Some(arg) => self.evaluate(context, &*host, arg).entities(),
                None => return,
            }
        } else {
            Vec::new()
        };
        let Some(Arg::Variable(variable)) = args.next() else {
            return;
        };
        let index = if indexed {
            match args.next() {
                Some(arg) => Some(self.evaluate(context, &*host, arg).number()),
                None => return,
            }
        } else {
            None
        };
        let operation = if modify {
            match args.next() {
                Some(Arg::Enum(operation)) => Some(*operation),
                _ => return,
            }
        } else {
            None
        };
        let Some(operand) = args.next() else { return };
        let operand = self.evaluate(context, &*host, operand);
        let targets: Vec<Target> = if player_action {
            players
                .into_iter()
                .map(|player| Target::Player(player, *variable))
                .collect()
        } else {
            vec![Target::Global(*variable)]
        };
        for target in targets {
            let old = self.read(&*host, target);
            let new = match index {
                None => match operation {
                    Some(operation) => apply(&*host, operation, &old, &operand),
                    None => operand.clone(),
                },
                Some(index) if index >= 0.0 => {
                    let index = index as usize;
                    let mut items = old.array();
                    if items.len() <= index {
                        // Gaps fill with 0 [assumption].
                        items.resize(index + 1, Value::Number(0.0));
                    }
                    items[index] = match operation {
                        Some(operation) => apply(&*host, operation, &items[index], &operand),
                        None => operand.clone(),
                    };
                    Value::Array(items)
                }
                Some(_) => continue,
            };
            self.write(host, target, new);
        }
    }

    fn read<H: Host + ?Sized>(&self, host: &H, target: Target) -> Value {
        match target {
            Target::Global(index) => host.global(index),
            Target::Player(player, index) => host.player(player, index),
        }
    }

    fn write<H: Host + ?Sized>(&self, host: &mut H, target: Target, value: Value) {
        match target {
            Target::Global(index) => host.set_global(index, value),
            Target::Player(player, index) => host.set_player(player, index, value),
        }
    }
}

/// Whether `arg` uses value `id` anywhere.
fn names(arg: &Arg, id: Id) -> bool {
    let value_names =
        |value: &logic::Value| value.id == id || value.args.iter().any(|arg| names(arg, id));
    match arg {
        Arg::Value(value) => value_names(value),
        Arg::Array(values) => values.iter().any(value_names),
        _ => false,
    }
}

#[derive(Clone, Copy)]
enum Target {
    Global(u8),
    Player(EntityId, u8),
}

/// Whether a For's control variable reached or passed Range Stop, in the
/// Step's direction (the For description).
fn passed(value: f64, stop: f64, step: f64) -> bool {
    if step >= 0.0 {
        value >= stop
    } else {
        value <= stop
    }
}

/// A Modify Variable operation (enum `4A28`).
fn apply<H: Host + ?Sized>(host: &H, operation: Id, old: &Value, operand: &Value) -> Value {
    use eval::ids;
    let value = |id| arithmetic(host, id, old, operand);
    match operation {
        modify::ADD => value(ids::ADD),
        modify::SUBTRACT => value(ids::SUBTRACT),
        modify::MULTIPLY => value(ids::MULTIPLY),
        modify::DIVIDE => value(ids::DIVIDE),
        modify::MODULO => value(ids::MODULO),
        modify::RAISE_TO_POWER => value(ids::RAISE_TO_POWER),
        modify::MIN => value(ids::MIN),
        modify::MAX => value(ids::MAX),
        modify::APPEND_TO_ARRAY => {
            let mut items = old.array();
            items.extend(operand.array());
            Value::Array(items)
        }
        modify::REMOVE_FROM_ARRAY_BY_VALUE => {
            let removed = operand.array();
            Value::Array(
                old.array()
                    .into_iter()
                    .filter(|item| !removed.iter().any(|gone| gone.equals(item)))
                    .collect(),
            )
        }
        modify::REMOVE_FROM_ARRAY_BY_INDEX => {
            let mut items = old.array();
            let index = operand.number();
            if index >= 0.0 && (index as usize) < items.len() {
                items.remove(index as usize);
            }
            Value::Array(items)
        }
        _ => old.clone(),
    }
}

/// Whether `player` passes an event's team and player filters.
fn matches_filter<H: Host + ?Sized>(event: &Event, player: EntityId, host: &H) -> bool {
    (event.team == 0 || host.team_of(player) == event.team)
        && (event.slot < 0 || host.slot_of(player) == event.slot)
        && (event.hero == 0 || host.hero_of(player) == Some(event.hero))
}

fn joined_or_left(kind: u8, player: EntityId) -> GameEvent {
    GameEvent {
        kind,
        context: Context {
            event_player: Some(player),
            ..Context::default()
        },
    }
}
