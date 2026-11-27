//! A map entity on its track ([`crate::track`]): the mover state
//! `STU_49FE0019` and the action `STU_4A4BEDAA`, which the client
//! registers with no behaviour (`python tools/stu.py 49FE0019`: record
//! #243, reader, writer and block the nullsub; `4A4BEDAA` has no
//! registration of its own): the server moves the payload (`Payloads.md`
//! "ModeGraph4 client pass" §2, "Client tools pass").
//!
//! The mover's fields as `1225` binds them (push rid4, rollback rid8, the
//! run of its own rid17, the hold rid139):
//! - `m_speed`: metres a second along the track, read each tick (the push
//!   counts its pushers, `#573`); below zero it rolls back.
//! - `m_971B2969` (#479e): the distance along the track, read and written.
//! - `m_A3D27CCA` (#499): the distance over the track's length.
//! - `m_F620FC2A` (#1622): whether it moved this tick (a rollback held at
//!   a barrier doesn't).
//! - `m_330800D9` (#687) and the plug `m_625C2727`: each waypoint it
//!   passes forward, then the plug (`1225` sends the waypoint `0070`, whose
//!   `121E` answers once per round, st0).
//! - `m_0ECFADFE` (#7582): the checkpoints' distances over the length, in
//!   objective order: the HUD's markers (`STU_EB41F27F` slot 58
//!   `0x7FF7898FAC30` takes each item as a share of the length `1D9F`,
//!   compares it with the distance `612C` and indexes it against the
//!   objectives passed, `m_5F243DBF` = objective − #12454).
//! - `m_onFinishedPlug`: forward at the track's end the state finishes.
//!
//! Assumptions: a waypoint counts as passed once the payload reaches it
//! moving forward (the one it starts on too, so a run from the start
//! tells waypoint 0); a rollback stops at the last barrier
//! ([`Track::floor`], `Payloads.md` §4).
//!
//! `2ABA` (lifts) binds no distance variable. Its distance is held by the
//! native movement component through `World::track_distance`; progress
//! still goes to #18393. Placement and translation retain the map rotation.

use super::*;

impl<W: World + ?Sized> Vm<'_, '_, '_, W> {
    /// The mover's begin: its outputs for where the payload stands, then
    /// a move each tick.
    pub(super) fn begin_track_move(&mut self, key: StateKey, node: &Node) -> Result<(), InitFault> {
        self.state_mut(key).expect("active mover").kind = StateKind::TrackMove {
            last_ms: self.machine.now_ms,
        };
        self.move_on_track(key, node, 0.0)?;
        self.register_tick(key, self.machine.now_ms);
        Ok(())
    }

    /// The mover's tick: its speed for the time since its last move.
    pub(super) fn tick_track_move(&mut self, key: StateKey) -> Result<(), InitFault> {
        let now = self.machine.now_ms;
        let Some(state) = self.state_mut(key) else {
            return Ok(());
        };
        let StateKind::TrackMove { last_ms } = state.kind else {
            return Ok(());
        };
        if last_ms == now {
            return Ok(());
        }
        state.kind = StateKind::TrackMove { last_ms: now };
        let node = self
            .graph(key.instance)
            .state(usize::from(key.raw))
            .expect("mover");
        self.move_on_track(key, node, (now - last_ms) as f64 / 1000.0)
    }

    /// Move `seconds` at the state's speed and write its outputs; the
    /// waypoints passed follow its plug, the track's end finishes it.
    fn move_on_track(&mut self, key: StateKey, node: &Node, seconds: f64) -> Result<(), InitFault> {
        let id = key.instance;
        let site = self.site(id, node);
        let entity = self.machine.snapshot.entity;
        let Some(track) = self.world.track(site, entity) else {
            self.report(site, Issue::UnsupportedState(node.class.clone()));
            return Ok(());
        };
        let length = track.length();
        let distance_var = field(&node.fields, "m_971B2969");
        let from = if distance_var.is_null() {
            self.world.track_distance(site, entity)
        } else {
            let Some(value) = self.evaluate(id, node, distance_var, None) else {
                return Ok(());
            };
            Some(expr::to_float(&value).clamp(0.0, length))
        };
        let speed = if seconds > 0.0 {
            let Some(speed) = self.evaluate(id, node, field(&node.fields, "m_speed"), None) else {
                return Ok(());
            };
            expr::to_float(&speed)
        } else {
            0.0
        };
        // Markers are immutable; do not rebuild them each tick or when
        // this graph has no marker output (the lifts' `2ABA`).
        if seconds == 0.0 && !field(&node.fields, "m_0ECFADFE").is_null() {
            let markers = track
                .waypoints()
                .iter()
                .filter(|waypoint| waypoint.objective.is_some())
                .map(|waypoint| (waypoint.objective, waypoint.distance))
                .collect::<BTreeMap<_, _>>()
                .into_values()
                .map(|distance| Value::Float(distance / length))
                .collect();
            self.put(id, node, "m_0ECFADFE", Value::Array(markers))?;
        }
        // `2ABA` binds progress only. The distance belongs to the native
        // movement component, not to an optional script output.
        let Some(from) = from else {
            self.put(id, node, "m_F620FC2A", Value::Bool(false))?;
            return Ok(());
        };
        let to = if speed >= 0.0 {
            (from + speed * seconds).min(length)
        } else {
            (from + speed * seconds).max(track.floor(from))
        };
        if !self.world.place_on_track(site, entity, to) {
            self.report(site, Issue::UnsupportedState(node.class.clone()));
            return Ok(());
        }
        self.put(id, node, "m_971B2969", Value::Float(to))?;
        self.put(id, node, "m_A3D27CCA", Value::Float(to / length))?;
        self.put(id, node, "m_F620FC2A", Value::Bool(to != from))?;
        if speed <= 0.0 || seconds <= 0.0 {
            return Ok(());
        }
        for waypoint in track.waypoints().iter().filter(|waypoint| {
            from <= waypoint.distance
                && (waypoint.distance < to || (to >= length && waypoint.distance <= to))
        }) {
            if !self.state(key).is_some_and(|state| state.active) {
                return Ok(());
            }
            self.put(id, node, "m_330800D9", Value::Entity(waypoint.entity))?;
            self.follow(id, node, "m_625C2727", Some(key), 0)?;
        }
        if to >= length && from < length {
            self.post(id, Event::Finish(key.raw))?;
        }
        Ok(())
    }

    /// `STU_4A4BEDAA`: put each of `m_targets` on its track at the waypoint
    /// `m_32958E47` (none: the start) plus `m_AEE4E389` metres (`1225`
    /// rid96 puts the payload on the checkpoint where its leg starts,
    /// rid220 back at the start).
    pub(super) fn place_on_track(
        &mut self,
        id: u16,
        node: &Node,
        depth: usize,
    ) -> Result<bool, InitFault> {
        let site = self.site(id, node);
        let Some(targets) = self.evaluate(id, node, field(&node.fields, "m_targets"), None) else {
            return Ok(false);
        };
        let targets: Vec<u32> = match targets {
            Value::Array(items) => items.iter().filter_map(expr::to_entity).collect(),
            value => expr::to_entity(&value).into_iter().collect(),
        };
        let waypoint = match field(&node.fields, "m_32958E47") {
            Json::Null => None,
            config => {
                let Some(value) = self.evaluate(id, node, config, None) else {
                    return Ok(false);
                };
                expr::to_entity(&value).filter(|&entity| expr::valid_entity(entity))
            }
        };
        let offset = match field(&node.fields, "m_AEE4E389") {
            Json::Null => 0.0,
            config => {
                let Some(value) = self.evaluate(id, node, config, None) else {
                    return Ok(false);
                };
                expr::to_float(&value)
            }
        };
        for target in targets
            .into_iter()
            .filter(|&target| expr::valid_entity(target))
        {
            let Some(track) = self.world.track(site, target) else {
                self.report(site, Issue::UnsupportedField("STU_4A4BEDAA target's track"));
                continue;
            };
            let start = waypoint
                .and_then(|waypoint| track.waypoint(waypoint))
                .map_or(0.0, |waypoint| waypoint.distance);
            let distance = (start + offset).clamp(0.0, track.length());
            if !self.world.place_on_track(site, target, distance) {
                self.report(site, Issue::UnsupportedField("STU_4A4BEDAA placement"));
            }
        }
        self.follow(id, node, "m_outPlug", None, depth)?;
        Ok(true)
    }
}
