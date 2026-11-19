//! Clock and pending-event ordering for authored statescripts.
//!
//! Client research (1.74): time is int64 milliseconds. Command frame K runs
//! as the window [16K, 16K + 15]: an update of every instance at each time an
//! event is due, and one at the window's end (component update
//! 0x7FF78AA9C910, step 0x7FF78AA9DE30). An update drains the instance's
//! events up to now, ticks the ticking states and runs the deferred list,
//! until the deferred list is empty (0x7FF78AAA27C0). Events (dispatcher
//! 0x7FF78AAA1690): 0 a state op (begin, finish, abort), 1 a timer, 2 a
//! deferred completion check, 3 an Entry, 8 a button edge, 9 "a watched
//! variable changed". A posted event goes after every event of the same time
//! (FIFO), never in the past (0x7FF78AAA2990); a state's queued events are
//! dropped at 0x7FF78AAA12F0, begin requests queued at now at 0x7FF78AAA10F0.
//! Init (0x7FF78AAA3F00) queues every server-run Entry at the start time.
//! Button edges go to every instance (0x7FF789C66AA0), one per changed
//! command bit, in the client's order: bit -> logical button 0x40 11, 0x80
//! 12, 0x01 20, 0x02 29, 0x08 35, 0x10 36, 0x20 37, 0x04 30, 0x800 31, 0x400
//! 14, then the second byte 0x01 111, 0x02 112, 0x04 113, 0x08 114, 0x10 110,
//! 0x20 116, 0x40 115; an action byte presses and releases its button, by
//! the client's 46-entry table `0x7FF78BC278F0` (ClientInput.md): code 42
//! is logical 81, the hero select key (H), which the Training Partner
//! console `265F` takes too.

use std::collections::{BTreeMap, VecDeque};

pub const FRAME_MS: u64 = orpheus_core::tick::TICK_PERIOD.as_millis() as u64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ButtonEdge {
    pub button: u8,
    pub pressed: bool,
}

/// Logical edges in the client dispatcher order. Action bytes are pulses.
pub fn button_edges(previous: u32, current: u32, action: u8) -> impl Iterator<Item = ButtonEdge> {
    const INPUTS: [(u32, u8); 17] = [
        (0x040, 11),
        (0x080, 12),
        (0x001, 20),
        (0x002, 29),
        (0x008, 35),
        (0x010, 36),
        (0x020, 37),
        (0x004, 30),
        (0x800, 31),
        (0x400, 14),
        (0x001, 111),
        (0x002, 112),
        (0x004, 113),
        (0x008, 114),
        (0x010, 110),
        (0x020, 116),
        (0x040, 115),
    ];
    let pulse = match action {
        1..=5 => Some(14 + action),
        6..=7 => Some(15 + action),
        8 => Some(56),
        9 => Some(71),
        10..=15 => Some(81 + action),
        16 => Some(120),
        17..=28 => Some(80 + action),
        29..=38 => Some(104 + action),
        39 => Some(117),
        40 => Some(118),
        41 => Some(109),
        42 => Some(81),
        43 => Some(90),
        44 => Some(2),
        45 => Some(3),
        _ => None,
    };
    INPUTS
        .into_iter()
        .filter_map(move |(mask, button)| {
            ((previous ^ current) & mask != 0).then_some(ButtonEdge {
                button,
                pressed: current & mask != 0,
            })
        })
        .chain(pulse.into_iter().flat_map(|button| {
            [
                ButtonEdge {
                    button,
                    pressed: true,
                },
                ButtonEdge {
                    button,
                    pressed: false,
                },
            ]
        }))
}

pub(crate) fn frame_start(frame: u32) -> u64 {
    u64::from(frame) * FRAME_MS
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct QueueFull {
    pub limit: usize,
}

/// Each instance consumes events in timestamp order, with FIFO ties. The
/// component controls the order in which instances take a turn.
pub(crate) struct Timeline<E> {
    instances: BTreeMap<u16, BTreeMap<u64, VecDeque<E>>>,
    pending: usize,
    limit: usize,
}

impl<E> Timeline<E> {
    pub fn new(limit: usize) -> Self {
        Self {
            instances: BTreeMap::new(),
            pending: 0,
            limit,
        }
    }

    pub fn reserve(&self, count: usize) -> Result<(), QueueFull> {
        if count > self.limit - self.pending {
            Err(QueueFull { limit: self.limit })
        } else {
            Ok(())
        }
    }

    pub fn push(&mut self, instance: u16, due_ms: u64, event: E) -> Result<(), QueueFull> {
        self.reserve(1)?;
        self.instances
            .entry(instance)
            .or_default()
            .entry(due_ms)
            .or_default()
            .push_back(event);
        self.pending += 1;
        Ok(())
    }

    pub fn next_due(&self) -> Option<u64> {
        self.instances
            .values()
            .filter_map(|events| events.first_key_value().map(|(&due_ms, _)| due_ms))
            .min()
    }

    pub fn pop_due(&mut self, instance: u16, now_ms: u64) -> Option<E> {
        let events = self.instances.get_mut(&instance)?;
        let mut next = events.first_entry()?;
        if *next.key() > now_ms {
            return None;
        }
        let event = next.get_mut().pop_front().expect("populated timestamp");
        if next.get().is_empty() {
            next.remove();
        }
        if events.is_empty() {
            self.instances.remove(&instance);
        }
        self.pending -= 1;
        Some(event)
    }

    pub fn retain(&mut self, instance: u16, mut keep: impl FnMut(u64, &E) -> bool) {
        if let Some(events) = self.instances.get_mut(&instance) {
            events.retain(|&due_ms, queued| {
                let before = queued.len();
                queued.retain(|event| keep(due_ms, event));
                self.pending -= before - queued.len();
                !queued.is_empty()
            });
            if events.is_empty() {
                self.instances.remove(&instance);
            }
        }
    }

    pub fn remove(&mut self, instance: u16) {
        if let Some(events) = self.instances.remove(&instance) {
            self.pending -= events.values().map(VecDeque::len).sum::<usize>();
        }
    }

    pub fn iter(&self, instance: u16) -> impl Iterator<Item = (u64, &E)> {
        self.instances
            .get(&instance)
            .into_iter()
            .flatten()
            .flat_map(|(&due_ms, events)| events.iter().map(move |event| (due_ms, event)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_edges_preserve_aliases_release_order_and_action_pulses() {
        let collect = |previous, current, action| {
            button_edges(previous, current, action)
                .map(|edge| (edge.button, edge.pressed))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            collect(0x88, 0x841, 2),
            [
                (11, true),
                (12, false),
                (20, true),
                (35, false),
                (31, true),
                (111, true),
                (114, false),
                (115, true),
                (16, true),
                (16, false)
            ]
        );
        assert_eq!(collect(8, 8, 19), [(99, true), (99, false)]);
        // The client's table `0x7FF78BC278F0` (ClientInput.md): 20 is 100,
        // 42 the hero select key 81; it has 46 entries.
        assert_eq!(collect(0x004000, 0, 20), [(100, true), (100, false)]);
        assert_eq!(collect(0, 0, 42), [(81, true), (81, false)]);
        assert!(collect(0x004000, 0, 46).is_empty());
    }

    #[test]
    fn preserves_due_time_fifo_and_instance_turns() {
        let mut timeline = Timeline::new(6);
        for (instance, time, event) in [
            (2, 18, 'a'),
            (1, 17, 'b'),
            (2, 17, 'c'),
            (2, 17, 'd'),
            (1, 16, 'e'),
        ] {
            timeline.push(instance, time, event).unwrap();
        }
        assert_eq!(timeline.next_due(), Some(16));
        assert_eq!(timeline.pop_due(2, 16), None);
        assert_eq!(timeline.pop_due(1, 16), Some('e'));
        assert_eq!(timeline.pop_due(2, 17), Some('c'));
        timeline.push(2, 17, 'f').unwrap();
        assert_eq!(timeline.pop_due(2, 17), Some('d'));
        assert_eq!(timeline.pop_due(2, 17), Some('f'));
        assert_eq!(timeline.pop_due(2, 17), None);
        assert_eq!(timeline.pop_due(1, 17), Some('b'));
        assert_eq!(timeline.next_due(), Some(18));
        assert_eq!(timeline.pop_due(2, 18), Some('a'));
        assert_eq!(timeline.next_due(), None);
        assert_eq!(frame_start(u32::MAX), u64::from(u32::MAX) * 16);
    }

    #[test]
    fn bounded_pending_events_and_cancellation_reclaim_slots() {
        let mut timeline = Timeline::new(4);
        for (id, time, event) in [(7, 5, 42), (7, 6, 42), (7, 6, 43), (9, 5, 99)] {
            timeline.push(id, time, event).unwrap();
        }
        assert_eq!(timeline.push(9, 5, 100), Err(QueueFull { limit: 4 }));
        timeline.retain(7, |time, event| time != 5 || *event != 42);
        assert_eq!(
            timeline
                .iter(7)
                .map(|(time, event)| (time, *event))
                .collect::<Vec<_>>(),
            [(6, 42), (6, 43)]
        );
        assert_eq!(timeline.reserve(2), Err(QueueFull { limit: 4 }));
        timeline.remove(9);
        timeline.reserve(2).unwrap();
        timeline.push(7, 7, 44).unwrap();
        timeline.push(7, 8, 45).unwrap();
        assert_eq!(timeline.next_due(), Some(6));
        assert_eq!(timeline.pop_due(9, 8), None);
    }
}
