//! The game messages between the entities that run graphs on the server,
//! one path for every sender (match.py `queue_body_message`,
//! `deliver_body_messages`): the players' and AI heroes' bodies and
//! controllers, the mode graph (and the match flow in its place), the map's
//! placed scripts and the Biotic Fields send; those and the Training Bots
//! take.
//!
//! A message names one entity, or every entity
//! ([`orpheus_script::mode::EVERY_ENTITY`]: a `STU_1D719351` target with its
//! primary channel, `STU_57692020`). One to every entity reaches each
//! entity that runs graphs once, but its sender: the sender's own instances
//! took it from the VM when it was sent. The client works so (research
//! report `GameMessages.md`): a send to every entity registers under the
//! "no entity" id (`sub_7FF789C5C3E0`, the `STU_38EE1100` target dispatch,
//! into `sub_7FF789CAA770`), and each receiving entry looks its message up
//! under its own entity, then under that id (`sub_7FF789CAAD40` from
//! `sub_7FF789C603A0`), so every entity's entries find it, the sender's
//! own among them, subject to their own filters.
//!
//! The room routes what its entities sent since the last tick once a tick
//! ([`deliver`]), at most [`MAX_MESSAGES`]; a message to an entity no graph
//! runs takes no place. The messages over that wait, in order, for the next
//! tick ([`Backlog`]): the first tick of a Workshop match on Hanamura has
//! `0F4A` send 286 to the map's tagged placeables before the objectives'
//! `0F4C` report (`0813`), which a drop lost, so its round never counted
//! down (`refs/reports/Workshop.md` "ParkourMatch7"). A message an entity
//! sends while it takes one goes the next tick (the client's receivers pick
//! a send up at their own next update: the same frame or the next).

use orpheus_script::{
    mode::EVERY_ENTITY,
    vm::{GameMessage, MessageEnd},
};

use crate::{
    bot_script::BotScripts, mode_script::ModeRunner, placed_script::PlacedScripts,
    replication::WorldDelivery, spawned::SpawnedEntities,
};

/// The room's messages per tick (match.py `MAX_BODY_MESSAGES`).
pub const MAX_MESSAGES: usize = 256;

/// What a tick's [`deliver`] left for the next: the messages over its
/// [`MAX_MESSAGES`], and the tick's ends behind them (an end never goes
/// before the message it ends).
#[derive(Default)]
pub struct Backlog {
    messages: Vec<GameMessage>,
    ends: Vec<MessageEnd>,
}

/// Some of the room's entities that run graphs.
pub trait Recipients {
    /// Whether these run the graphs of `entity` (never [`EVERY_ENTITY`]).
    fn runs(&self, entity: u32) -> bool;
    /// `message` to the entity it names, one of these, or to every entity:
    /// each of these but its sender takes it.
    fn receive(&mut self, message: &GameMessage);
    /// A persistent sender let its message to `end.target`, one of these,
    /// go.
    fn end(&mut self, end: &MessageEnd);
    /// A persistent sender that `update.target`, one of these, holds
    /// changed its parameters; these route none by default.
    fn update(&mut self, _update: &GameMessage) {}
    /// The entities whose graphs these run now.
    fn entities(&self) -> Vec<u32>;
}

/// Persistent senders' new parameters to the entities that hold their
/// messages (`orpheus_script` `World::update_message`), after the tick's
/// messages ([`deliver`]).
pub fn deliver_updates(updates: &[GameMessage], recipients: &mut [&mut dyn Recipients]) {
    for update in updates {
        if let Some(group) = recipients
            .iter_mut()
            .find(|group| group.runs(update.target))
        {
            group.update(update);
        }
    }
}

/// The entities each held broadcast reached ([`deliver_held`]).
#[derive(Default)]
pub struct HeldReach {
    /// By sender and message: the entities holding it now.
    reached: Vec<((u32, u64), Vec<u32>)>,
}

/// The messages held to every entity now (`held`: a mode graph's
/// SendGameMessage states, `orpheus_script::mode::ModeScript::held`):
/// each goes once to every entity that runs graphs and hasn't had it,
/// but its sender, as the client's receivers find a live sender at their
/// next look-up whenever they start (GameMessages.md). An entity that
/// went away is forgotten, so one that starts again under its id takes it
/// again. A message no longer held ends for the entities it reached.
pub fn deliver_held(
    held: &[GameMessage],
    reach: &mut HeldReach,
    recipients: &mut [&mut dyn Recipients],
) {
    let key = |message: &GameMessage| (message.sender, message.message);
    let (kept, gone): (Vec<_>, Vec<_>) = std::mem::take(&mut reach.reached)
        .into_iter()
        .partition(|(sent, _)| held.iter().any(|message| key(message) == *sent));
    for ((sender, message), entities) in gone {
        for target in entities {
            let end = MessageEnd {
                sender,
                target,
                message,
            };
            if let Some(group) = recipients.iter_mut().find(|group| group.runs(target)) {
                group.end(&end);
            }
        }
    }
    reach.reached = kept;
    let present: Vec<u32> = recipients
        .iter()
        .flat_map(|group| group.entities())
        .collect();
    for message in held {
        let index = match reach
            .reached
            .iter()
            .position(|(sent, _)| *sent == key(message))
        {
            Some(index) => index,
            None => {
                reach.reached.push((key(message), Vec::new()));
                reach.reached.len() - 1
            }
        };
        reach.reached[index]
            .1
            .retain(|entity| present.contains(entity));
        for &entity in &present {
            if entity == message.sender || reach.reached[index].1.contains(&entity) {
                continue;
            }
            let to = GameMessage {
                target: entity,
                ..message.clone()
            };
            if let Some(group) = recipients.iter_mut().find(|group| group.runs(entity)) {
                group.receive(&to);
            }
            reach.reached[index].1.push(entity);
        }
    }
}

/// Hand the `backlog` and the room's `messages`, then its message `ends`,
/// to the entities they name (see the module notes); what goes over
/// [`MAX_MESSAGES`] stays in the `backlog` for the next tick.
pub fn deliver(
    backlog: &mut Backlog,
    messages: Vec<GameMessage>,
    ends: Vec<MessageEnd>,
    recipients: &mut [&mut dyn Recipients],
) {
    let mut queue = std::mem::take(&mut backlog.messages);
    queue.extend(messages);
    queue.retain(|message| {
        message.target == EVERY_ENTITY || recipients.iter().any(|group| group.runs(message.target))
    });
    // A persistent send that ended this tick while another of the same
    // sender, target and message waits to be delivered (`0F4A` holds `079E`
    // to its objective in setup, and again from the round's state as the
    // setup's ends) ends first, so the receiver takes the new one, even when
    // the new one waits past the cap. [Assumption: a send that began and
    // ended within one tick while another like it began is not met.]
    let (first, later): (Vec<MessageEnd>, Vec<MessageEnd>) = ends.into_iter().partition(|end| {
        queue.iter().any(|message| {
            (message.sender, message.target, message.message)
                == (end.sender, end.target, end.message)
        })
    });
    for end in first {
        if let Some(group) = recipients.iter_mut().find(|group| group.runs(end.target)) {
            group.end(&end);
        }
    }
    backlog.ends.extend(later);
    if queue.len() > MAX_MESSAGES {
        backlog.messages = queue.split_off(MAX_MESSAGES);
        eprintln!(
            "match message queue reached {MAX_MESSAGES}; {} wait for the next tick",
            backlog.messages.len()
        );
    }
    for message in &queue {
        if message.target == EVERY_ENTITY {
            for group in recipients.iter_mut() {
                group.receive(message);
            }
        } else if let Some(group) = recipients
            .iter_mut()
            .find(|group| group.runs(message.target))
        {
            group.receive(message);
        }
    }
    if !backlog.messages.is_empty() {
        return;
    }
    for end in std::mem::take(&mut backlog.ends) {
        if let Some(group) = recipients.iter_mut().find(|group| group.runs(end.target)) {
            group.end(&end);
        }
    }
}

/// The room's players (AI heroes too): each one's current body and
/// controller.
pub struct Players<'a, 'w> {
    worlds: &'a mut [&'w mut WorldDelivery],
    /// The room's controllers and teams, as the controller graph's filters
    /// know them.
    roster: Vec<(u32, i64)>,
    tick: u32,
}

impl<'a, 'w> Players<'a, 'w> {
    pub fn new(worlds: &'a mut [&'w mut WorldDelivery], tick: u32) -> Self {
        let roster = worlds
            .iter()
            .filter_map(|world| world.controller_team())
            .collect();
        Self {
            worlds,
            roster,
            tick,
        }
    }

    /// What the players' graphs sent since the last call.
    pub fn take_messages(&mut self) -> Vec<GameMessage> {
        self.worlds
            .iter_mut()
            .flat_map(|world| world.take_messages())
            .collect()
    }
}

impl Recipients for Players<'_, '_> {
    fn runs(&self, entity: u32) -> bool {
        self.worlds.iter().any(|world| world.runs(entity))
    }

    fn receive(&mut self, message: &GameMessage) {
        for world in self.worlds.iter_mut() {
            if message.target == EVERY_ENTITY || world.runs(message.target) {
                world.receive_message(self.tick, &self.roster, message);
            }
        }
    }

    fn end(&mut self, end: &MessageEnd) {
        if let Some(world) = self.worlds.iter_mut().find(|world| world.runs(end.target)) {
            world.end_message(end);
        }
    }

    fn entities(&self) -> Vec<u32> {
        self.worlds
            .iter()
            .flat_map(|world| world.script_entities())
            .collect()
    }
}

/// The Training Bots' body scripts at world tick `tick`.
pub struct Bots<'a> {
    pub scripts: &'a mut BotScripts,
    pub tick: u32,
}

impl Recipients for Bots<'_> {
    fn runs(&self, entity: u32) -> bool {
        self.scripts.runs(entity)
    }

    fn receive(&mut self, message: &GameMessage) {
        self.scripts.receive_message(message, self.tick);
    }

    fn end(&mut self, end: &MessageEnd) {
        self.scripts.end_message(end);
    }

    fn entities(&self) -> Vec<u32> {
        self.scripts.entities()
    }
}

impl Recipients for PlacedScripts {
    fn runs(&self, entity: u32) -> bool {
        PlacedScripts::runs(self, entity)
    }

    fn receive(&mut self, message: &GameMessage) {
        self.receive_message(message);
    }

    fn end(&mut self, end: &MessageEnd) {
        self.end_message(end);
    }

    fn update(&mut self, update: &GameMessage) {
        self.update_message(update);
    }

    fn entities(&self) -> Vec<u32> {
        self.world().map_or_else(Vec::new, |world| {
            world
                .entities()
                .iter()
                .filter(|placed| placed.running())
                .map(|placed| placed.entity)
                .collect()
        })
    }
}

impl Recipients for SpawnedEntities {
    fn runs(&self, entity: u32) -> bool {
        SpawnedEntities::runs(self, entity)
    }

    fn receive(&mut self, message: &GameMessage) {
        self.receive_message(message);
    }

    fn end(&mut self, end: &MessageEnd) {
        self.end_message(end);
    }

    fn entities(&self) -> Vec<u32> {
        SpawnedEntities::entities(self)
    }
}

impl Recipients for ModeRunner {
    fn runs(&self, entity: u32) -> bool {
        ModeRunner::runs(self, entity)
    }

    fn receive(&mut self, message: &GameMessage) {
        self.receive_message(message);
    }

    fn end(&mut self, end: &MessageEnd) {
        self.end_message(end);
    }

    /// The mode entity's own instances take its broadcasts from the VM.
    fn entities(&self) -> Vec<u32> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use orpheus_script::wire::Variables;

    use super::*;

    /// Entities that keep what they took, by entity.
    struct Group {
        entities: BTreeSet<u32>,
        taken: Vec<(u32, u64)>,
        ended: Vec<(u32, u64)>,
    }

    impl Group {
        fn new(entities: &[u32]) -> Self {
            Self {
                entities: entities.iter().copied().collect(),
                taken: Vec::new(),
                ended: Vec::new(),
            }
        }
    }

    impl Recipients for Group {
        fn runs(&self, entity: u32) -> bool {
            self.entities.contains(&entity)
        }

        fn receive(&mut self, message: &GameMessage) {
            for &entity in &self.entities {
                let named = if message.target == EVERY_ENTITY {
                    entity != message.sender
                } else {
                    entity == message.target
                };
                if named {
                    self.taken.push((entity, message.message));
                }
            }
        }

        fn end(&mut self, end: &MessageEnd) {
            self.ended.push((end.target, end.message));
        }

        fn entities(&self) -> Vec<u32> {
            self.entities.iter().copied().collect()
        }
    }

    fn mail(sender: u32, target: u32, message: u64) -> GameMessage {
        GameMessage {
            sender,
            target,
            message,
            parameters: Variables::new(),
        }
    }

    /// A held broadcast reaches every entity once, entities that start later
    /// too, but not its sender; one that went away takes it again when it
    /// comes back; when it is no longer held it ends for those it reached.
    #[test]
    fn a_held_broadcast_reaches_each_entity_once_and_later_ones_too() {
        let held = [mail(0x20, EVERY_ENTITY, 7)];
        let mut reach = HeldReach::default();
        let mut players = Group::new(&[1]);
        let mut mode = Group::new(&[0x20]);
        deliver_held(&held, &mut reach, &mut [&mut players, &mut mode]);
        deliver_held(&held, &mut reach, &mut [&mut players, &mut mode]);
        assert_eq!(players.taken, [(1, 7)]);
        assert!(mode.taken.is_empty());
        players.entities.insert(2);
        deliver_held(&held, &mut reach, &mut [&mut players, &mut mode]);
        assert_eq!(players.taken, [(1, 7), (2, 7)]);
        players.entities.remove(&2);
        deliver_held(&held, &mut reach, &mut [&mut players, &mut mode]);
        players.entities.insert(2);
        deliver_held(&held, &mut reach, &mut [&mut players, &mut mode]);
        assert_eq!(players.taken, [(1, 7), (2, 7), (2, 7)]);
        deliver_held(&[], &mut reach, &mut [&mut players, &mut mode]);
        assert_eq!(players.ended, [(1, 7), (2, 7)]);
    }

    /// A message to every entity reaches each entity of every group once,
    /// but its sender; one to an entity reaches it alone; one to an entity
    /// no graph runs goes nowhere and takes no place in the tick's
    /// MAX_MESSAGES, of which the first go, in order. The rest go first the
    /// next tick, and the tick's ends after them, each to its entity.
    #[test]
    fn messages_reach_the_named_entity_or_every_entity_but_the_sender() {
        const NOBODY: u32 = 0x99;
        let mut players = Group::new(&[1, 2]);
        let mut bots = Group::new(&[10, 11]);
        let mut mode = Group::new(&[0x20]);
        let mut messages = vec![mail(0x20, EVERY_ENTITY, 7), mail(1, EVERY_ENTITY, 8)];
        for message in 0..MAX_MESSAGES as u64 {
            messages.push(mail(1, NOBODY, 1000 + message));
            messages.push(mail(0x20, 11, message));
        }
        let ends = vec![MessageEnd {
            sender: 0x20,
            target: 2,
            message: 9,
        }];
        let mut backlog = Backlog::default();
        deliver(
            &mut backlog,
            messages,
            ends,
            &mut [&mut players, &mut bots, &mut mode],
        );
        assert_eq!(players.taken, [(1, 7), (2, 7), (2, 8)]);
        assert_eq!(mode.taken, [(0x20, 8)]);
        let mut to_bots: Vec<_> = [(10, 7), (11, 7), (10, 8), (11, 8)]
            .into_iter()
            .chain((0..MAX_MESSAGES as u64 - 2).map(|message| (11, message)))
            .collect();
        assert_eq!(bots.taken, to_bots);
        assert!(players.ended.is_empty(), "the end waits for its message");
        // The next tick: the two left over, then this tick's own.
        deliver(
            &mut backlog,
            vec![mail(0x20, 10, 99)],
            Vec::new(),
            &mut [&mut players, &mut bots, &mut mode],
        );
        to_bots.extend([
            (11, MAX_MESSAGES as u64 - 2),
            (11, MAX_MESSAGES as u64 - 1),
            (10, 99),
        ]);
        assert_eq!(bots.taken, to_bots);
        assert_eq!(players.ended, [(2, 9)]);
        assert!(bots.ended.is_empty() && mode.ended.is_empty());
    }
}
