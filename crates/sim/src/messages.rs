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
//! runs takes no place. A message an entity sends while it takes one goes
//! the next tick (the client's receivers pick a send up at their own next
//! update: the same frame or the next).

use orpheus_script::{
    mode::EVERY_ENTITY,
    vm::{GameMessage, MessageEnd},
};

use crate::{
    bot_script::BotScripts, mode_script::ModeRunner, placed_script::PlacedScripts,
    replication::WorldDelivery, spawned::SpawnedFields,
};

/// The room's messages per tick (match.py `MAX_BODY_MESSAGES`).
pub const MAX_MESSAGES: usize = 256;

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
}

/// Hand the room's `messages`, then its message `ends`, to the entities
/// they name (see the module notes).
pub fn deliver(
    mut messages: Vec<GameMessage>,
    ends: Vec<MessageEnd>,
    recipients: &mut [&mut dyn Recipients],
) {
    messages.retain(|message| {
        message.target == EVERY_ENTITY || recipients.iter().any(|group| group.runs(message.target))
    });
    if messages.len() > MAX_MESSAGES {
        eprintln!(
            "match message queue reached {MAX_MESSAGES}; {} dropped",
            messages.len() - MAX_MESSAGES
        );
    }
    for message in messages.iter().take(MAX_MESSAGES) {
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
    for end in &ends {
        if let Some(group) = recipients.iter_mut().find(|group| group.runs(end.target)) {
            group.end(end);
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
}

impl Recipients for SpawnedFields {
    fn runs(&self, entity: u32) -> bool {
        SpawnedFields::runs(self, entity)
    }

    fn receive(&mut self, message: &GameMessage) {
        self.receive_message(message);
    }

    fn end(&mut self, end: &MessageEnd) {
        self.end_message(end);
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
    }

    fn mail(sender: u32, target: u32, message: u64) -> GameMessage {
        GameMessage {
            sender,
            target,
            message,
            parameters: Variables::new(),
        }
    }

    /// A message to every entity reaches each entity of every group once,
    /// but its sender; one to an entity reaches it alone; one to an entity
    /// no graph runs goes nowhere and takes no place in the tick's
    /// MAX_MESSAGES, of which the first go, in order. Ends go to their
    /// entity.
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
        deliver(messages, ends, &mut [&mut players, &mut bots, &mut mode]);
        assert_eq!(players.taken, [(1, 7), (2, 7), (2, 8)]);
        assert_eq!(mode.taken, [(0x20, 8)]);
        let to_bots: Vec<_> = [(10, 7), (11, 7), (10, 8), (11, 8)]
            .into_iter()
            .chain((0..MAX_MESSAGES as u64 - 2).map(|message| (11, message)))
            .collect();
        assert_eq!(bots.taken, to_bots);
        assert_eq!(players.ended, [(2, 9)]);
        assert!(bots.ended.is_empty() && mode.ended.is_empty());
    }
}
