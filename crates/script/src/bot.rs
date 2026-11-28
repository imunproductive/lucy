//! The statescript of an NPC body, shared by all observers, as the Python
//! reference ran it (`ow174/game/script/bot.py`): the body's graphs, the
//! Practice Range firing behavior, the AI weapon buttons, the practice
//! nameplate and the bot's death.
//!
//! [`BotBody`] mirrors `BotScript`; the match owns the observer streams and
//! projects [`BotBody::snapshot`] with [`BotBody::encoder`].

use std::{error::Error, fmt, ops::Range, sync::Arc};

use serde_json::Value as Json;

use crate::{
    codecs::CodecTable,
    expr::{EntityRelation, EvalError, native_shape},
    graph::{GraphStore, Node},
    snapshot::SnapshotEncoder,
    target_tags::{TargetTagLists, TargetTags},
    timeline::FRAME_MS,
    vitals::PoolKind,
    vm::{
        AiMoveRequest, AuraChange, BodyEffects, CombatMod, Component, HealthChange, InitFault,
        Knockback, MAX_INIT_WORK, MapRays, Motion, PlayRequest, RaisedCriteria, Scaled, SeenBodies,
        ShotEvent, Site, Snapshot, TargetCandidate, TargetTagEffect, Unknowns, World, body_roots,
    },
    weapons::DamageTags,
    wire::{Value, Variables},
};
use lucy_content::maps::{PRACTICE_RANGE_MODE, TUTORIAL_MODE};
use lucy_wire::entity::GAME_MODE_ENTITY;

/// `003E.025`: the body's authoritative death message.
pub const DIED: u64 = 0x0240_0000_0000_003e;
/// `0011.025`: the body took damage. Entries of many body graphs take it
/// (the Training Bot's `0033` rid 3, `0C17` rid 0 and `0E8C` rid 8, all
/// server-only; `01CA` rid 6 in every hero body); no graph sends it, so the
/// server does, as for `003E`. Its parameters, by the low 16 bits of their
/// `026` identifiers: [`DAMAGE_TAGS`], [`DAMAGE_SOURCE`], [`DAMAGE_AMOUNT`],
/// [`DAMAGE_DIRECTION`].
pub const DAMAGED: u64 = 0x0240_0000_0000_0011;
/// `003D.0A4`: the damage tags of the health change, an array of
/// identifiers (`0E8C` #1017 picks the flinch by them; `0C17` skips the pain
/// line for `0262`).
pub const DAMAGE_TAGS: u32 = 0x003d;
/// `0012.0A4`: who dealt it, an entity (`0C17` #41). `003E` carries its
/// killer under the same parameter.
pub const DAMAGE_SOURCE: u32 = 0x0012;
/// `0013.0A4`: how much, a number (`0C17` #337 adds it up over a second
/// and compares the sum with the health left, #7404 / #520, to pick the
/// minor or severe pain line). Assumption: the damage dealt.
pub const DAMAGE_AMOUNT: u32 = 0x0013;
/// `0065.0A4`: a vector (`0E8C` #1018, synced to the clients by st17; its
/// client-only part turns it into the flinch's direction against the
/// body's facing, `2C5` / `1688` the same way). Assumption: the direction
/// the damage travels, from the attacker to the body.
pub const DAMAGE_DIRECTION: u32 = 0x0065;
/// `0013.025`: the placement's eligibility reset.
pub const RESET: u64 = 0x0240_0000_0000_0013;
/// The Training Bot's body graph that puts a respawned bot together: its
/// entry for [`RESET`] plays Anim state 0 or 1 (alias `0451.0C8`, 2.5 s;
/// state 1 while entity variable 7143 is unset), and its Entry sets
/// [`RANGE_BOT`] (upstream kjs `a667ef27`, `bot-rebuild.md`).
pub const REBUILD_GRAPH: u32 = 0x0ba8;
/// Entity variable `#7082`, set by [`REBUILD_GRAPH`]'s Entry on every
/// Practice Range bot body. The bot's death graph `0C18` sends its kill
/// feed message only when the victim or the killer lacks it.
pub const RANGE_BOT: u32 = 7082;
/// The Practice Range firing placement behavior (`FIRING` in
/// `ow174/game/bots.py`).
pub const FIRING_GRAPH: u32 = 0x0b92;
/// The Training Bot patrol behavior (bot bundle; the Practice Range's
/// walking spawners list it, `ai_spawners_174.json`). Read 2026-11-18:
/// Entry → #6960 = #39 → BooleanSwitch on the owner's health > 0 → the AI
/// candidate `STU_91EF24A9` (st0), whose selected subgraph starts StateWait
/// 2 s (st1) and the AI movement state `STU_ECAEE401` (st6, goals move-to
/// and face-to #6948). The 2 s wait goes on to StateWait 1 s (st5), which
/// sets `#39 = (#39 + 1) % count(#6947)` and `#6948 = #6947[#39]` and asks
/// the movement state for the walk (its input `m_8CBFEBB9`). The walk's end
/// (`m_1B09389D`) begins the 1 s wait again; its other plugs `m_881FC2B2`
/// and `m_3D5F729C` begin the 2 s one. So the first walk sets off 3 s
/// after the behavior starts and each next one 1 s after the last ended.
pub const PATROL_GRAPH: u32 = 0x0b8e;
/// The patrol's route `#6947` (`1B23.0D80`): the spawner's AI hint points,
/// in order. Retail's value lists the hint entities; the server gives their
/// positions, which the move goal's `STU_71A87E92` passes through as they
/// are (assumption: the hints don't move).
pub const PATROL_ROUTE: u32 = 6947;
/// The patrol's route index `#39` (`0027.0D80`), the spawner's override (2
/// on every Practice Range route).
pub const PATROL_INDEX: u32 = 39;
/// The nameplate and overhead health bar graph (client-only presenters; for
/// a player under `01CF` state 29, for a training bot under `0C18` state 3).
pub const PRACTICE_NAMEPLATE: u32 = 0x0d24;
pub const PRACTICE_NAMEPLATE_PARENT: u32 = 0x0c18;
/// `0D24`'s text override #8812 (`226C.01C`, a public schema entry and a
/// synced instance variable with a presence bit). Its name field (`01D0.01C`
/// of presenters st42 and st53) shows `#17476 = #8812 || STU_CD7F3617(…)`,
/// the entity's own name, which a bot body doesn't have: the elimination
/// label (`1FC0`) reads the same name and shows none. The stock `0C18`
/// passes no override; graph `0D00` passes a UX display text this way. The
/// server sets it to the bot hero's name as a string, the type the native
/// name has. Assumption, not seen in retail traffic.
pub const NAMEPLATE_TEXT: u32 = 8812;
/// The body's dead flag, entity variable #478 (`01DE.01C`). No graph writes
/// it; 111 graphs read it, and `0D24` shares it with the clients (st76,
/// remote 2). The overhead health bar (st51) needs `#17549 = !#478e && …`
/// (st38), so a bar that is up hides when this goes true. leafall903/ORPHEUS
/// (MIT), commit c239a84, sets it in combat.py `_bot_died` for its
/// Junkenstein NPCs only ("2B33/10 watches the body death flag").
pub const DEAD_FLAG: u32 = 478;
const PRACTICE_BOT_HEROES: Range<u64> = 0x02e0_0000_0000_016b..0x02e0_0000_0000_016f;
const OWNER_HEALTH: u32 = 253;
/// A bot's movement sample on the ground (bit 1 of the 13 movement bits,
/// `lucy_physics::mover::sample_movement_flags`).
pub const BOT_MOVEMENT_SAMPLE: u16 = 1 << 1;

/// Every nameplate (`0D24`) instance of a body's script.
fn nameplates(component: &Component) -> Vec<u16> {
    component
        .snapshot()
        .into_iter()
        .flat_map(|snapshot| &snapshot.instances)
        .filter(|(_, instance)| instance.graph == PRACTICE_NAMEPLATE)
        .map(|(&id, _)| id)
        .collect()
}

/// The practice nameplate instance: graph `0D24` under state 3 of a `0C18`
/// instance (`practice_nameplate` in bot.py).
pub fn practice_nameplate(snapshot: &Snapshot) -> Option<u16> {
    snapshot.instances.iter().find_map(|(&id, instance)| {
        let (parent, state) = instance.parent?;
        (instance.graph == PRACTICE_NAMEPLATE
            && state == 3
            && snapshot
                .instances
                .get(&parent)
                .is_some_and(|parent| parent.graph == PRACTICE_NAMEPLATE_PARENT))
        .then_some(id)
    })
}

/// What an NPC body's graphs ask outside the body (`BotWorld` in bot.py).
/// The rest answers as the reference's base world: ruleset defaults, the
/// mode entity's variables, nothing else.
pub struct BotWorld {
    pub graphs: Arc<GraphStore>,
    pub hero: u64,
    /// The bot's body definition (`003`), which names its animation data.
    pub body: u64,
    pub mode: u64,
    /// The game's ruleset ([`GraphStore::mode_variables`]).
    pub ruleset: u64,
    /// The logical button each AI weapon slot holds (the body's
    /// `ai_weapon_buttons`).
    pub weapon_buttons: Vec<(i64, u8)>,
    /// Where the firing behavior's owner stands; none without the behavior.
    pub owner_position: Option<[f64; 3]>,
    /// The bodies a target search can find, as the match placed them this
    /// tick; none: the world lists no bodies.
    pub targets: Option<Vec<TargetCandidate>>,
    pub unknowns: Unknowns,
    /// The body's health now and at most, as the match keeps it (#253).
    pub health: f64,
    pub max_health: f64,
    /// The criteria the body raised on itself ([`World::criteria`]) since
    /// the match last took them, in order.
    pub criteria: Vec<RaisedCriteria>,
    /// Its 13 movement bits as the match moves it: on the ground
    /// ([`BOT_MOVEMENT_SAMPLE`]) unless a knockback threw it. Assumption:
    /// on the ground the other bits stay off, moving (bit 4) too on a patrol
    /// route, as the match keeps no velocity for a walking bot.
    pub movement_sample: u16,
    /// The health changes the body's graphs made (an aura's heal or
    /// damage), for the match to apply.
    pub health_changes: Vec<HealthChange>,
    /// The auras the body's ApplyAura states put on bodies or took off,
    /// for the match to apply.
    pub aura_changes: Vec<AuraChange>,
    /// The knockbacks the body's graphs reported (`STU_AFBB59E9`: a
    /// punch's `0D07` played on it), for the match.
    pub knockbacks: Vec<Knockback>,
    /// The statuses and combat modifiers its states hold.
    pub effects: BodyEffects,
    /// The tags its own states add and suppress (Biotic Grenade's enemy
    /// aura `0C72` st4 grants `02FB`), as a player's body keeps them.
    pub target_tags: TargetTags,
    /// The map's collision the body's ray casts meet; none: no map.
    pub map_rays: Option<Arc<dyn MapRays>>,
}

impl World for BotWorld {
    fn unknowns(&self) -> Unknowns {
        self.unknowns
    }

    fn map_rays(&self) -> Option<&dyn MapRays> {
        self.map_rays.as_deref()
    }

    /// As a player's body ([`crate::player::PlayerWorld`]).
    fn knockback(&mut self, _site: Site, knockback: Knockback) -> bool {
        self.knockbacks.push(knockback);
        true
    }

    fn assumption(&self, class: &str) -> Option<&'static str> {
        match class {
            // As `PlayerWorld`: the tunable's authored value, not scaled for
            // the entity.
            "STU_082FA44E" => Some("the tunable's authored value"),
            "STU_AC829876" => Some("true (a bot is of entity kind 1)"),
            // As `PlayerWorld`: a server-only entity query whose meaning is
            // open. `0C17` rid 23 starts its pain lines' audience (#1839)
            // with it, and rid 22 and 21 add the bot and its attacker.
            "STU_ED45C5E8" => Some("no entities"),
            _ => None,
        }
    }

    fn ruleset(&mut self, id: u32) -> Option<Value> {
        self.graphs
            .ruleset_value(id, self.hero, self.mode, self.ruleset)
    }

    fn entity_variable(&mut self, entity: u32, id: u32) -> Value {
        if entity != GAME_MODE_ENTITY {
            return Value::Null;
        }
        self.graphs
            .mode_variables(self.mode, self.ruleset)
            .and_then(|variables| variables.get(&id).copied())
            .map_or(Value::Null, Value::from)
    }

    fn ai_weapon_button(&mut self, slot: i64) -> Option<u8> {
        self.weapon_buttons
            .iter()
            .find_map(|&(item, button)| (item == slot).then_some(button))
    }

    /// The body's own total (`STU_9B7648C3` on the owner: `0C17` #520 reads
    /// it after a hit). A bot has one pool as far as the match knows, so a
    /// pool kind is unknown.
    fn body_health(&self, _site: Site, kind: Option<PoolKind>, maximum: bool) -> Option<f64> {
        kind.is_none().then_some(if maximum {
            self.max_health
        } else {
            self.health
        })
    }

    /// The criteria a bot raises on itself (its pain lines); one aimed at
    /// another entity (`0C17` rid16's kill criteria `05E9` on its
    /// controller) has no voice set to answer it here.
    fn criteria(&mut self, site: Site, raised: RaisedCriteria) {
        if raised.target == site.entity {
            self.criteria.push(raised);
        }
    }

    fn modify_health(&mut self, change: HealthChange) -> bool {
        self.health_changes.push(change);
        true
    }

    fn change_aura(&mut self, _site: Site, change: AuraChange) -> bool {
        self.aura_changes.push(change);
        true
    }

    /// A target tag a state of the bot's own grants or removes on it
    /// (Biotic Grenade's enemy aura `0C72` st4 grants `02FB`), kept as a
    /// player's body keeps them; the match sends them as its component 76.
    fn set_target_tag_effect(&mut self, site: Site, effect: TargetTagEffect, active: bool) -> bool {
        effect.entity == site.entity && self.target_tags.set(effect, active)
    }

    /// Its own tags as its states hold them; the authored ones aren't
    /// known here.
    fn has_target_tag(&self, site: Site, entity: u32, tag: u64) -> Option<bool> {
        (entity == site.entity)
            .then(|| self.target_tags.has(tag))
            .flatten()
    }

    fn set_status(&mut self, site: Site, status: Option<i64>) -> bool {
        self.effects.set_status(site, status)
    }

    fn combat_mod(&mut self, site: Site, modifier: Option<CombatMod>) -> bool {
        self.effects.combat_mod(site, modifier)
    }

    /// The body's movement sampler (`STU_96F1DBCF`): what the match gave
    /// ([`BotBody::set_movement_sample`]). `0E8C`'s knockback anim (st7,
    /// which a splash hit starts) ends once this says grounded (bit 1, st11
    /// on #545), 0.25 s on at the soonest; before, no answer left it for
    /// ever.
    fn movement_sample(&self, _site: Site) -> Option<u16> {
        Some(self.movement_sample)
    }

    /// From the body's compiled animation data
    /// ([`crate::anim_lengths::AnimLengths`]).
    fn animation_length(
        &self,
        _site: Site,
        trees: &[u64],
        alias: u64,
        category: u64,
    ) -> Option<f64> {
        self.graphs
            .anim_lengths()
            .length(self.body, trees, alias, category)
    }

    fn animation_scalar(
        &self,
        _site: Site,
        trees: &[u64],
        alias: u64,
        category: u64,
    ) -> Option<f64> {
        self.graphs
            .anim_lengths()
            .scalar(self.body, trees, alias, category)
    }

    /// The reconstruction policy for the behaviors the server starts: the
    /// single authored firing placement selects with an owner position, the
    /// patrol always (one candidate each; the client has no scoring code,
    /// AiDefinitions.md).
    fn selects_ai_behavior(&mut self, site: Site) -> Option<bool> {
        Some(match site.graph {
            FIRING_GRAPH => self.owner_position.is_some(),
            PATROL_GRAPH => true,
            _ => false,
        })
    }

    /// The patrol is written for the AI entity that possesses the body: it
    /// reads the body as `STU_0AC15F78(owner())` (the health test of its
    /// BooleanSwitch). Here it runs on the body itself, which is that body.
    /// Otherwise as the base world ([`crate::vm::listed_relation`]): a
    /// listed body's controller, which also possesses it now; a spawned
    /// entity's owner, and a projectile's shooter (the damaging Biotic
    /// Orb's aura `1101` on a bot names its instigator
    /// `STU_2B2E2F6D(#40, 0)`, the body that threw the orb).
    fn related_entity(&mut self, site: Site, relation: EntityRelation, entity: u32) -> Option<u32> {
        match relation {
            EntityRelation::PossessedBody
                if site.graph == PATROL_GRAPH && entity == site.entity =>
            {
                Some(entity)
            }
            _ => crate::vm::listed_relation(self.target_candidates(site)?, relation, entity),
        }
    }

    fn target_candidates(&self, _site: Site) -> Option<&[TargetCandidate]> {
        self.targets.as_deref()
    }

    /// A bot has no player: it sees the bodies where they are.
    fn seen_bodies(&self, _site: Site) -> Option<SeenBodies<'_>> {
        Some(SeenBodies::Listed)
    }

    /// `STU_4049CF77`: the bot's AI definition (`1404`, `14AF`) has no
    /// difficulty profiles (`m_difficultyProfiles` null, BotDamage.md), so
    /// the graphs take their authored fallback (the Training Bot's rocket,
    /// `0BA3` node 178: `1961`, 8); another listed body's AI as the match
    /// gives it.
    fn ai_parameter(&mut self, site: Site, entity: u32, identifier: u64) -> Option<Option<f64>> {
        if entity == site.entity {
            return Some(None);
        }
        self.targets
            .as_deref()?
            .iter()
            .find(|body| body.entity == entity || body.controller == Some(entity))
            .map(|body| body.ai_parameter(self.graphs.ai_definitions(), identifier))
    }

    fn native(&mut self, site: Site, config: &Json) -> Option<Value> {
        let class = config.get("$")?.as_str()?;
        // The bot's knockback scale (mover mod 26) for Rocket Punch's victim
        // graph `0D07` on it: a Training Bot's graphs hold no movement mod
        // 26, so its default 1 (`lucy_physics::mover::Mover::knockback_scale`).
        if class == "STU_ABE57332" && is_owner(config.get("m_entity")) {
            return Some(Value::Float(1.0));
        }
        if class == "STU_AC829876"
            && self.mode == PRACTICE_RANGE_MODE
            && PRACTICE_BOT_HEROES.contains(&self.hero)
            && site.graph == PRACTICE_NAMEPLATE_PARENT
            && is_owner(config.get("m_entity"))
            && config.get("m_filter").is_some_and(is_nameplate_filter)
        {
            // The stock 0C18 owner gate for its authored 0D24 child. What
            // filter flag 0x100 means natively is unverified.
            return Some(Value::Bool(true));
        }
        if site.graph == FIRING_GRAPH && self.owner_position.is_some() {
            let owner = config.get("m_entity");
            let possessed = owner
                .filter(|owner| owner.get("$").and_then(Json::as_str) == Some("STU_0AC15F78"))
                .and_then(|owner| owner.get("m_entity"))
                .is_some_and(|entity| {
                    entity.get("$").and_then(Json::as_str) == Some("STU_B5A0CAF0")
                });
            if possessed {
                match class {
                    "STU_71A87E92" => return self.owner_position.map(Value::Vector),
                    // This NPC has no player possessor.
                    "STU_295D117D" => return Some(Value::Null),
                    _ => (),
                }
            }
        }
        match class {
            "STU_A3DEF368"
                if self
                    .graphs
                    .mode_variables(self.mode, self.ruleset)
                    .is_some() =>
            {
                Some(Value::Entity(GAME_MODE_ENTITY))
            }
            // See `assumption`.
            "STU_082FA44E" => self.graphs.tunables().value(config).map(Value::Float),
            // See `assumption`.
            "STU_ED45C5E8" => Some(Value::Array(Vec::new())),
            // Is the bot itself of entity kind 1 (Biotic Grenade's auras
            // `0C72` node 16 and `0B99` node 4 ask before their effect):
            // yes, as the target searches take bots (`vm/targets.rs`).
            // Assumption.
            "STU_AC829876"
                if is_owner(config.get("m_entity"))
                    && config.get("m_filter").is_some_and(is_kind_filter) =>
            {
                Some(Value::Bool(true))
            }
            // Whether the body's primary possessor (`STU_DB742151` of the
            // owner) has component 92 (`STU_76AF64F0`): the 104319 reader
            // `CDB700` tests that component (leafall903/ORPHEUS (MIT)
            // `scene.py` `native`, `driver.py` `TRUSTED_NATIVES`; false for
            // a player's controller). An NPC body is possessed by its AI
            // definition (`m_8B9A461F`: `1404` for the Range bodies `1403`,
            // `143E`, `146E`, `14ED`; `14AF` for `14A9`, `14D2`), and both
            // declare component 92. So true: the Training Bot's rocket
            // (`0BA3` node 178) takes its AI damage, not the 19 a player's
            // copy of the volley would deal.
            "STU_D3EB8D87" if is_own_possessor(config.get("m_00903303")) => Some(Value::Bool(true)),
            // IsTutorial (CE8580), as `PlayerWorld` answers it.
            "STU_A77AC2AC" if native_shape(config, class, None) => {
                Some(Value::Bool(self.mode == TUTORIAL_MODE))
            }
            _ => None,
        }
    }
}

/// `{"$": "STU_B5A0CAF0"}`, optionally with a zero `m_EE729DCB`.
fn is_owner(config: Option<&Json>) -> bool {
    config.and_then(Json::as_object).is_some_and(|fields| {
        fields.get("$").and_then(Json::as_str) == Some("STU_B5A0CAF0")
            && fields.iter().all(|(key, value)| match key.as_str() {
                "$" => true,
                "m_EE729DCB" => value.as_u64() == Some(0),
                _ => false,
            })
    })
}

/// A filter whose only set field is the entity kind 1.
fn is_kind_filter(filter: &Json) -> bool {
    filter.as_object().is_some_and(|fields| {
        fields.iter().all(|(key, value)| match key.as_str() {
            "$" => value.as_str() == Some("STUConfigVarFilter"),
            "m_84065FD1" => value.as_u64() == Some(1),
            _ => matches!(value, Json::Null) || value.as_u64() == Some(0),
        })
    })
}

/// `STU_DB742151` (the primary possessor) of the owner.
fn is_own_possessor(config: Option<&Json>) -> bool {
    config.is_some_and(|config| {
        native_shape(config, "STU_DB742151", Some("m_entity")) && is_owner(config.get("m_entity"))
    })
}

/// The stock filter: its set fields (not null, not zero) are exactly the
/// candidate mask and the 0x100 exclusion.
fn is_nameplate_filter(filter: &Json) -> bool {
    let Some(fields) = filter.as_object() else {
        return false;
    };
    let set = |value: &Json| match value {
        Json::Null | Json::Bool(false) => false,
        Json::Number(number) => number.as_f64() != Some(0.0),
        _ => true,
    };
    let mut expected = [("$", false), ("m_84065FD1", false), ("m_4EA686DB", false)];
    for (key, value) in fields.iter().filter(|(_, value)| set(value)) {
        let matches = match key.as_str() {
            "$" => value.as_str() == Some("STUConfigVarFilter"),
            "m_84065FD1" => value.as_u64() == Some(0x3fff_ffff),
            "m_4EA686DB" => value.as_u64() == Some(0x100),
            _ => false,
        };
        let Some(seen) = expected.iter_mut().find(|(name, _)| name == key) else {
            return false;
        };
        if !matches {
            return false;
        }
        seen.1 = true;
    }
    expected.iter().all(|(_, seen)| *seen)
}

#[derive(Debug, Eq, PartialEq)]
pub enum BotError {
    MissingBody(u64),
    DeathOrder { frame: u32, last: u32 },
    Script(InitFault),
}

impl fmt::Display for BotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingBody(body) => write!(f, "no statescript data for entity body {body:016X}"),
            Self::DeathOrder { frame, last } => {
                write!(
                    f,
                    "bot death at frame {frame} predates its script frame {last}"
                )
            }
            Self::Script(error) => error.fmt(f),
        }
    }
}

impl Error for BotError {}

impl From<InitFault> for BotError {
    fn from(error: InitFault) -> Self {
        Self::Script(error)
    }
}

/// Where and as what an NPC body starts.
#[derive(Clone, Copy, Debug)]
pub struct BotSpawn<'a> {
    /// The entity body definition (`0x0400…`).
    pub body: u64,
    pub hero: u64,
    /// The hero's display name (`075` `m_0EDCE350`, "Training Bot"); empty
    /// for none. See [`NAMEPLATE_TEXT`].
    pub name: &'a str,
    pub entity: u32,
    pub frame: u32,
    pub mode: u64,
    pub ruleset: u64,
    pub health: f64,
    /// The firing placement's position; none for a bot without one.
    pub firing_position: Option<[f64; 3]>,
    /// The patrol's route, for a spawner that lists [`PATROL_GRAPH`]: the
    /// behavior starts with the body and asks the server's walker for each
    /// walk ([`BotBody::move_goal`]).
    pub patrol: Option<Patrol<'a>>,
    /// A bot made again at its spawner after a death: [`RESET`] goes to its
    /// [`REBUILD_GRAPH`] instances before its first frame, so it is seen
    /// putting itself together (upstream kjs `a667ef27`; seen there in the
    /// original client: the parts gather over about 0.9 s). Retail's sender
    /// of the message is not known: a reconstruction. A first spawn plays
    /// no rebuild.
    pub respawned: bool,
    /// The map's collision, for the body's ray casts; none: no map.
    pub map_rays: Option<&'a Arc<dyn MapRays>>,
}

/// A patrolling spawner's overrides of [`PATROL_GRAPH`]'s variables.
#[derive(Clone, Copy, Debug)]
pub struct Patrol<'a> {
    /// [`PATROL_ROUTE`]: the hint points' positions, in order.
    pub route: &'a [[f64; 3]],
    /// [`PATROL_INDEX`].
    pub index: i64,
}

/// An NPC body's statescript on the server clock (`BotScript` in bot.py).
pub struct BotBody {
    component: Component,
    world: BotWorld,
    /// The AI behaviors the server started (firing, patrol): the server's
    /// own, so the observers' frames leave them out.
    behaviors: Vec<u16>,
    frame: u32,
    death_frame: Option<u32>,
}

impl BotBody {
    /// Run the body's graphs through their spawn frame and start the AI
    /// behaviors the server gives it: with a firing position and AI weapon
    /// buttons the firing behavior with its scoped reset, with a route the
    /// patrol. Each nameplate instance (`0D24`) then gets the name as
    /// its text override ([`NAMEPLATE_TEXT`]). Nothing enables the plate
    /// (`#8807`): the client shows it by itself (`docs/notes/behavior.md`,
    /// "When a bot's plate shows").
    pub fn start(
        graphs: Arc<GraphStore>,
        spawn: BotSpawn<'_>,
        unknowns: Unknowns,
    ) -> Result<Self, BotError> {
        let body = graphs
            .entity_body(spawn.body)
            .ok_or(BotError::MissingBody(spawn.body))?;
        let weapon_buttons: Vec<_> = body
            .ai_weapon_buttons
            .iter()
            .filter_map(|(slot, button)| {
                Some((slot.parse().ok()?, u8::try_from(button.as_u64()?).ok()?))
            })
            .collect();
        let firing = !weapon_buttons.is_empty() && graphs.graph(FIRING_GRAPH).is_some();
        let mut roots = body_roots(body)?;
        // Hero-body defaults (players use the same schema). Non-hero
        // definitions can declare entity references, which need world
        // binding context: docs/dev/loose-ends.md "Non-hero root defaults".
        if body.hero.is_some() {
            crate::vm::body_defaults(&mut roots, &graphs)?;
        }
        let mut world = BotWorld {
            graphs: Arc::clone(&graphs),
            hero: spawn.hero,
            body: spawn.body,
            mode: spawn.mode,
            ruleset: spawn.ruleset,
            weapon_buttons,
            owner_position: spawn.firing_position.filter(|_| firing),
            targets: None,
            unknowns,
            health: spawn.health,
            max_health: spawn.health,
            criteria: Vec::new(),
            movement_sample: BOT_MOVEMENT_SAMPLE,
            health_changes: Vec::new(),
            aura_changes: Vec::new(),
            knockbacks: Vec::new(),
            effects: BodyEffects::default(),
            target_tags: TargetTags::default(),
            map_rays: spawn.map_rays.cloned(),
        };
        let mut component = Component::start(
            graphs,
            spawn.entity,
            Variables::from([(OWNER_HEALTH, Value::Float(spawn.health))]),
            roots,
            spawn.frame,
            &mut world,
            MAX_INIT_WORK,
        )?;
        component.advance_frame(spawn.frame, &mut world)?;
        let mut behaviors = Vec::new();
        if world.owner_position.is_some()
            && let Some(id) = component.add_instance(FIRING_GRAPH, Variables::new(), &mut world)?
        {
            behaviors.push(id);
            // The reconstructed spawn reset goes to this instance only;
            // other 0013 receivers have their own rules.
            component.queue_message(id, RESET, Variables::new())?;
            component.run_now(&mut world)?;
        }
        if let Some(patrol) = spawn.patrol {
            let route = patrol.route.iter().copied().map(Value::Vector).collect();
            let variables = Variables::from([
                (PATROL_ROUTE, Value::Array(route)),
                (PATROL_INDEX, Value::Integer(patrol.index)),
            ]);
            if let Some(id) = component.add_instance(PATROL_GRAPH, variables, &mut world)? {
                behaviors.push(id);
                component.run_now(&mut world)?;
            }
        }
        if spawn.respawned {
            let rebuilds: Vec<u16> = component
                .snapshot()
                .into_iter()
                .flat_map(|snapshot| &snapshot.instances)
                .filter(|(_, instance)| instance.graph == REBUILD_GRAPH)
                .map(|(&id, _)| id)
                .collect();
            for id in rebuilds {
                component.queue_message(id, RESET, Variables::new())?;
            }
            component.run_now(&mut world)?;
        }
        if !spawn.name.is_empty() {
            for id in nameplates(&component) {
                component.set_instance_variable(
                    id,
                    NAMEPLATE_TEXT,
                    Value::String(spawn.name.to_owned()),
                    &mut world,
                )?;
            }
        }
        Ok(Self {
            component,
            world,
            behaviors,
            frame: spawn.frame,
            death_frame: None,
        })
    }

    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.component.snapshot()
    }

    /// The observer frames' encoder: it leaves the AI behaviors out.
    pub fn encoder<'a>(
        &'a self,
        graphs: &'a GraphStore,
        codecs: &'a CodecTable,
    ) -> SnapshotEncoder<'a> {
        SnapshotEncoder::new(graphs, codecs)
            .with_unknowns(self.world.unknowns)
            .hiding(&self.behaviors)
    }

    pub fn component(&self) -> &Component {
        &self.component
    }

    pub fn world(&self) -> &BotWorld {
        &self.world
    }

    /// What a health change of `kind` with `tags` that `other` is the other
    /// party of (its attacker or healer) becomes on the body
    /// ([`BodyEffects::scale`]): its states' modifiers, their party filters
    /// tested on `other`.
    pub fn combat_scale(&mut self, kind: u64, tags: &mut DamageTags, other: u32) -> Scaled {
        let passed = self.component.passed_party_filters(
            &mut self.world,
            |world| &world.effects,
            kind,
            other,
        );
        self.world.effects.scale(kind, tags, &passed)
    }

    /// The tags the body's states add and suppress now, when a tag came or
    /// went since the last call ([`TargetTags::take_change`]).
    pub fn take_target_tag_change(&mut self) -> Option<TargetTagLists> {
        self.world.target_tags.take_change()
    }

    /// The dashes and velocity actions the body's graphs began since the
    /// last call, for its mover ([`Component::take_motions`]).
    pub fn take_motions(&mut self) -> Vec<Motion> {
        self.component.take_motions()
    }

    /// Its movement bits from now on (the body's sampler reads them as its
    /// frames run); true when they changed, so a frame should run to see it.
    pub fn set_movement_sample(&mut self, sample: u16) -> bool {
        std::mem::replace(&mut self.world.movement_sample, sample) != sample
    }

    /// The AI behaviors' instances (firing, patrol) while they run.
    pub fn behaviors(&self) -> &[u16] {
        &self.behaviors
    }

    /// The point the patrol's movement state asks the walker to walk to,
    /// with the state ([`Component::ai_move_requests`]); none between
    /// walks or once dead.
    pub fn move_goal(&self) -> Option<AiMoveRequest> {
        if self.component.owner_dead() {
            return None;
        }
        self.component.ai_move_requests().into_iter().next()
    }

    /// The walker's report that the walk to `goal` ended at world tick
    /// `frame` ([`Component::ai_move_arrived`]); the graph takes it at that
    /// frame's time, when the body next runs.
    pub fn move_arrived(&mut self, goal: AiMoveRequest, frame: u32) -> Result<(), BotError> {
        let delay = u64::from(frame.saturating_sub(self.frame)) * FRAME_MS;
        self.component
            .ai_move_arrived(goal.instance, goal.raw_state, delay, &mut self.world)?;
        Ok(())
    }

    pub fn frame(&self) -> u32 {
        self.frame
    }

    pub fn death_frame(&self) -> Option<u32> {
        self.death_frame
    }

    pub fn take_shots(&mut self) -> Vec<ShotEvent> {
        self.component.take_shots().unwrap_or_default()
    }

    /// Read the authored volley a shot came from, evaluating its configs in
    /// the shot's state (`state.evaluate` in combat.py `_shoot`). None: the
    /// shot's state is no longer a weapon volley of this body.
    pub fn with_shot<T>(
        &mut self,
        shot: &ShotEvent,
        read: impl FnOnce(&Node, &mut dyn FnMut(&Json) -> Result<Value, EvalError>) -> T,
    ) -> Option<T> {
        let Self {
            component, world, ..
        } = self;
        let node = component.shot_node(shot)?;
        let mut evaluate = |config: &Json| component.evaluate_shot_config(shot, config, world);
        Some(read(node, &mut evaluate))
    }

    /// The volley count of the state a shot came from, which the other
    /// clients' shot record names (projectiles.py `_show_others`:
    /// `state.volleys`); 0 when that state is gone.
    pub fn shot_volleys(&self, shot: &ShotEvent) -> u8 {
        self.component
            .snapshot()
            .and_then(|snapshot| snapshot.instances.get(&shot.instance))
            .and_then(|instance| instance.states.get(&shot.raw_state))
            .map_or(0, |state| match state.kind {
                crate::vm::StateKind::WeaponVolley { volleys, .. } => volleys,
                _ => 0,
            })
    }

    /// The owner's health input #253 (`set_health` in driver.py), kept in
    /// step with the bot's pools after each hit (combat.py
    /// `_sync_body_health`).
    pub fn set_health(&mut self, health: f64) -> Result<(), BotError> {
        self.world.health = health;
        self.component
            .set_entity_variable(OWNER_HEALTH, Value::Float(health), &mut self.world)?;
        Ok(())
    }

    /// Set one of the body's entity variables now; the next frame carries
    /// it to the clients when a graph shares it.
    pub fn set_entity_variable(&mut self, id: u32, value: Value) -> Result<(), BotError> {
        self.component
            .set_entity_variable(id, value, &mut self.world)?;
        Ok(())
    }

    /// One of the body's entity variables as stored now (none: never set).
    pub fn entity_variable(&self, id: u32) -> Option<&Value> {
        self.component.snapshot()?.variables.get(&id)
    }

    /// The hit the body took and survived: `0011.025` ([`DAMAGED`]) with
    /// its parameters runs now. The Training Bot's `0E8C` sets its flinch
    /// (#1024, #6500, #1018, synced by st17 for the clients' own flinch
    /// anim) and `0C17` raises its pain criteria 0.1 s later. A body that
    /// doesn't fire never advances on its own, so the script first catches
    /// up to the frame before `frame`; a firing body still runs that frame
    /// for its weapons. False: it is dead or no entry took the message.
    pub fn take_damage(&mut self, frame: u32, parameters: Variables) -> Result<bool, BotError> {
        self.receive_message(frame, 0, DAMAGED, parameters)
    }

    /// A game message from `sender` at world frame `frame` (to the body or
    /// to every entity): its entries run now, after the script caught up
    /// to the frame before (as [`Self::take_damage`]). False: the body is
    /// dead or no entry took it.
    pub fn receive_message(
        &mut self,
        frame: u32,
        sender: u32,
        message: u64,
        parameters: Variables,
    ) -> Result<bool, BotError> {
        if self.component.owner_dead() {
            return Ok(false);
        }
        let before = frame.saturating_sub(1);
        if before > self.frame {
            self.frame = before;
            self.component.advance_frame(before, &mut self.world)?;
        }
        Ok(self
            .component
            .receive_message(sender, message, parameters, &mut self.world)?
            != 0)
    }

    /// A persistent sender (`sender`) let its message to the body go
    /// ([`Component::end_message`]).
    pub fn end_message(&mut self, sender: u32, message: u64) -> Result<(), BotError> {
        Ok(self
            .component
            .end_message(sender, message, &mut self.world)?)
    }

    /// Apply an aura change to the live body at `frame`
    /// ([`Component::change_aura`]): an aura put on it (by a hit or an
    /// ApplyAura elsewhere) or taken off. False: nothing changed (a dead
    /// body takes no aura; the bundle lacks the aura; it already left).
    pub fn change_aura(&mut self, frame: u32, change: &AuraChange) -> Result<bool, BotError> {
        if matches!(change, AuraChange::Apply { .. }) && self.component.owner_dead() {
            return Ok(false);
        }
        let before = frame.saturating_sub(1);
        if before > self.frame {
            self.frame = before;
            self.component.advance_frame(before, &mut self.world)?;
        }
        Ok(self.component.change_aura(change, &mut self.world)?)
    }

    /// The health changes the body's graphs made since the last call.
    pub fn take_health_changes(&mut self) -> Vec<HealthChange> {
        std::mem::take(&mut self.world.health_changes)
    }

    /// The aura changes the body's graphs made since the last call.
    pub fn take_aura_changes(&mut self) -> Vec<AuraChange> {
        std::mem::take(&mut self.world.aura_changes)
    }

    /// Another body's ability graph played `request.graph` on the live
    /// body at `frame` (Rocket Punch's `0D07`, `vm/play.rs`
    /// `Plays::Others`): played as its PlayScript would. False on a dead
    /// body.
    pub fn play_script(&mut self, frame: u32, request: &PlayRequest) -> Result<bool, BotError> {
        if self.component.owner_dead() {
            return Ok(false);
        }
        let before = frame.saturating_sub(1);
        if before > self.frame {
            self.frame = before;
            self.component.advance_frame(before, &mut self.world)?;
        }
        self.component.play_script(request, &mut self.world)?;
        Ok(true)
    }

    /// The knockbacks the body's graphs reported since the last call.
    pub fn take_knockbacks(&mut self) -> Vec<Knockback> {
        std::mem::take(&mut self.world.knockbacks)
    }

    /// Whether the live body has something to run by the end of `frame`: a
    /// graph event due (its waits after a hit), or a health change that
    /// applies each frame (an aura's rate: the damaging Biotic Orb's
    /// `1101`). Other per-frame states don't make it run: an idle bot runs
    /// only for its events, which its hit reactions' timing depends on.
    pub fn due_by(&self, frame: u32) -> bool {
        !self.component.owner_dead()
            && (self.component.changes_health()
                || self
                    .component
                    .next_due_ms()
                    .is_some_and(|due| due / crate::timeline::FRAME_MS <= u64::from(frame)))
    }

    /// The criteria the body raised on itself since the last call, in order
    /// ([`World::criteria`]).
    pub fn take_criteria(&mut self) -> Vec<RaisedCriteria> {
        std::mem::take(&mut self.world.criteria)
    }

    /// The bodies the body's target searches can find from now on, as the
    /// match placed them this tick.
    pub fn set_target_candidates(&mut self, candidates: &[TargetCandidate]) {
        let targets = self.world.targets.get_or_insert_with(Vec::new);
        targets.clear();
        targets.extend_from_slice(candidates);
    }

    /// Advance a live body one server frame; the firing behavior's owner
    /// moves to `position` (`BotScript.advance_live`). False: nothing ran
    /// (an old frame or a dead bot).
    pub fn advance_live(
        &mut self,
        frame: u32,
        position: Option<[f64; 3]>,
    ) -> Result<bool, BotError> {
        if frame <= self.frame || self.component.owner_dead() {
            return Ok(false);
        }
        self.frame = frame;
        if let Some(position) = position.filter(|_| self.world.owner_position.is_some()) {
            self.world.owner_position = Some(position);
        }
        self.component.advance_frame(frame, &mut self.world)?;
        Ok(true)
    }

    /// Deliver the death message (`BotScript.die`): the behaviors go, the
    /// shots and held buttons are dropped, the frame runs, the dead flag
    /// ([`DEAD_FLAG`]) goes true, health is 0, and `003E.025` runs now. A
    /// bot that dies in its spawn frame runs the death in the next frame,
    /// after its initial frame. False: it was already dead or no entry took
    /// the message. The caller freezes the last live observer baseline
    /// before calling this.
    ///
    /// The dead flag is set for every bot, in upstream's order (before the
    /// health and 003E); upstream sets it for Junkenstein's NPCs only.
    /// Assumption: the retail server set it for any body that died, as no
    /// graph does and the clients' plates and death checks read it.
    pub fn die(&mut self, frame: u32, parameters: Variables) -> Result<bool, BotError> {
        if self.component.owner_dead() {
            return Ok(false);
        }
        if frame < self.frame {
            return Err(BotError::DeathOrder {
                frame,
                last: self.frame,
            });
        }
        self.frame = frame.max(self.frame.saturating_add(1));
        self.component.set_owner_dead(true);
        for behavior in std::mem::take(&mut self.behaviors) {
            self.component.destroy_instance(behavior, &mut self.world)?;
        }
        self.component.discard_shots();
        self.component.advance_frame(self.frame, &mut self.world)?;
        self.component.release_buttons();
        self.component
            .set_entity_variable(DEAD_FLAG, Value::Bool(true), &mut self.world)?;
        self.world.health = 0.0;
        self.component
            .set_entity_variable(OWNER_HEALTH, Value::Float(0.0), &mut self.world)?;
        if self
            .component
            .receive_message(0, DIED, parameters, &mut self.world)?
            == 0
        {
            return Ok(false);
        }
        self.death_frame = Some(frame);
        Ok(true)
    }

    /// Run the graph events due within the death's lifetime, frame by frame
    /// up to `frame` (`BotScript.advance_death`); `each` sees the body after
    /// every frame that ran.
    pub fn advance_death(
        &mut self,
        frame: u32,
        lifetime_frames: u32,
        mut each: impl FnMut(u32, &Snapshot),
    ) -> Result<(), BotError> {
        let Some(death) = self.death_frame else {
            return Ok(());
        };
        let limit = frame.min(death.saturating_add(lifetime_frames));
        while let Some(due) = self.component.next_due_ms() {
            let Ok(due) = u32::try_from(due / crate::timeline::FRAME_MS) else {
                break;
            };
            if due > limit {
                break;
            }
            self.frame = due;
            self.component.advance_frame(due, &mut self.world)?;
            if let Some(snapshot) = self.component.snapshot() {
                each(due, snapshot);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const BODY: u64 = 0x0400_0000_0000_1403;
    const NAMED_BODY: u64 = 0x0400_0000_0000_f0fd;
    const TRAINING_BOT: u64 = 0x02e0_0000_0000_016b;
    const ENTITY: u32 = 0xc000_1000;
    const WEAPON: u32 = 0xf0fe;
    const LIVE: u32 = 32;

    fn node(position: u16, class: &str, state: Option<u16>, mut fields: Json) -> Json {
        fields["m_2BBEEAB8"] = json!(state.is_some());
        fields["m_ADEB6E05"] = json!(class.contains("Action"));
        json!({
            "pos":position, "class":class, "uid":position, "rid":position, "remote":-1,
            "state":state, "client_only":false, "server_only":false, "fields":fields,
        })
    }

    fn plug(target: u16, input: &str) -> Json {
        json!({"$":"Plug","links":[[target, input]],"m_FF3DAF1E":[]})
    }

    fn on_message(position: u16, message: u64, target: u16, input: &str) -> Json {
        node(
            position,
            "STUStatescriptEntryGameMessage",
            None,
            json!({
                "m_gameMessage":{"m_gameMessage":format!("0x{message:016X}")},
                "m_outPlug":plug(target, input),
            }),
        )
    }

    fn mark(position: u16) -> Json {
        node(
            position,
            "STUStatescriptActionSetVar",
            None,
            json!({
                "m_out_Var":{"$":"STUConfigVarDynamic","m_identifier":LIVE},
                "m_value":{"$":"STUConfigVarBool","m_value":1},
            }),
        )
    }

    fn graph(index: u32, nodes: Vec<Json>, states: Json, entries: Json) -> Json {
        json!({
            "index":index, "items":nodes.len(), "nodes":nodes, "states":states,
            "entries":entries, "remote":[], "sync_vars":[], "containers":{},
            "unknown_nodes":[], "fields":{"m_680A2CB2":[0]},
            "header":{
                "m_nodesBitCount":4, "m_statesBitCount":4, "m_remoteSyncNodesBitCount":4,
                "m_syncVarsBitCount":4
            }
        })
    }

    /// A weapon whose button 20 starts a volley, and a firing behavior whose
    /// reset holds AI weapon slot 0 (button 20) down.
    fn graphs() -> Arc<GraphStore> {
        let weapon = graph(
            WEAPON,
            vec![
                node(
                    0,
                    "STUStatescriptEntry",
                    None,
                    json!({"m_outPlug":plug(1, "m_beginPlug")}),
                ),
                node(
                    1,
                    "STUStatescriptStateLogicalButton",
                    Some(0),
                    json!({
                        "m_logicalButton":{"$":"STUConfigVarInt","m_value":20},
                        "m_onGoingDownPlug":plug(2, "m_beginPlug"),
                    }),
                ),
                node(2, "STUStatescriptStateWeaponVolley", Some(1), json!({})),
                on_message(3, RESET, 5, "m_inPlug"),
                on_message(4, DIED, 5, "m_inPlug"),
                mark(5),
            ],
            json!([1, 2]),
            json!([0, 3, 4]),
        );
        let firing = graph(
            FIRING_GRAPH,
            vec![
                on_message(0, RESET, 1, "m_inPlug"),
                node(
                    1,
                    "STUStatescriptActionSetVar",
                    None,
                    json!({
                        "m_out_Var":{"$":"STUConfigVarDynamic","m_identifier":LIVE},
                        "m_value":{"$":"STUConfigVarBool","m_value":1},
                        "m_outPlug":plug(2, "m_beginPlug"),
                    }),
                ),
                node(
                    2,
                    "STU_D7F9E1C3",
                    Some(0),
                    json!({
                        "m_14EDDDA7":{"$":"STU_B5A0CAF0"},
                        "m_20BCEE62":{"$":"STUConfigVarBool","m_value":1},
                        "m_A60150E2":{"$":"STUConfigVarInt","m_value":0},
                    }),
                ),
            ],
            json!([2]),
            json!([0]),
        );
        // A body whose one graph is a nameplate stand-in: as the stock 0D24
        // (st56, st57), a state waits for `0C3C.025` and then holds #8807
        // true.
        let nameplate = graph(
            PRACTICE_NAMEPLATE,
            vec![
                node(
                    0,
                    "STUStatescriptEntry",
                    None,
                    json!({"m_outPlug":plug(1, "m_beginPlug")}),
                ),
                node(
                    1,
                    "STUStatescriptStateGameMessageEntry",
                    Some(0),
                    json!({
                        "m_gameMessage":{
                            "m_gameMessage":"0x0240000000000C3C"
                        },
                        "m_452862E6":{"$":"Plug","links":[[2, "m_beginPlug"]],"m_FF3DAF1E":[1]},
                    }),
                ),
                node(
                    2,
                    "STUStatescriptStateStack",
                    Some(1),
                    json!({
                        "m_out_Var":{"$":"STUConfigVarDynamic","m_identifier":8807},
                        "m_value":{"$":"STUConfigVarBool","m_value":1},
                    }),
                ),
            ],
            json!([1, 2]),
            json!([0]),
        );
        let body = json!({
            "hero":null, "name":"Fixture", "manager":null, "weapons":[],
            "graphs":[{"graph":WEAPON, "m_1EB5A024":null, "m_75408640":0}],
            "ai_weapon_buttons":{"0":20},
        });
        let named = json!({
            "hero":null, "name":"Named", "manager":null, "weapons":[],
            "graphs":[{"graph":PRACTICE_NAMEPLATE, "m_1EB5A024":null, "m_75408640":0}],
            "ai_weapon_buttons":{},
        });
        Arc::new(
            GraphStore::from_json(
                &serde_json::to_vec(&json!({
                    "version":1, "graphs":[weapon, firing, nameplate], "bodies":{},
                    "entities":{
                        format!("0x{BODY:016X}"):body,
                        format!("0x{NAMED_BODY:016X}"):named,
                    },
                    "missing":[]
                }))
                .unwrap(),
            )
            .unwrap(),
        )
    }

    fn spawn(firing_position: Option<[f64; 3]>) -> BotSpawn<'static> {
        BotSpawn {
            body: BODY,
            hero: TRAINING_BOT,
            name: "Training Bot",
            entity: ENTITY,
            frame: 100,
            mode: PRACTICE_RANGE_MODE,
            ruleset: 0,
            health: 200.0,
            firing_position,
            patrol: None,
            respawned: false,
            map_rays: None,
        }
    }

    #[test]
    fn a_nameplate_instance_takes_the_bots_name_as_its_text() {
        let graphs = graphs();
        let named = BotSpawn {
            body: NAMED_BODY,
            ..spawn(None)
        };
        let bot = BotBody::start(Arc::clone(&graphs), named, Unknowns::Refuse).unwrap();
        let snapshot = bot.snapshot().unwrap();
        let (_, nameplate) = snapshot
            .instances
            .iter()
            .find(|(_, instance)| instance.graph == PRACTICE_NAMEPLATE)
            .unwrap();
        assert_eq!(
            nameplate.variables.get(&NAMEPLATE_TEXT),
            Some(&Value::String("Training Bot".into()))
        );
        // No name, no override; other graphs never get one.
        let nameless = BotSpawn { name: "", ..named };
        let bot = BotBody::start(Arc::clone(&graphs), nameless, Unknowns::Refuse).unwrap();
        let snapshot = bot.snapshot().unwrap();
        assert!(
            snapshot
                .instances
                .values()
                .all(|instance| !instance.variables.contains_key(&NAMEPLATE_TEXT))
        );
        let bot = BotBody::start(graphs, spawn(None), Unknowns::Refuse).unwrap();
        assert!(
            bot.snapshot()
                .unwrap()
                .instances
                .values()
                .all(|instance| !instance.variables.contains_key(&NAMEPLATE_TEXT))
        );
    }

    #[test]
    fn the_firing_behavior_holds_the_weapon_button_until_the_bot_dies() {
        let graphs = graphs();
        let codecs = CodecTable::client();
        // Without a firing position nothing presses the button.
        let mut idle = BotBody::start(Arc::clone(&graphs), spawn(None), Unknowns::Refuse).unwrap();
        assert!(idle.behaviors().is_empty());
        assert!(idle.advance_live(101, None).unwrap());
        assert!(idle.take_shots().is_empty());

        let mut bot = BotBody::start(
            Arc::clone(&graphs),
            spawn(Some([0.0, 0.0, 3.0])),
            Unknowns::Refuse,
        )
        .unwrap();
        assert_eq!(bot.behaviors(), [2]);
        let snapshot = bot.snapshot().unwrap();
        assert!(
            snapshot.initialization_complete(),
            "{:?}",
            snapshot.diagnostics
        );
        // The reset reached the behavior only.
        assert_eq!(snapshot.instances[&2].variables[&LIVE], Value::Bool(true));
        assert!(!snapshot.instances[&1].variables.contains_key(&LIVE));
        // The held button went down and fired its one-shot volley.
        assert_eq!(
            snapshot.instances[&1].states[&0].kind,
            crate::vm::StateKind::LogicalButton { down: true }
        );
        assert_eq!(snapshot.instances[&1].states[&1].activation, 1);
        let observed = bot
            .encoder(&graphs, &codecs)
            .project(snapshot, crate::snapshot::View::Observer)
            .unwrap();
        assert_eq!(observed.instance_count(), 1);
        assert!(!bot.advance_live(100, None).unwrap());
        assert!(bot.advance_live(101, Some([1.0, 0.0, 3.0])).unwrap());
        assert_eq!(bot.world().owner_position, Some([1.0, 0.0, 3.0]));
        assert!(!bot.take_shots().is_empty());

        assert_eq!(
            bot.die(100, Variables::new()),
            Err(BotError::DeathOrder {
                frame: 100,
                last: 101
            })
        );
        assert!(bot.die(101, Variables::new()).unwrap());
        assert!(bot.component().owner_dead());
        assert!(bot.behaviors().is_empty());
        assert_eq!(bot.frame(), 102);
        assert_eq!(bot.death_frame(), Some(101));
        let snapshot = bot.snapshot().unwrap();
        assert_eq!(snapshot.instances.keys().copied().collect::<Vec<_>>(), [1]);
        assert_eq!(snapshot.instances[&1].variables[&LIVE], Value::Bool(true));
        assert_eq!(snapshot.variables[&OWNER_HEALTH], Value::Float(0.0));
        assert_eq!(
            snapshot.instances[&1].states[&0].kind,
            crate::vm::StateKind::LogicalButton { down: false }
        );
        assert!(bot.take_shots().is_empty());
        // A dead bot neither dies again nor advances live.
        assert!(!bot.die(103, Variables::new()).unwrap());
        assert!(!bot.advance_live(103, None).unwrap());
        let mut frames = Vec::new();
        bot.advance_death(200, 60, |frame, _| frames.push(frame))
            .unwrap();
        assert!(frames.is_empty());
    }

    #[test]
    fn the_world_answers_only_the_stock_nameplate_gate_and_firing_owner() {
        let filter = json!({
            "$":"STUConfigVarFilter", "m_84065FD1":0x3fff_ffff_u32, "m_4EA686DB":0x100,
            "m_27CF71D5":0, "m_condition":null,
        });
        let gate = json!({"$":"STU_AC829876", "m_entity":{"$":"STU_B5A0CAF0"}, "m_filter":filter});
        let site = |graph| Site {
            entity: ENTITY,
            instance: 1,
            graph,
            node: Some(1),
        };
        let mut world = BotWorld {
            graphs: graphs(),
            hero: TRAINING_BOT,
            body: 0,
            mode: PRACTICE_RANGE_MODE,
            ruleset: 0,
            weapon_buttons: vec![(0, 20)],
            owner_position: Some([1.0, 2.0, 3.0]),
            targets: None,
            unknowns: Unknowns::Refuse,
            health: 200.0,
            max_health: 200.0,
            criteria: Vec::new(),
            movement_sample: BOT_MOVEMENT_SAMPLE,
            health_changes: Vec::new(),
            aura_changes: Vec::new(),
            knockbacks: Vec::new(),
            effects: BodyEffects::default(),
            target_tags: TargetTags::default(),
            map_rays: None,
        };
        assert_eq!(
            world.native(site(PRACTICE_NAMEPLATE_PARENT), &gate),
            Some(Value::Bool(true))
        );
        assert_eq!(world.native(site(0x0a5b), &gate), None);
        let mut other_filter = gate.clone();
        other_filter["m_filter"]["m_66747E7D"] = json!(2);
        assert_eq!(
            world.native(site(PRACTICE_NAMEPLATE_PARENT), &other_filter),
            None
        );
        let mut other_owner = gate.clone();
        other_owner["m_entity"] = json!({"$":"STU_1FF67CA6"});
        assert_eq!(
            world.native(site(PRACTICE_NAMEPLATE_PARENT), &other_owner),
            None
        );
        world.hero = 0x02e0_0000_0000_0170;
        assert_eq!(world.native(site(PRACTICE_NAMEPLATE_PARENT), &gate), None);

        let possessed = json!({"$":"STU_0AC15F78","m_entity":{"$":"STU_B5A0CAF0"}});
        let position = json!({"$":"STU_71A87E92","m_entity":possessed});
        let possessor = json!({"$":"STU_295D117D","m_entity":possessed});
        assert_eq!(world.selects_ai_behavior(site(FIRING_GRAPH)), Some(true));
        assert_eq!(
            world.native(site(FIRING_GRAPH), &position),
            Some(Value::Vector([1.0, 2.0, 3.0]))
        );
        assert_eq!(
            world.native(site(FIRING_GRAPH), &possessor),
            Some(Value::Null)
        );
        assert_eq!(world.native(site(WEAPON), &position), None);
        assert_eq!(world.ai_weapon_button(0), Some(20));
        assert_eq!(world.ai_weapon_button(1), None);
        world.owner_position = None;
        assert_eq!(world.selects_ai_behavior(site(FIRING_GRAPH)), Some(false));
        assert_eq!(world.native(site(FIRING_GRAPH), &position), None);
    }

    /// The pack's Training Bot in the Practice Range, unknown content read as
    /// the Python reference did: the firing behavior runs out of sight, the
    /// stock nameplate mounts under 0C18 with the bot's name as its text and
    /// the observers' full frame carries that name; the death raises the
    /// dead flag and runs to its end.
    #[test]
    #[ignore = "requires the derived game data's bot graph bundle"]
    fn the_practice_training_bot_starts_its_behavior_and_dies() {
        let data = lucy_testkit::runtime_dir().join("data");
        let graphs =
            Arc::new(GraphStore::load_runtime(&data, "game_graphs_bots_174.json.gz").unwrap());
        let codecs = CodecTable::client();
        let mut bot = BotBody::start(
            Arc::clone(&graphs),
            spawn(Some([0.0, 0.0, 3.0])),
            Unknowns::Tolerate,
        )
        .unwrap();
        let snapshot = bot.snapshot().unwrap();
        assert!(snapshot.initialization_complete());
        let [behavior] = *bot.behaviors() else {
            panic!("one behavior: {:?}", bot.behaviors());
        };
        assert_eq!(snapshot.instances[&behavior].graph, FIRING_GRAPH);
        let nameplate = practice_nameplate(snapshot).unwrap();
        let parent = snapshot.instances[&nameplate].parent.unwrap();
        assert_eq!(
            snapshot.instances[&parent.0].graph,
            PRACTICE_NAMEPLATE_PARENT
        );
        let name = Value::String("Training Bot".into());
        assert_eq!(
            snapshot.instances[&nameplate]
                .variables
                .get(&NAMEPLATE_TEXT),
            Some(&name)
        );
        let encoder = bot.encoder(&graphs, &codecs);
        let observed = encoder
            .project(snapshot, crate::snapshot::View::Observer)
            .unwrap();
        assert_eq!(observed.instance_count(), snapshot.instances.len() - 1);
        assert_eq!(
            observed.instance_variable(nameplate, NAMEPLATE_TEXT),
            Some(&name)
        );
        let full = encoder.full(&observed).unwrap();
        let mut text = lucy_core::bits::BitWriter::new();
        crate::wire::write_value(&mut text, &name, crate::wire::ValueEncoding::Compact).unwrap();
        assert!(lucy_testkit::contains_bits(
            full.as_bytes(),
            full.bit_len(),
            text.as_bytes(),
            text.bit_len()
        ));
        for frame in 101..=160 {
            assert!(bot.advance_live(frame, Some([0.0, 0.0, 3.0])).unwrap());
        }
        assert!(bot.die(161, Variables::new()).unwrap());
        assert!(bot.behaviors().is_empty());
        // The death raises the dead flag the health bar reads (0D24 st38),
        // and the observers' frame carries it.
        let dead = bot
            .encoder(&graphs, &codecs)
            .project_next(bot.snapshot().unwrap(), &observed)
            .unwrap();
        assert_eq!(dead.entity_variable(DEAD_FLAG), Some(&Value::Bool(true)));
        assert_ne!(
            observed.entity_variable(DEAD_FLAG),
            Some(&Value::Bool(true))
        );
        let mut frames = Vec::new();
        bot.advance_death(400, 120, |frame, _| frames.push(frame))
            .unwrap();
        assert!(!frames.is_empty());
        assert!(frames.iter().all(|&frame| frame <= 161 + 120));
        let snapshot = bot.snapshot().unwrap();
        assert!(snapshot.initialization_complete());
        encoder_full(&graphs, &codecs, &bot);
    }

    /// A Training Bot made again after a death gets `0013.025` on its
    /// `0BA8`, which starts the rebuild Anim (state 1 with #7143 unset); a
    /// first spawn plays none. Either way `0BA8`'s Entry sets #7082, the
    /// kill feed gate of bot-on-bot kills.
    #[test]
    #[ignore = "requires the derived game data's bot graph bundle"]
    fn a_respawned_training_bot_rebuilds_itself() {
        let data = lucy_testkit::runtime_dir().join("data");
        let graphs =
            Arc::new(GraphStore::load_runtime(&data, "game_graphs_bots_174.json.gz").unwrap());
        let rebuild = |respawned| {
            let bot = BotBody::start(
                Arc::clone(&graphs),
                BotSpawn {
                    respawned,
                    ..spawn(None)
                },
                Unknowns::Tolerate,
            )
            .unwrap();
            let snapshot = bot.snapshot().unwrap();
            assert_eq!(
                snapshot.variables.get(&RANGE_BOT).map(crate::expr::truthy),
                Some(true)
            );
            let (_, instance) = snapshot
                .instances
                .iter()
                .find(|(_, instance)| instance.graph == REBUILD_GRAPH)
                .expect("the bot body lists 0BA8");
            [0, 1].map(|state| {
                instance
                    .states
                    .get(&state)
                    .is_some_and(|state| state.active)
            })
        };
        assert_eq!(rebuild(false), [false, false]);
        assert_eq!(rebuild(true), [false, true]);
    }

    /// The pack's Training Bot takes a Heavy Pulse Rifle hit it survives
    /// (`0011.025` with the rifle's tags, graph `0254` rid 25): its flinch
    /// graph `0E8C` sets the strength #1024 to 0.2 for tag `09A2`, with the
    /// direction #1018, and the observers' frame carries both (st17 syncs
    /// them for the clients' own flinch anim); 0.1 s later `0C17` raises one
    /// pain criteria (`0608` or `0609`) on the bot, heard by the bot and its
    /// attacker, and the flinch ends (#1024 back to 0) after its 0.1 s.
    #[test]
    #[ignore = "requires the derived game data's bot graph bundle"]
    fn the_practice_training_bot_flinches_and_raises_its_pain_criteria() {
        const FLINCH: u32 = 0x0e8c;
        const STRENGTH: u32 = 1024;
        const DIRECTION: u32 = 1018;
        // The authored 0.2 as the client's float32.
        let rifle_flinch = Value::Float(f64::from(0.2_f32));
        let data = lucy_testkit::runtime_dir().join("data");
        let graphs =
            Arc::new(GraphStore::load_runtime(&data, "game_graphs_bots_174.json.gz").unwrap());
        let codecs = CodecTable::client();
        let mut bot = BotBody::start(Arc::clone(&graphs), spawn(None), Unknowns::Tolerate).unwrap();
        let snapshot = bot.snapshot().unwrap();
        let flinch = snapshot
            .instances
            .iter()
            .find_map(|(&id, instance)| (instance.graph == FLINCH).then_some(id))
            .unwrap();
        let observed = bot
            .encoder(&graphs, &codecs)
            .project(snapshot, crate::snapshot::View::Observer)
            .unwrap();
        assert_ne!(
            observed.instance_variable(flinch, STRENGTH),
            Some(&rifle_flinch)
        );
        let rifle: crate::weapons::DamageTags = [0x03bf, 0x09a2, 0x1e1e, 0x2d2f, 0x4d29]
            .map(|tag| 0x0d80_0000_0000_0000 | tag)
            .into_iter()
            .collect();
        let hit = 130;
        bot.set_health(181.0).unwrap();
        assert!(
            bot.take_damage(
                hit,
                Variables::from([
                    (DAMAGE_TAGS, rifle.to_value()),
                    (DAMAGE_SOURCE, Value::Entity(0xa000_0101)),
                    (DAMAGE_AMOUNT, Value::Float(19.0)),
                    (DAMAGE_DIRECTION, Value::Vector([0.0, 0.0, 1.0])),
                ]),
            )
            .unwrap()
        );
        let snapshot = bot.snapshot().unwrap();
        assert!(snapshot.initialization_complete());
        let hurt = bot
            .encoder(&graphs, &codecs)
            .project_next(snapshot, &observed)
            .unwrap();
        assert_eq!(
            hurt.instance_variable(flinch, STRENGTH),
            Some(&rifle_flinch)
        );
        assert_eq!(
            hurt.instance_variable(flinch, DIRECTION),
            Some(&Value::Vector([0.0, 0.0, 1.0]))
        );
        assert!(bot.take_criteria().is_empty(), "the pain waits 0.1 s");
        let mut criteria = Vec::new();
        for frame in hit..hit + 40 {
            if bot.due_by(frame) {
                assert!(bot.advance_live(frame, None).unwrap());
            }
            criteria.extend(
                bot.take_criteria()
                    .into_iter()
                    .map(|raised| (frame, raised.identifier, raised.audience)),
            );
        }
        // 19 of the 181 left is a tenth or more (#7404 / #520, rid 3): the
        // severe pain `0609` (voice set `011D`'s "PnSvr" stimulus `0004`),
        // with `0C17`'s `191D` (more than 5 %, rid 38), which the voice set
        // doesn't answer. The message ran at the start of frame 129 (the one
        // before the hit); its 0.1 s wait (rid 20), in whole frames, is over
        // in frame 136. The pain's audience (#1839, `m_297E0B63`) is the
        // `STU_ED45C5E8` query (no entities, an assumption), then the bot
        // (rid 22) and its attacker (rid 21); `191D` names none.
        let attacker = 0xa000_0101;
        assert_eq!(
            criteria,
            [
                (hit + 6, 0x0d80_0000_0000_0609, Some(vec![ENTITY, attacker])),
                (hit + 6, 0x0d80_0000_0000_191d, None)
            ],
            "{criteria:x?}"
        );
        let settled = bot
            .encoder(&graphs, &codecs)
            .project_next(bot.snapshot().unwrap(), &hurt)
            .unwrap();
        assert_ne!(
            settled.instance_variable(flinch, STRENGTH),
            Some(&rifle_flinch)
        );

        // A Helix Rocket's explosion (`0254` rid 35, its `m_D2D11CE9` tags)
        // carries the splash tag `078D` and not `0302`, so `0E8C` (rid 13,
        // 15) also starts its networked knockback anim, st7 (alias `0174`,
        // no timer: `m_37EE887D`). Its subgraph's movement sampler (st10)
        // writes #545, the body's grounded bit; after its 0.25 s wait (st12)
        // st11 ends the anim once #545 is true. A training bot stands on the
        // ground, so the anim ends then. Before, the bot's world answered no
        // movement sample, #545 stayed unset and the clients played the anim
        // for ever.
        const KNOCKBACK: u16 = 7;
        let explosion: crate::weapons::DamageTags = [0x03bf, 0x04b1, 0x078d, 0x0980, 0x2d30]
            .map(|tag| 0x0d80_0000_0000_0000 | tag)
            .into_iter()
            .collect();
        let blast = hit + 60;
        assert!(
            bot.take_damage(
                blast,
                Variables::from([
                    (DAMAGE_TAGS, explosion.to_value()),
                    (DAMAGE_SOURCE, Value::Entity(0xa000_0101)),
                    (DAMAGE_AMOUNT, Value::Float(40.0)),
                    (DAMAGE_DIRECTION, Value::Vector([0.0, 0.0, 1.0])),
                ]),
            )
            .unwrap()
        );
        let knocked = bot
            .encoder(&graphs, &codecs)
            .project_next(bot.snapshot().unwrap(), &settled)
            .unwrap();
        assert!(knocked.state(flinch, KNOCKBACK).is_some());
        let knockback = |bot: &BotBody| {
            bot.snapshot().unwrap().instances[&flinch]
                .states
                .get(&KNOCKBACK)
                .is_some_and(|state| state.active)
        };
        let mut ended = None;
        for frame in blast..blast + 120 {
            if bot.due_by(frame) {
                assert!(bot.advance_live(frame, None).unwrap());
            }
            if !knockback(&bot) {
                ended = Some(frame);
                break;
            }
        }
        // The message ran at the start of frame `blast - 1`; the 250 ms wait
        // is over in frame `blast - 1 + ceil(250 / 16)`.
        assert_eq!(ended, Some(blast + 15));
        // The observers' frame turns it off.
        let recovered = bot
            .encoder(&graphs, &codecs)
            .project_next(bot.snapshot().unwrap(), &knocked)
            .unwrap();
        assert_ne!(
            recovered.state(flinch, KNOCKBACK),
            knocked.state(flinch, KNOCKBACK)
        );

        // A bot the knockback throws: the match gives it its mover's bits
        // from its next frame on (in the air, rising, moving) until it lands
        // 40 frames on, and runs its frame when they change, as when it is
        // due. The 0.25 s wait is only the anim's minimum: it ends on the
        // landing frame.
        const THROWN: u16 = 1 | 1 << 2 | 1 << 4;
        let throw = blast + 60;
        let landing = throw + 40;
        assert!(
            bot.take_damage(
                throw,
                Variables::from([
                    (DAMAGE_TAGS, explosion.to_value()),
                    (DAMAGE_SOURCE, Value::Entity(0xa000_0101)),
                    (DAMAGE_AMOUNT, Value::Float(40.0)),
                    (DAMAGE_DIRECTION, Value::Vector([0.0, 0.0, 1.0])),
                ]),
            )
            .unwrap()
        );
        assert!(knockback(&bot));
        let mut ended = None;
        for frame in throw..throw + 120 {
            let sample = if (throw + 1..landing).contains(&frame) {
                THROWN
            } else {
                BOT_MOVEMENT_SAMPLE
            };
            if bot.set_movement_sample(sample) || bot.due_by(frame) {
                assert!(bot.advance_live(frame, None).unwrap());
            }
            if !knockback(&bot) {
                ended = Some(frame);
                break;
            }
        }
        assert_eq!(ended, Some(landing));
        encoder_full(&graphs, &codecs, &bot);
    }

    /// The patrol `0B8E` on the Training Bot asks the walker for its route
    /// points on its own timings. The body's spawn frame 100 runs first; the
    /// behavior starts at the end of it (the clock at 1615 ms): its 2 s wait
    /// ends at 3615 ms, its 1 s wait at 4615 ms, in frame 288 (4615 / 16 =
    /// 288.4): `#39` goes from 2 to (2 + 1) % 3 = 0 and the movement state
    /// asks for route point 0. The walker reports the walk over at frame
    /// 300, taken at that frame's end (4815 ms): the point goes, the 1 s
    /// wait runs to 5815 ms, frame 363, and asks for point 1; over at 400,
    /// point 2 at 7415 ms, frame 463; over at 500, point 0 again at 9015
    /// ms, frame 563. The behavior is the server's: the observers' frames
    /// leave it out. A dead bot asks for nothing.
    #[test]
    #[ignore = "requires the derived game data's bot graph bundle"]
    fn the_patrol_asks_for_its_route_points_on_its_graphs_timings() {
        let data = lucy_testkit::runtime_dir().join("data");
        let graphs =
            Arc::new(GraphStore::load_runtime(&data, "game_graphs_bots_174.json.gz").unwrap());
        let codecs = CodecTable::client();
        let route = [[1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [3.0, 0.0, 0.0]];
        let mut bot = BotBody::start(
            Arc::clone(&graphs),
            BotSpawn {
                patrol: Some(Patrol {
                    route: &route,
                    index: 2,
                }),
                ..spawn(None)
            },
            Unknowns::Tolerate,
        )
        .unwrap();
        let snapshot = bot.snapshot().unwrap();
        assert!(snapshot.initialization_complete());
        let [patrol] = *bot.behaviors() else {
            panic!("one behavior: {:?}", bot.behaviors());
        };
        assert_eq!(snapshot.instances[&patrol].graph, PATROL_GRAPH);
        let observed = bot
            .encoder(&graphs, &codecs)
            .project(snapshot, crate::snapshot::View::Observer)
            .unwrap();
        assert_eq!(observed.instance_count(), snapshot.instances.len() - 1);
        // Runs the body as the match does (a frame when something is due)
        // up to `last`; the first frame it asks for a point, and the point.
        let next_goal = |bot: &mut BotBody, frames: std::ops::RangeInclusive<u32>| {
            for frame in frames {
                if bot.due_by(frame) {
                    assert!(bot.advance_live(frame, None).unwrap());
                }
                if let Some(goal) = bot.move_goal() {
                    return Some((frame, goal));
                }
            }
            None
        };
        let (mut asked, mut goal) = next_goal(&mut bot, 101..=600).unwrap_or_else(|| {
            let snapshot = bot.snapshot().unwrap();
            panic!(
                "no walk asked for: {:?} {:?} {:?}",
                snapshot.diagnostics,
                snapshot.instances[&patrol].variables,
                snapshot.instances[&patrol].states
            )
        });
        assert_eq!((asked, goal.target), (288, route[0]));
        assert_eq!(goal.instance, patrol);
        let mut walks = Vec::new();
        for arrived in [300, 400, 500] {
            // Still asked for until the walk is over.
            assert_eq!(
                next_goal(&mut bot, asked + 1..=arrived - 1),
                Some((asked + 1, goal))
            );
            bot.move_arrived(goal, arrived).unwrap();
            assert!(bot.due_by(arrived));
            assert!(bot.advance_live(arrived, None).unwrap());
            assert_eq!(bot.move_goal(), None);
            (asked, goal) = next_goal(&mut bot, arrived + 1..=arrived + 100).unwrap();
            walks.push((asked, goal.target));
        }
        assert_eq!(walks, [(363, route[1]), (463, route[2]), (563, route[0])]);
        assert!(bot.die(563, Variables::new()).unwrap());
        assert!(bot.behaviors().is_empty());
        assert_eq!(bot.move_goal(), None);
    }

    /// `0C17`'s death branch (rid8, server-only) takes `003E`'s tags
    /// (`003D` → #1605) and killer (`0012` → #392) and, 0.05 s after the
    /// death (frame 200; 3250 ms, in whole frames frame 204), raises `05FB`
    /// on the bot for a killing hit tagged `0644`, else `1C92` for `074E`;
    /// any other hit leaves the line to the clients' own 20 % death cry
    /// (client-only, stimulus `0001`). The kill criteria `071B` names the
    /// killer, not the bot, so the bot keeps none of it. The killing hit's
    /// damage message comes first, with the kill's fields, as the match
    /// sends it (`lucy_sim::bot_script` `BotScripts::died`): it adds no
    /// second cry, but `0C17`'s pain branch still runs (its entry rid0 on
    /// `0011`, server-only; only tag `0262` skips it, rid25): rid34 reads
    /// the health left (#520 = `STU_9B7648C3(self)`, 0 after the kill),
    /// rid35 adds the hit to the last second's damage (#7404 = 200), and
    /// 0.1 s later (rid20; frame 206) rid3 tests `#7404 / #520 < 0.1`. The
    /// VM's division by zero gives 0 (`expr::arithmetic` opcode 26), so the
    /// test holds and rid5 raises the minor pain `0608`, not rid4's severe
    /// `0609`; rid38's `> 0.05` fails, so no `191D`. The match drops every
    /// criteria of a dead bot but its death cries
    /// (`voice_lines::is_death_cry`), so no client hears it.
    #[test]
    #[ignore = "requires the derived game data's bot graph bundle"]
    fn a_dead_training_bot_raises_the_death_cry_its_killing_tags_pick() {
        const KILLER: u32 = 0xa000_0101;
        let data = lucy_testkit::runtime_dir().join("data");
        let graphs =
            Arc::new(GraphStore::load_runtime(&data, "game_graphs_bots_174.json.gz").unwrap());
        let cry = |tags: &[u64]| {
            let mut bot =
                BotBody::start(Arc::clone(&graphs), spawn(None), Unknowns::Tolerate).unwrap();
            let tags: crate::weapons::DamageTags =
                tags.iter().map(|tag| 0x0d80_0000_0000_0000 | tag).collect();
            let parameters = Variables::from([
                (DAMAGE_TAGS, tags.to_value()),
                (DAMAGE_SOURCE, Value::Entity(KILLER)),
            ]);
            let mut damage = parameters.clone();
            damage.insert(DAMAGE_AMOUNT, Value::Float(200.0));
            bot.set_health(0.0).unwrap();
            bot.take_damage(200, damage).unwrap();
            assert!(bot.die(200, parameters).unwrap());
            let mut criteria: Vec<_> = Vec::new();
            for frame in 200..=260 {
                bot.advance_death(frame, 187, |_, _| {}).unwrap();
                criteria.extend(
                    bot.take_criteria()
                        .into_iter()
                        .filter(|raised| raised.target == ENTITY)
                        .map(|raised| (frame, raised.identifier)),
                );
            }
            criteria
        };
        const PAIN: (u32, u64) = (206, 0x0d80_0000_0000_0608);
        assert_eq!(cry(&[0x0644]), [(204, 0x0d80_0000_0000_05fb), PAIN]);
        assert_eq!(cry(&[0x03bf, 0x074e]), [(204, 0x0d80_0000_0000_1c92), PAIN]);
        assert_eq!(cry(&[0x03bf, 0x09a2]), [PAIN]);
    }

    /// Biotic Grenade's aura (`053:0267`) on the pack's Training Bot: its
    /// graph `0C71` plays the enemy aura `0C72` on a bot of the other team
    /// than the thrower, whose ModifyHealth (st0) deals 60 from the thrower
    /// at once, and the ally aura `0B99` on a bot of his team, which heals
    /// 100.
    #[test]
    #[ignore = "requires the derived game data's bot graph bundle"]
    fn the_biotic_grenade_aura_damages_an_enemy_bot_and_heals_an_ally() {
        const BIOTIC_GRENADE: u64 = 0x04a0_0000_0000_0267;
        const ANA: u32 = 0xa000_0101;
        let data = lucy_testkit::runtime_dir().join("data");
        let graphs =
            Arc::new(GraphStore::load_runtime(&data, "game_graphs_bots_174.json.gz").unwrap());
        let changes = |thrower_team: u8| {
            let mut bot =
                BotBody::start(Arc::clone(&graphs), spawn(None), Unknowns::Tolerate).unwrap();
            let body = |entity, team| TargetCandidate {
                entity,
                team: Some(team),
                dead: false,
                controller: Some(entity & !0xff),
                player: Some(entity & !0xff),
                ai: false,
                ai_control: None,
                feet: [0.0; 3],
                height: 1.8,
                radius: 0.4,
                yaw: 0.0,
                tags: None,
                health: None,
                hardpoints: None,
                spawned: Vec::new(),
            };
            bot.set_target_candidates(&[body(ENTITY, 1), body(ANA, thrower_team)]);
            let landed = 130;
            let aura = AuraChange::Apply {
                source: None,
                target: ENTITY,
                aura: BIOTIC_GRENADE,
                instigator: ANA,
                creator: ANA,
                parameters: Vec::new(),
            };
            assert!(bot.change_aura(landed, &aura).unwrap());
            let mut changes: Vec<_> = bot
                .take_health_changes()
                .into_iter()
                .map(|change| (landed, change))
                .collect();
            for frame in landed..landed + 300 {
                if bot.due_by(frame) {
                    bot.advance_live(frame, None).unwrap();
                }
                changes.extend(
                    bot.take_health_changes()
                        .into_iter()
                        .map(|change| (frame, change)),
                );
            }
            let snapshot = bot.snapshot().unwrap();
            assert!(snapshot.initialization_complete());
            changes
                .into_iter()
                .map(|(frame, change)| {
                    (
                        frame,
                        change.target,
                        change.instigator,
                        change.amount,
                        change.heal,
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(changes(0), [(130, ENTITY, ANA, 60.0, false)]);
        assert_eq!(changes(1), [(130, ENTITY, ANA, 100.0, true)]);
    }

    fn encoder_full(graphs: &GraphStore, codecs: &CodecTable, bot: &BotBody) {
        let encoder = bot.encoder(graphs, codecs);
        let projected = encoder
            .project(bot.snapshot().unwrap(), crate::snapshot::View::Observer)
            .unwrap();
        encoder.full(&projected).unwrap();
    }
}
