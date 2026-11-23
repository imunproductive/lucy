//! An evaluation kept with what it read ([`Reads`]): evaluating the same
//! value again in the same context gives the same result while every read
//! gives the same answer again, since a value reads nothing else (the
//! catalog and the settings don't change; a value that reads the clock or
//! draws a random number is never kept, [`memoizable`]). A 12-player C32S2
//! match evaluated about 300 re-evaluated action parameters a tick, of
//! which 99 % came out as the tick before.

use std::cell::RefCell;

use crate::{
    catalog::Id,
    eval::ids,
    host::{Activity, RayHit, Variables, World},
    logic::{self, Arg},
    value::{EntityId, Value},
};

/// What an evaluation read, in order.
#[derive(Debug, Default)]
pub struct Reads(Vec<Read>);

impl Reads {
    /// Whether each read gives the same answer from `host` now, bit for
    /// bit (stopping at the first that doesn't).
    pub fn hold<H: World + Variables + ?Sized>(&self, host: &H) -> bool {
        self.0.iter().all(|read| read.holds(host))
    }
}

/// Whether evaluating `arg` again could be skipped while its reads hold:
/// it doesn't read Total Time Elapsed (the engine's clock, no host read)
/// or draw from the engine's random source.
pub fn memoizable(arg: &Arg) -> bool {
    !names_any(
        arg,
        &[
            ids::TOTAL_TIME_ELAPSED,
            ids::RANDOM_INTEGER,
            ids::RANDOM_REAL,
            ids::RANDOM_VALUE_IN_ARRAY,
            ids::RANDOMIZED_ARRAY,
        ],
    )
}

/// Whether `arg` uses any of values `ids` anywhere.
pub(crate) fn names_any(arg: &Arg, ids: &[Id]) -> bool {
    let value_names = |value: &logic::Value| {
        ids.contains(&value.id) || value.args.iter().any(|arg| names_any(arg, ids))
    };
    match arg {
        Arg::Value(value) => value_names(value),
        Arg::Array(values) => values.iter().any(value_names),
        _ => false,
    }
}

/// The host as an evaluation sees it, each read written down.
pub struct Recorder<'h, H: ?Sized> {
    host: &'h H,
    reads: RefCell<Vec<Read>>,
    /// A read that isn't written down was made (a ray cast): the reads
    /// don't cover the evaluation.
    unrecorded: std::cell::Cell<bool>,
}

impl<'h, H: ?Sized> Recorder<'h, H> {
    pub fn new(host: &'h H) -> Self {
        Self {
            host,
            reads: RefCell::default(),
            unrecorded: std::cell::Cell::new(false),
        }
    }

    /// The reads, if they cover everything the evaluation read.
    pub fn finish(self) -> Option<Reads> {
        (!self.unrecorded.get()).then(|| Reads(self.reads.into_inner()))
    }

    fn push(&self, read: Read) {
        self.reads.borrow_mut().push(read);
    }
}

/// Bit-for-bit equality of an answer.
trait Same {
    fn same(&self, other: &Self) -> bool;
}

macro_rules! same_by_eq {
    ($($ty:ty),*) => {
        $(impl Same for $ty {
            fn same(&self, other: &Self) -> bool {
                self == other
            }
        })*
    };
}

same_by_eq!(
    bool,
    u8,
    i32,
    u32,
    u64,
    usize,
    Option<u64>,
    Option<EntityId>,
    Vec<EntityId>,
    Vec<u64>
);

impl Same for f64 {
    fn same(&self, other: &Self) -> bool {
        self.to_bits() == other.to_bits()
    }
}

impl Same for [f64; 3] {
    fn same(&self, other: &Self) -> bool {
        self.iter().zip(other).all(|(a, b)| a.same(b))
    }
}

impl Same for Value {
    fn same(&self, other: &Self) -> bool {
        self.identical(other)
    }
}

/// The world's answers an evaluation reads ([`World`]), each with its
/// arguments, written down by [`Recorder`] and asked again by
/// [`Read::holds`].
macro_rules! world_reads {
    ($($name:ident($($arg:ident: $ty:ty),*) -> $ret:ty;)*) => {
        #[allow(non_camel_case_types)]
        #[derive(Debug)]
        enum Read {
            $($name { $($arg: $ty,)* answer: $ret },)*
            Global { index: u8, value: Value },
            Player { player: EntityId, index: u8, value: Value },
        }

        impl Read {
            fn holds<H: World + Variables + ?Sized>(&self, host: &H) -> bool {
                match self {
                    $(Self::$name { $($arg,)* answer } => host.$name($(*$arg),*).same(answer),)*
                    Self::Global { index, value } => host.global_is(*index, value),
                    Self::Player { player, index, value } => host.player_is(*player, *index, value),
                }
            }
        }

        impl<H: World + ?Sized> World for Recorder<'_, H> {
            $(fn $name(&self, $($arg: $ty),*) -> $ret {
                let answer = self.host.$name($($arg),*);
                self.push(Read::$name { $($arg,)* answer: answer.clone() });
                answer
            })*

            fn ray_cast(
                &self,
                start: [f64; 3],
                end: [f64; 3],
                include: &[EntityId],
                exclude: &[EntityId],
            ) -> RayHit {
                self.unrecorded.set(true);
                self.host.ray_cast(start, end, include, exclude)
            }
        }
    };
}

world_reads! {
    players() -> Vec<EntityId>;
    team_of(player: EntityId) -> u8;
    slot_of(player: EntityId) -> i32;
    hero_of(player: EntityId) -> Option<u64>;
    is_dummy_bot(player: EntityId) -> bool;
    has_spawned(player: EntityId) -> bool;
    is_alive(player: EntityId) -> bool;
    entity_exists(entity: EntityId) -> bool;
    position(entity: EntityId) -> [f64; 3];
    eye_position(player: EntityId) -> [f64; 3];
    facing(player: EntityId) -> [f64; 3];
    velocity(player: EntityId) -> [f64; 3];
    throttle(player: EntityId) -> [f64; 3];
    is_button_held(player: EntityId, button: Id) -> bool;
    is_on_ground(player: EntityId) -> bool;
    activity(player: EntityId, activity: Activity) -> bool;
    altitude(player: EntityId) -> f64;
    health(player: EntityId) -> f64;
    max_health(player: EntityId) -> f64;
    ultimate_charge_percent(player: EntityId) -> f64;
    ability_cooldown(player: EntityId, button: Id) -> f64;
    has_status(player: EntityId, status: Id) -> bool;
    is_communicating(player: EntityId, kind: Option<Id>) -> bool;
    allowed_heroes(player: EntityId) -> Vec<u64>;
    all_heroes() -> Vec<u64>;
    host_player() -> Option<EntityId>;
    match_time() -> f64;
    is_game_in_progress() -> bool;
    is_in_setup() -> bool;
    is_waiting_for_players() -> bool;
    is_assembling_heroes() -> bool;
    current_map() -> u32;
    current_game_mode() -> u64;
    last_created_entity(creator: Option<EntityId>) -> Value;
    last_text_id(creator: Option<EntityId>) -> Value;
    entity_count() -> usize;
    text_count() -> usize;
    server_load() -> [f64; 3];
    nearest_walkable_position(position: [f64; 3]) -> [f64; 3];
    is_in_line_of_sight(start: [f64; 3], end: [f64; 3]) -> bool;
}

/// A variable read once is written down once: the evaluation sets none, so
/// each read of it gives the same value (the costliest reads to ask again:
/// whole arrays compared).
impl<H: Variables + ?Sized> Variables for Recorder<'_, H> {
    fn global(&self, index: u8) -> Value {
        let value = self.host.global(index);
        let read = |read: &Read| matches!(read, Read::Global { index: was, .. } if *was == index);
        if !self.reads.borrow().iter().any(read) {
            self.push(Read::Global {
                index,
                value: value.clone(),
            });
        }
        value
    }

    fn player(&self, player: EntityId, index: u8) -> Value {
        let value = self.host.player(player, index);
        let read = |read: &Read| matches!(read, Read::Player { player: who, index: was, .. } if *who == player && *was == index);
        if !self.reads.borrow().iter().any(read) {
            self.push(Read::Player {
                player,
                index,
                value: value.clone(),
            });
        }
        value
    }

    fn set_global(&mut self, _: u8, _: Value) {
        unreachable!("an evaluation sets no variable")
    }

    fn set_player(&mut self, _: EntityId, _: u8, _: Value) {
        unreachable!("an evaluation sets no variable")
    }
}
