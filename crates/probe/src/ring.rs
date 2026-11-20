//! The probe's ring buffer: what the frame sampler and the hooks recorded,
//! numbered, for `fetch` to hand out in order.

use std::collections::VecDeque;

use serde_json::{Value, json};

/// What the frame sampler saw at one input sample.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameSample {
    /// Input samples since the probe hooked the sampler.
    pub index: u64,
    /// The newest command frame in component 59's command ring (the
    /// sample's own `+0x1038` is still 0 at the sample site).
    pub frame: u32,
    pub comp59: u64,
    pub entity: u64,
    pub entity_id: u32,
    pub comp4: u64,
    /// Component 4 `+0x160`: about 1 m above the feet.
    pub position: Option<[f32; 3]>,
    /// The position change since the previous sample, per second.
    pub velocity: Option<[f32; 3]>,
    /// The stored view (`+0x107A`, `+0x107C`), 16-bit angles.
    pub view: [i16; 2],
    /// What the sample carries after the probe's input.
    pub buttons: u32,
    pub forward: i8,
    pub right: i8,
    pub action: u8,
    /// The sampler's extra reads, in the order they were set.
    pub reads: Vec<Value>,
}

impl FrameSample {
    pub fn json(&self) -> Value {
        json!({
            "index": self.index,
            "frame": self.frame,
            "comp59": self.comp59,
            "entity": self.entity,
            "entity_id": self.entity_id,
            "comp4": self.comp4,
            "position": self.position,
            "velocity": self.velocity,
            "view": self.view,
            "buttons": self.buttons,
            "forward": self.forward,
            "right": self.right,
            "action": self.action,
            "reads": self.reads,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Frame(Box<FrameSample>),
    /// A hook hit: the registers at the hook (rax..r15, rflags), the return
    /// address at a function entry, the reads and the unwound callers.
    Hit {
        hook: u32,
        address: u64,
        registers: Box<[u64; 17]>,
        caller: Option<u64>,
        /// The low 64 bits of xmm0-xmm3 (float arguments).
        xmm: [u64; 4],
        reads: Vec<Value>,
        stack: Vec<u64>,
    },
    /// A hooked function returned: `rax`, `xmm0` (as a double and its low
    /// float) and the call's return address.
    Return {
        hook: u32,
        rax: u64,
        xmm0: u64,
        caller: u64,
        reads: Vec<Value>,
    },
    Text(String),
}

const REGISTER_NAMES: [&str; 17] = [
    "rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi", "r8", "r9", "r10", "r11", "r12", "r13",
    "r14", "r15", "rflags",
];

impl Event {
    fn kind(&self) -> &'static str {
        match self {
            Self::Frame(_) => "frame",
            Self::Hit { .. } => "hit",
            Self::Return { .. } => "return",
            Self::Text(_) => "text",
        }
    }

    fn hook(&self) -> Option<u32> {
        match self {
            Self::Hit { hook, .. } | Self::Return { hook, .. } => Some(*hook),
            _ => None,
        }
    }

    fn json(&self) -> Value {
        match self {
            Self::Frame(sample) => sample.json(),
            Self::Hit {
                hook,
                address,
                registers,
                caller,
                xmm,
                reads,
                stack,
            } => {
                let registers: serde_json::Map<String, Value> = REGISTER_NAMES
                    .iter()
                    .zip(registers.iter())
                    .map(|(name, value)| ((*name).to_owned(), (*value).into()))
                    .collect();
                json!({
                    "hook": hook, "address": address, "registers": registers, "caller": caller,
                    "xmm": xmm,
                    "xmm_f32": xmm.iter().map(|&v| finite(f64::from(f32::from_bits(v as u32)))).collect::<Vec<_>>(),
                    "reads": reads, "stack": stack,
                })
            }
            Self::Return {
                hook,
                rax,
                xmm0,
                caller,
                reads,
            } => json!({
                "hook": hook, "rax": rax, "xmm0": xmm0,
                "xmm0_f32": finite(f64::from(f32::from_bits(*xmm0 as u32))),
                "xmm0_f64": finite(f64::from_bits(*xmm0)),
                "caller": caller, "reads": reads,
            }),
            Self::Text(text) => json!({ "text": text }),
        }
    }
}

fn finite(value: f64) -> Value {
    if value.is_finite() {
        value.into()
    } else {
        format!("{value}").into()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub seq: u64,
    /// Microseconds since the probe started.
    pub time_us: u64,
    pub thread: u32,
    pub event: Event,
}

impl Record {
    pub fn json(&self) -> Value {
        let mut value = self.event.json();
        if let Value::Object(map) = &mut value {
            map.insert("seq".into(), self.seq.into());
            map.insert("t".into(), (self.time_us as f64 / 1e6).into());
            map.insert("thread".into(), self.thread.into());
            map.insert("kind".into(), self.event.kind().into());
        }
        value
    }
}

/// Which records a fetch wants.
#[derive(Clone, Debug, Default)]
pub struct Filter {
    /// Record kinds (`frame`, `hit`, `return`, `text`); empty = all.
    pub kinds: Vec<String>,
    /// Hook ids; empty = all.
    pub hooks: Vec<u32>,
}

impl Filter {
    fn wants(&self, record: &Record) -> bool {
        (self.kinds.is_empty() || self.kinds.iter().any(|kind| kind == record.event.kind()))
            && (self.hooks.is_empty()
                || record
                    .event
                    .hook()
                    .is_none_or(|hook| self.hooks.contains(&hook)))
    }
}

/// The newest `capacity` records.
#[derive(Debug)]
pub struct Ring {
    records: VecDeque<Record>,
    capacity: usize,
    next: u64,
}

/// One fetch: the records after `since`, the sequence number to ask from
/// next, and how many records after `since` the ring had already dropped.
#[derive(Debug, PartialEq)]
pub struct Fetched {
    pub records: Vec<Value>,
    pub next: u64,
    pub lost: u64,
}

impl Ring {
    pub fn new(capacity: usize) -> Self {
        Self {
            records: VecDeque::with_capacity(capacity.min(1 << 16)),
            capacity: capacity.max(1),
            next: 1,
        }
    }

    pub fn push(&mut self, time_us: u64, thread: u32, event: Event) -> u64 {
        if self.records.len() == self.capacity {
            self.records.pop_front();
        }
        let seq = self.next;
        self.next += 1;
        self.records.push_back(Record {
            seq,
            time_us,
            thread,
            event,
        });
        seq
    }

    /// The last sequence number handed out (0 before the first record).
    pub fn last(&self) -> u64 {
        self.next - 1
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn fetch(&self, since: u64, max: usize, filter: &Filter) -> Fetched {
        let first = self.records.front().map_or(self.next, |record| record.seq);
        let lost = first.saturating_sub(since + 1);
        let start = since.saturating_add(1).max(first);
        let skip = (start - first) as usize;
        let mut records = Vec::new();
        let mut next = since.max(first.saturating_sub(1));
        for record in self.records.iter().skip(skip) {
            if records.len() >= max {
                break;
            }
            next = record.seq;
            if filter.wants(record) {
                records.push(record.json());
            }
        }
        Fetched {
            records,
            next,
            lost,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_ring_drops_the_oldest_and_a_fetch_counts_them() {
        let mut ring = Ring::new(3);
        for index in 0..5 {
            ring.push(index, 7, Event::Text(format!("r{index}")));
        }
        let all = ring.fetch(0, 10, &Filter::default());
        assert_eq!(all.lost, 2);
        assert_eq!(all.next, 5);
        let texts: Vec<&str> = all
            .records
            .iter()
            .map(|r| r["text"].as_str().unwrap())
            .collect();
        assert_eq!(texts, ["r2", "r3", "r4"]);
        let rest = ring.fetch(3, 1, &Filter::default());
        assert_eq!((rest.records.len(), rest.next, rest.lost), (1, 4, 0));
        assert!(ring.fetch(5, 10, &Filter::default()).records.is_empty());
        assert_eq!(ring.fetch(5, 10, &Filter::default()).next, 5);
    }

    #[test]
    fn a_filtered_fetch_still_moves_past_what_it_skips() {
        let mut ring = Ring::new(10);
        ring.push(0, 1, Event::Text("a".into()));
        ring.push(
            0,
            1,
            Event::Return {
                hook: 2,
                rax: 0,
                xmm0: 0,
                caller: 0,
                reads: Vec::new(),
            },
        );
        ring.push(0, 1, Event::Text("b".into()));
        let filter = Filter {
            kinds: vec!["return".into()],
            hooks: vec![2],
        };
        let fetched = ring.fetch(0, 10, &filter);
        assert_eq!(fetched.records.len(), 1);
        assert_eq!(fetched.next, 3);
    }
}
