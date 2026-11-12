//! Match entity baselines and the shared mode presentation frame.
//!
//! Client research (1.74):
//!
//! - Component 26 holds the filter bits: `0x800000 << team` picks the team's
//!   physics proxy. The client's own session builder gives its player
//!   `0x10040001 | team bit` (0x7FF78973950E), but 0x10000000 is the
//!   spectator bit (GetEntityIsSpectator, 0x7FF789B2F000): a player with it
//!   gets the spectator HUD, and a body with only it has no collision and no
//!   enemy outline. So a player gets `0x00040001 | team bit`. The client
//!   knows five teams, bits 23-27 (GetTeam 0x7FF7894E9570); team 4 is
//!   free-for-all, whose members are all enemies (0x7FF789B2C690). Bit 28 is
//!   the spectator.
//! - The low filter bits say what kind of entity it is. The graphs' entity
//!   filters (`STUConfigVarFilter`) take an entity whose bits meet
//!   `m_84065FD1` and leave out one whose bits meet `m_4EA686DB` (the native
//!   test, leafall903/ORPHEUS (MIT) `CHANGELOG.md` "node 231 filter"). Read
//!   from their uses in 104319: 0x1 is a hero (the Biotic Field `0258`
//!   heals it, Deadeye `022F` and the Pulse Bomb `0034` take it, Ana's rifle
//!   `0B94` nodes 122 and 126 count an enemy or ally hero hit), 0x40 a body
//!   that can be hit (quick melee `0043` searches for 0x40, then tests its
//!   finds for an enemy 0x1, so a hero carries both). Nearly every hit and
//!   heal search leaves out 0x10, taken as the dead bit (as upstream's
//!   `junkenstein_ai.py` `enemy_filter_bits`: team, 1, and 0x10 when dead).
//! - The client's own ray tests (a shot, a projectile's sweep, a spray) ask
//!   for the same flag word on each candidate entity: leafall903/ORPHEUS
//!   (MIT) `docs/COMMUNICATION_WHEEL.txt` ran the spray query natively;
//!   `1B60A50` requires a candidate's flags (entity `+270`) to overlap the
//!   query's allow mask, `0x0001000000000040`, and miss its deny mask. The
//!   weapons' ray profiles (`097`, `STU_8EFC337B`: `m_BC69BB40` allow,
//!   `m_5CE47069` deny) allow only bit 48 on nearly every one (`0001`,
//!   `0015`, `0019`, `002A`, …, the Helix Rocket's and Ana's darts'
//!   included) and deny 0x10 (the dead bit above), 0x4 or 0x80; the Biotic
//!   Field's throw (`011D`, deny 0x5) passes through heroes (0x1). So the
//!   word is the one the graphs filter, and a body a weapon hits needs bit
//!   48, which no graph filter can name (their masks stop at `0x3FFFFFFF`).
//! - So every body, a player's or a training bot's (the bots are heroes,
//!   `02E0…016B`-`016E`), gets its team bit and [`BODY_KINDS`]
//!   ([`body_filter`]); a body that dies gets [`DEAD`] too, with the pools
//!   of its death. Checked in the game: with its team bit alone, and then
//!   with the team bit and 0x41, a Training Bot was not hit by the client's
//!   own shots (Soldier: 76's bullets and Ana's darts left a hole and the
//!   map's impact sound on the wall behind it, Helix Rockets flew through
//!   it), though the server hit it. Assumptions: that the candidate's flags
//!   are component 26's bits (no client code read; that the client's
//!   `0xB8BDF0` ORs more bits into them is known, from what is not), and
//!   that the retail server set 0x10 at a death (the Python reference sent
//!   the team bit alone and never changed it). Not explained:
//!   ow1-preservation's bots, with `0x11040001` and no bit 48, showed
//!   impact sparks on their body (its run 343).
//! - Component 29 (MPlayerComponentData): the player's 16-byte id, BattleTag
//!   (the client names the entity by it), level, and on a competitive card
//!   the rank (m_rankedLevel, m_heroicRank, m_rankedLevelTier), which the team
//!   list shows (0x7FF789A16620).
//! - Component 74 (hero select participant) names the portrait frame; the
//!   team list draws a frame only when its asset is loaded (0x7FF78A64BA30),
//!   and the client loads it when a player entity's 74 names it
//!   (0x7FF7894C9D90). A list entry is filled again whenever a card arrives
//!   (UI-world event 0x47), so each card goes once more after the frames
//!   loaded. On a role queue card 74's m_roleQueueRole makes hero select
//!   refuse heroes of the other roles (CanPlayHero 0x7FF789CB0F92, while the
//!   global 0x7FF78BD4FB90 is on, which it is from the start).
//! - A training bot's body has no component 123 (the hero): the client fails
//!   the whole frame on a component it cannot place (assert 0xD62712AE,
//!   "Failed to deserialize component update ... 143E ... /123"; 143E has 1,
//!   3, 4, 22, 24, 25, 26, 28, 31, 35, 39, 43, 47, 49, 51, 69, 76, 78, 83, 86,
//!   90, 97, 115, 119, 122, 127).
//! - The elimination label and the kill feed name the victim by
//!   `STU_CD7F3617`, which finds the body's controller (its component 44,
//!   or the reverse table component 44 builds) and reads the controller's
//!   name; a bot body has no name component of its own. So each training
//!   bot gets a stand-in controller entity ([`bot_controller_create`]).

use std::{collections::HashMap, io};

use crate::{
    body::yaw_rotation,
    bots::{BOT_CONTROLLER_ENTITY, BOT_ENTITY, Bot},
    instance::FREE_FOR_ALL_TEAM,
};
use orpheus_content::{
    maps::{GameMap, SpawnChoice},
    stats::GMP_STATS,
};
use orpheus_core::bits::BitWriter;
use orpheus_script::{
    graph::GraphStore,
    vitals::health,
    wire::{self, Instance, Layout, Variables},
};
use orpheus_wire::{
    entity::{
        Component, ComponentValue, Create, EntityUpdate, GAME_MODE_ENTITY, Operation, PLAYER_PATH,
        SkinTheme, WITH_PARTICIPANT, WITH_SCALE, WITH_TRANSFORM,
    },
    jam::codec::Fields,
};

const PLAYER_DEFINITION: u64 = 0x0400_0000_0000_000e;
const GAME_MODE_DEFINITION: u64 = 0x0400_0000_0000_16b2;
const PLAYER_FILTER: u64 = 0x0004_0001;
const TEAM_BIT: u64 = 0x0080_0000;
/// The kind bits of a living body (see the module notes): a hero (0x1) that
/// can be hit (0x40), and that the weapons' ray tests take (bit 48).
pub const BODY_KINDS: u64 = 0x0001_0000_0000_0041;
/// The filter bit of a dead body (see the module notes).
pub const DEAD: u64 = 0x10;
const MODE_UI: u32 = 0x0cb6;
const MODE_UI_STATE: usize = 21;
/// The training bots' stand-in accounts: `BOT_ACCOUNT + slot`, with the
/// lobby's account id tag (`login.rs` `ID_HIGH_TAG`). Real accounts are
/// `0x1000_0000 | crc` (`accounts.rs` `account_id_for`), AI heroes count
/// down from `u32::MAX` (`game_service/ai.rs`).
const BOT_ACCOUNT: u32 = 0xffff_f000;
const ACCOUNT_HIGH: u64 = 1 << 56;
/// The stand-in controller of the bot in `slot` (one per slot, kept across
/// respawns): [`BOT_CONTROLLER_ENTITY`] + slot, below the bots' own ids.
pub fn bot_controller_entity(slot: usize) -> io::Result<u32> {
    u32::try_from(slot)
        .ok()
        .filter(|&slot| slot < BOT_ENTITY - BOT_CONTROLLER_ENTITY)
        .map(|slot| BOT_CONTROLLER_ENTITY + slot)
        .ok_or_else(|| invalid("too many training bots for their controller ids"))
}

fn possession_link(body: u32) -> Component {
    fields(44, vec![Some(ComponentValue::Entity(body)); 3])
}

/// The create of the bot's stand-in controller in `slot`, linked to its
/// current body, so the client can name the bot in "ELIMINATED <name>" and
/// the kill feed (see the module notes; the Python reference sent none). A
/// player entity (definition `000E`, as players have): component 26 the
/// bot's team bit only, 29 a stand-in account and the bot's name, 44 the
/// body. A player's create flags without the participant one, and no 74, so
/// it stays out of the team list. Checked in game: the bots are named in
/// "ELIMINATED" and the kill feed.
pub fn bot_controller_create(bot: &Bot, slot: usize) -> io::Result<EntityUpdate> {
    use ComponentValue::*;
    let entity = bot_controller_entity(slot)?;
    let account = BOT_ACCOUNT + (entity - BOT_CONTROLLER_ENTITY);
    let mut components = vec![
        fields(26, vec![Some(Id(team_bit(bot.spawn.team)?))]),
        fields(
            29,
            vec![
                Some(DbId(u64::from(account), ACCOUNT_HIGH)),
                Some(Text(bot.spawn.name.clone())),
            ],
        ),
    ];
    components.push(possession_link(bot.entity));
    let mut definition = Create::new(PLAYER_DEFINITION, components);
    definition.flags = PLAYER_PATH | WITH_TRANSFORM | WITH_SCALE;
    Ok(create(entity, definition))
}

/// The respawned bot's controller in `slot` now controls its new body: its
/// component 44 again.
pub fn bot_controller_link(bot: &Bot, slot: usize) -> io::Result<EntityUpdate> {
    Ok(EntityUpdate {
        entity: bot_controller_entity(slot)?,
        operation: Some(Operation::Update(vec![possession_link(bot.entity)])),
        ..EntityUpdate::default()
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Possession {
    pub body: u32,
    pub hero: u64,
    pub skin: Option<u64>,
}

impl Possession {
    pub fn components(self) -> [Component; 2] {
        [
            fields(44, vec![Some(ComponentValue::Entity(self.body)); 3]),
            fields(
                75,
                vec![
                    Some(ComponentValue::Id(self.hero)),
                    self.skin.map(ComponentValue::Id),
                    None,
                    Some(ComponentValue::Id(self.hero)),
                ],
            ),
        ]
    }
}

pub struct PlayerAppearance<'a> {
    pub entity: u32,
    pub slot: i8,
    pub team: u8,
    /// The n-th player of the team takes the team's n-th spawn point; in a
    /// free-for-all the n-th player of the match (match.py Player).
    pub spawn: usize,
    /// The spawn rooms the player's team uses now (the match flow's).
    pub room: SpawnChoice,
    pub card: &'a PlayerCard,
}

#[derive(Debug, Default)]
pub struct PlayerCard {
    pub account: [u64; 2],
    pub battle_tag: String,
    pub level: Option<u32>,
    pub portrait_frame: Option<u64>,
    pub ranked: Option<(i16, i16, i8)>,
    pub role: Option<i8>,
    pub skins: HashMap<u64, SkinTheme>,
    /// The lobby's player record, the 20308 card every client of the match
    /// gets (ow174/content/player.py `record`). Empty: ids and name only.
    pub record: Fields,
}

pub fn player_create(
    map: &GameMap,
    player: &PlayerAppearance<'_>,
    possession: Option<Possession>,
    owner_board: Option<Component>,
) -> io::Result<EntityUpdate> {
    use ComponentValue::*;
    let card = player.card;
    let mut components = vec![
        fields(
            26,
            vec![Some(Id(PLAYER_FILTER | player_team_bit(map, player.team)?))],
        ),
        fields(
            29,
            vec![
                Some(DbId(card.account[0], card.account[1])),
                (!card.battle_tag.is_empty()).then(|| Text(card.battle_tag.clone())),
                card.level.map(U32),
                None,
                None,
                None,
                None,
                card.ranked.map(|rank| I16(rank.0)),
                card.ranked.map(|rank| I16(rank.1)),
                card.ranked.map(|rank| I8(rank.2)),
            ],
        ),
        fields(
            74,
            vec![
                card.portrait_frame.filter(|&guid| guid != 0).map(Id),
                None,
                Some(I8(player.slot)),
                None,
                None,
                card.role.filter(|&role| role != 0).map(I8),
                Some(U8(1)),
                Some(U8(1)),
            ],
        ),
    ];
    if let Some(possession) = possession {
        components.extend(possession.components());
    }
    if let Some(board) = owner_board {
        if board.index != GMP_STATS {
            return Err(invalid("the player board requires component 64"));
        }
        components.push(board);
    }
    let mut definition = Create::new(PLAYER_DEFINITION, components);
    definition.flags = WITH_PARTICIPANT | PLAYER_PATH | WITH_TRANSFORM | WITH_SCALE;
    Ok(create(player.entity, definition))
}

/// The player entity's spawn room flag alone: component 74 field 6, which
/// the client checks before it opens hero select ("You must be in a spawn
/// room to change heroes", `map_areas` module notes). The player's create
/// sends it set.
pub fn spawn_room_flag(player: u32, inside: bool) -> EntityUpdate {
    let mut values = vec![None; 7];
    values[6] = Some(ComponentValue::U8(u8::from(inside)));
    EntityUpdate {
        entity: player,
        operation: Some(Operation::Update(vec![fields(74, values)])),
        ..EntityUpdate::default()
    }
}

pub fn mode_create(mode: u64) -> EntityUpdate {
    create(
        GAME_MODE_ENTITY,
        Create::new(
            GAME_MODE_DEFINITION,
            vec![fields(114, vec![None, Some(ComponentValue::Id(mode))])],
        ),
    )
}

/// The match freezes each bot's pose and pools when it queues the create.
pub fn bot_create(
    bot: &Bot,
    vitals: Option<&orpheus_script::vitals::Vitals>,
) -> io::Result<EntityUpdate> {
    let pools = vitals.map_or_else(
        || {
            BOT_PLATE.pools(health(
                bot.spawn.health as f32,
                bot.spawn.health as f32,
                0.0,
                0.0,
            ))
        },
        |vitals| BOT_PLATE.pools(vitals.component()),
    );
    let mut definition = Create::new(bot.spawn.body, vec![pools, bot_filter(bot)?]);
    definition.position = bot.position.map(|position| position as f32);
    definition.rotation = yaw_rotation(bot.yaw.round_ties_even());
    Ok(create(bot.entity, definition))
}

/// Build the initial world before the local player picks a hero; each bot's
/// stand-in controller right after its body.
pub fn initial_world(
    map: &GameMap,
    player: &PlayerAppearance<'_>,
    board: Component,
    bots: &[Bot],
) -> io::Result<Vec<EntityUpdate>> {
    let mut updates = Vec::with_capacity(map.placeables.len() + 2 + 2 * bots.len());
    updates.extend(map.placeables.iter().map(|&index| EntityUpdate {
        entity: 0x8000_0000 | index,
        operation: Some(Operation::Placeable(index)),
        ..EntityUpdate::default()
    }));
    updates.push(player_create(map, player, None, Some(board))?);
    updates.push(mode_create(map.mode_guid));
    for (slot, bot) in bots.iter().enumerate() {
        updates.push(bot_create(bot, None)?);
        updates.push(bot_controller_create(bot, slot)?);
    }
    Ok(updates)
}

/// H=0 game mode graph with its client-only UI entry dependencies retained.
pub fn mode_ui(graphs: &GraphStore, variables: &Variables) -> io::Result<BitWriter> {
    let graph = graphs
        .graph(MODE_UI)
        .ok_or_else(|| invalid("missing game mode UI graph 0CB6"))?;
    let state = graph
        .state(MODE_UI_STATE)
        .filter(|node| node.class == "STU_9D7BF987")
        .ok_or_else(|| invalid("missing verified mode UI shared-variable state"))?;
    let slot = graph
        .remote_positions()
        .iter()
        .position(|&pos| pos == state.pos)
        .ok_or_else(|| invalid("mode UI shared-variable state has no remote slot"))?;
    let mut instance = Instance::new(1, Layout::try_from(graph).map_err(invalid)?, None);
    let mut active = BitWriter::new();
    // The verified 9D7BF987 remote-full layout has X/A bits and no payload.
    active.bits(3, 2).map_err(invalid)?;
    instance.states.insert(slot, active);
    wire::remote_full(&[instance], variables).map_err(invalid)
}

fn create(entity: u32, definition: Create) -> EntityUpdate {
    EntityUpdate {
        entity,
        operation: Some(Operation::Create(Box::new(definition))),
        ..EntityUpdate::default()
    }
}

fn fields(index: u8, fields: Vec<Option<ComponentValue>>) -> Component {
    Component { index, fields }
}

pub(crate) fn team_bit(team: u8) -> io::Result<u64> {
    if team > FREE_FOR_ALL_TEAM {
        return Err(invalid("invalid team for a player or bot"));
    }
    Ok(TEAM_BIT << team)
}

/// The filter bit of a player's team; in a free-for-all the free-for-all
/// team's, since each player's own team number would run past the five teams
/// into the spectator bit (match.py Player.team_bit).
pub(crate) fn player_team_bit(map: &GameMap, team: u8) -> io::Result<u64> {
    team_bit(if map.free_for_all {
        FREE_FOR_ALL_TEAM
    } else {
        team
    })
}

/// A body's filter bits (component 26): its team bit with [`BODY_KINDS`],
/// and [`DEAD`] once it died (see the module notes).
pub fn body_filter(team_bit: u64, dead: bool) -> u64 {
    team_bit | BODY_KINDS | if dead { DEAD } else { 0 }
}

/// The filter bits component of a body.
pub fn filter_component(bits: u64) -> Component {
    fields(26, vec![Some(ComponentValue::Id(bits))])
}

/// A training bot's filter bits component now: dead or alive.
pub fn bot_filter(bot: &Bot) -> io::Result<Component> {
    Ok(filter_component(body_filter(
        team_bit(bot.spawn.team)?,
        bot.dead,
    )))
}

/// A debugging switch: `ORPHEUS_DEBUG_BOT_PLATE` lists changes to what the
/// clients learn about the training bots, one input of their nameplate or
/// health bar each, to find which one makes the client draw the bar with
/// black segment outlines and a mark before the name ("▌TRAINING BOT";
/// retail has neither, `docs/notes/behavior.md`). Leaving out the hero bit
/// and the stand-in controllers changed nothing (2026-11-12). The bar's
/// presenter (`0D24` st51, layout `019C.116`) gets the body itself and
/// fills most of its fields natively, from the body; the name's (st42,
/// `012B.116`) gets its text, a meter and a status icon from what the
/// server sends:
/// - `no-name`: no text override #8812 ([`orpheus_script::bot::NAMEPLATE_TEXT`],
///   a string, our assumption); the name then comes from the stand-in
///   controller (`#17476 = #8812 || STU_CD7F3617(…)`).
/// - `meter=<number>`: entity variable #6036, which `0D24` shares: the
///   name's meter (`3523`) shows `1 - #6036e`; other than 0 it also takes
///   `0D24` st4's first case, another status icon.
/// - `status`: entity variable #2143 true: the status icon (`23D1`) `25E9`
///   instead of `654A` (`0D24` st5).
/// - `pool-id=<n>`, `pool-kind=<n>`, `pool-tail=<a>.<b>.<c>.<d>`: each pool's
///   id, kind and last four bytes (`orpheus_wire::entity::pool`: 1, 0 and
///   0.1.0.1, unverified).
#[derive(Debug, Default, PartialEq)]
pub struct BotPlate {
    pub no_name: bool,
    pub meter: Option<f64>,
    pub status: bool,
    pool_id: Option<u32>,
    pool_kind: Option<u32>,
    pool_tail: Option<[u8; 4]>,
}

pub static BOT_PLATE: std::sync::LazyLock<BotPlate> = std::sync::LazyLock::new(|| {
    let Ok(list) = std::env::var("ORPHEUS_DEBUG_BOT_PLATE") else {
        return BotPlate::default();
    };
    let plate = BotPlate::parse(&list);
    tracing::warn!("debugging: training bots changed by {plate:?}");
    plate
});

impl BotPlate {
    fn parse(list: &str) -> Self {
        let mut plate = Self::default();
        for item in list
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
        {
            let (name, value) = item.split_once('=').unwrap_or((item, ""));
            let known = match name {
                "no-name" => {
                    plate.no_name = true;
                    true
                }
                "status" => {
                    plate.status = true;
                    true
                }
                "meter" => value.parse().map(|meter| plate.meter = Some(meter)).is_ok(),
                "pool-id" => value.parse().map(|id| plate.pool_id = Some(id)).is_ok(),
                "pool-kind" => value
                    .parse()
                    .map(|kind| plate.pool_kind = Some(kind))
                    .is_ok(),
                "pool-tail" => value
                    .split('.')
                    .map(str::parse)
                    .collect::<Result<Vec<u8>, _>>()
                    .ok()
                    .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
                    .map(|tail| plate.pool_tail = Some(tail))
                    .is_some(),
                _ => false,
            };
            if !known {
                tracing::warn!("ORPHEUS_DEBUG_BOT_PLATE: unknown change {item:?}");
            }
        }
        plate
    }

    /// The bots' entity variables this switch sets when their script
    /// starts.
    pub fn entity_variables(&self) -> Vec<(u32, wire::Value)> {
        let mut variables = Vec::new();
        if let Some(meter) = self.meter {
            variables.push((METER, wire::Value::Float(meter)));
        }
        if self.status {
            variables.push((STATUS, wire::Value::Bool(true)));
        }
        variables
    }

    /// A bot's pools component (51) with this switch's changes; a cleared
    /// slot (id 0) stays cleared.
    pub fn pools(&self, mut pools: Component) -> Component {
        for value in pools.fields.iter_mut().flatten() {
            let ComponentValue::Blob20(bytes) = value else {
                continue;
            };
            if bytes[..4] == [0; 4] {
                continue;
            }
            if let Some(id) = self.pool_id {
                bytes[..4].copy_from_slice(&id.to_le_bytes());
            }
            if let Some(kind) = self.pool_kind {
                bytes[12..16].copy_from_slice(&kind.to_le_bytes());
            }
            if let Some(tail) = self.pool_tail {
                bytes[16..].copy_from_slice(&tail);
            }
        }
        pools
    }
}

/// `0D24`'s shared entity variable #6036: the name's meter shows
/// `1 - #6036e` (st17). `0E8F` writes it for a dead player's body.
const METER: u32 = 6036;
/// `0D24`'s shared entity variable #2143: its status icon is `25E9` when
/// it is true, else `654A` (st5).
const STATUS: u32 = 2143;

fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bots::{BOT_CONTROLLER_ENTITY, BOT_ENTITY, BotCatalog, BotWalk};
    use orpheus_content::stats::{PlayerStats, StatsCatalog};
    use orpheus_physics::mover::settings::MoverCatalog;
    use orpheus_script::wire::Value;
    use orpheus_wire::entity::{FrameCandidates, MAX_PAYLOAD, frame, select_frame};
    use sha2::{Digest, Sha256};

    fn runtime() -> std::path::PathBuf {
        orpheus_testkit::runtime_dir().join("data")
    }

    #[test]
    #[ignore = "requires the derived 104319 spawn and stat catalogs"]
    fn matches_the_python_practice_range_create_frame_and_budget() {
        let data = runtime();
        // mover.py `SOLDIER_BODY`, the fallback body.
        let movers =
            MoverCatalog::load(data.join("game_movers_174.json"), 0x0400_0000_0000_03cf).unwrap();
        let bots = BotCatalog::load(data.join("ai_spawners_174.json")).unwrap();
        let stats = StatsCatalog::load(data.join("game_stats_174.json")).unwrap();
        let map = GameMap::practice_range();
        let bots = bots.make(map.map_guid, |body| {
            BotWalk::from_mover(movers.for_body(Some(body)))
        });
        let card = PlayerCard {
            account: [0x1234_5678, 0x0100_0000_0000_0000],
            battle_tag: "rust#2345".into(),
            level: Some(3000),
            portrait_frame: Some(0x0250_0000_0000_0f97),
            ranked: Some((3200, 120, 4)),
            role: Some(1),
            ..PlayerCard::default()
        };
        let player = PlayerAppearance {
            entity: 0xa000_0100,
            slot: 0,
            team: 1,
            spawn: 0,
            room: SpawnChoice::First,
            card: &card,
        };
        let board = stats.component(&PlayerStats::default(), map.mode_guid, None, None);
        // The Python reference's world has no stand-in controllers: each
        // bot's controller comes right after its body here. Its bots' filter
        // bits were their team bit alone; ours add the body's kind (module
        // notes). The other records match the Python frame byte for byte.
        let updates = initial_world(&map, &player, board, &bots).unwrap();
        let controllers = BOT_CONTROLLER_ENTITY..BOT_ENTITY;
        let python: Vec<_> = updates
            .iter()
            .filter(|update| !controllers.contains(&update.entity))
            .cloned()
            .map(|mut update| {
                let bot = bots.iter().find(|bot| bot.entity == update.entity);
                if let (Some(bot), Some(Operation::Create(create))) = (bot, &mut update.operation) {
                    let team = team_bit(bot.spawn.team).unwrap();
                    let filter = &mut create.components[1];
                    assert_eq!(*filter, filter_component(team | BODY_KINDS));
                    *filter = filter_component(team);
                }
                update
            })
            .collect();
        let expected: Vec<_> = [
            0x8000_0088,
            0x8000_022e,
            0x8000_0234,
            0x8000_0289,
            player.entity,
            GAME_MODE_ENTITY,
        ]
        .into_iter()
        .chain(BOT_ENTITY..BOT_ENTITY + 15)
        .collect();
        assert_eq!(
            python
                .iter()
                .map(|update| update.entity)
                .collect::<Vec<_>>(),
            expected
        );
        let with_controllers: Vec<_> = expected[..6]
            .iter()
            .copied()
            .chain((0..15).flat_map(|slot| [BOT_ENTITY + slot, BOT_CONTROLLER_ENTITY + slot]))
            .collect();
        assert_eq!(
            updates
                .iter()
                .map(|update| update.entity)
                .collect::<Vec<_>>(),
            with_controllers
        );
        let encoded = frame(1234, 1234, &[], &[], &python.iter().collect::<Vec<_>>()).unwrap();
        assert_eq!(encoded.len(), 1674);
        assert_eq!(
            format!("{:x}", Sha256::digest(&encoded)),
            "b82987cb93e2e93851a16452fbb2230637b89edd70cf9f07b9a34c054689d66f",
        );
        let mut remaining = updates.as_slice();
        let mut transmitted = Vec::new();
        while !remaining.is_empty() {
            let selected = select_frame(
                FrameCandidates {
                    tick: 1234,
                    reliable: &[],
                    unreliable: &[],
                    queued: remaining,
                    movements: &[],
                    owner_correction: false,
                    reliable_only: false,
                },
                |_| 1234,
            )
            .unwrap();
            assert!(selected.payload.len() <= MAX_PAYLOAD && !selected.entities.is_empty());
            transmitted.extend(selected.entities.iter().map(|update| update.entity));
            remaining = &remaining[selected.entities.len()..];
        }
        assert_eq!(transmitted, with_controllers);
    }

    /// Two fixture bots: a Training Bot body (`1403`) on team 1, and
    /// another body on team 0.
    fn fixture_bots() -> Vec<Bot> {
        BotCatalog::from_json(
            r#"{
                "heroes": {
                    "0x02E000000000016B": {"name":"Training Bot","body":"0x0400000000001403","health":200},
                    "0x02E0000000000002": {"name":"Other Bot","body":"0x04000000000003CF","health":200}
                },
                "maps": {"0x0800000000000688": {"hints": {}, "spawners": [
                    {"hero":"0x02E000000000016B","team":1,
                     "position":[1,2,3],"rotation":[0,0,0,1],"graphs":[]},
                    {"hero":"0x02E0000000000002","team":0,
                     "position":[0,0,0],"rotation":[0,0,0,1],"graphs":[]}
                ]}}
            }"#,
        )
        .unwrap()
        .make(GameMap::practice_range().map_guid, |_| {
            BotWalk::from_mover(&Default::default())
        })
    }

    fn created(update: &EntityUpdate) -> &Create {
        match &update.operation {
            Some(Operation::Create(create)) => create,
            other => panic!("not a create: {other:?}"),
        }
    }

    fn link(body: u32) -> Component {
        fields(44, vec![Some(ComponentValue::Entity(body)); 3])
    }

    /// The player stand-in: definition 000E with the bot's team bit, a
    /// stand-in account with the bot's name, and its body; a player's flags
    /// without the participant one, and no 74 (the team list's).
    #[test]
    fn a_player_stand_in_names_the_bot_and_controls_its_body() {
        use ComponentValue::*;
        let bots = fixture_bots();
        let update = bot_controller_create(&bots[0], 0).unwrap();
        assert_eq!(update.entity, BOT_CONTROLLER_ENTITY);
        let create = created(&update);
        assert_eq!(create.definition, PLAYER_DEFINITION);
        assert_eq!(create.flags, PLAYER_PATH | WITH_TRANSFORM | WITH_SCALE);
        assert_eq!(create.flags & WITH_PARTICIPANT, 0);
        assert_eq!(
            create.components,
            [
                fields(26, vec![Some(Id(TEAM_BIT << 1))]),
                fields(
                    29,
                    vec![
                        Some(DbId(u64::from(BOT_ACCOUNT), 1 << 56)),
                        Some(Text("Training Bot".into())),
                    ],
                ),
                link(BOT_ENTITY),
            ]
        );
        let other = bot_controller_create(&bots[1], 1).unwrap();
        assert_eq!(other.entity, BOT_CONTROLLER_ENTITY + 1);
        let Some(Some(DbId(account, _))) = created(&other).components[1].fields.first() else {
            panic!("no account");
        };
        assert_eq!(*account, u64::from(BOT_ACCOUNT) + 1);
        // The wire takes every component.
        frame(1234, 1234, &[], &[], &[&update, &other]).unwrap();
    }

    /// A bot's body is a hero that can be hit, that the weapons' ray tests
    /// take (bit 48), on its team (module notes): its create's filter bits;
    /// a bot created dead also says so.
    #[test]
    fn a_bot_body_is_a_hero_that_can_be_hit_until_it_dies() {
        let mut bots = fixture_bots();
        let filter = |bot: &Bot| created(&bot_create(bot, None).unwrap()).components[1].clone();
        assert_eq!(
            filter(&bots[0]),
            fields(26, vec![Some(ComponentValue::Id(0x0001_0000_0100_0041))])
        );
        assert_eq!(
            filter(&bots[1]),
            fields(26, vec![Some(ComponentValue::Id(0x0001_0000_0080_0041))])
        );
        bots[0].died(10);
        assert_eq!(
            filter(&bots[0]),
            fields(26, vec![Some(ComponentValue::Id(0x0001_0000_0100_0051))])
        );
    }

    /// Each bot's controller comes right after its body in the initial
    /// world; a respawned body's link is the controller's component 44
    /// again.
    #[test]
    fn the_initial_world_and_a_respawn_carry_the_controllers() {
        let bots = fixture_bots();
        let map = GameMap::practice_range();
        let card = PlayerCard::default();
        let player = PlayerAppearance {
            entity: 0xa000_0100,
            slot: 0,
            team: 0,
            spawn: 0,
            room: SpawnChoice::First,
            card: &card,
        };
        let board = Component {
            index: GMP_STATS,
            fields: Vec::new(),
        };
        let entities: Vec<u32> = initial_world(&map, &player, board, &bots)
            .unwrap()
            .iter()
            .map(|update| update.entity)
            .skip(map.placeables.len() + 2)
            .collect();
        assert_eq!(
            entities,
            [
                BOT_ENTITY,
                BOT_CONTROLLER_ENTITY,
                BOT_ENTITY + 1,
                BOT_CONTROLLER_ENTITY + 1
            ]
        );

        let fresh = bots[0].respawned(BOT_ENTITY + 2, 0);
        assert_eq!(
            bot_controller_link(&fresh, 0).unwrap(),
            EntityUpdate {
                entity: BOT_CONTROLLER_ENTITY,
                operation: Some(Operation::Update(vec![link(BOT_ENTITY + 2)])),
                ..EntityUpdate::default()
            }
        );
        assert!(bot_controller_entity((BOT_ENTITY - BOT_CONTROLLER_ENTITY) as usize).is_err());
    }

    #[test]
    #[ignore = "requires the derived 104319 graph bundle"]
    fn mode_ui_preserves_its_shared_variable_state_with_or_without_countdown() {
        let graphs = GraphStore::load(runtime().join("game_graphs_174.json.gz")).unwrap();
        let cases = [
            (Variables::new(), 38, "085b06000c"),
            (
                Variables::from([(15545, Value::Float(3.0))]),
                74,
                "085b0600cbe5a901c000",
            ),
        ];
        for (vars, length, expected) in cases {
            let frame = mode_ui(&graphs, &vars).unwrap();
            let actual: String = frame
                .as_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            assert_eq!((frame.bit_len(), actual.as_str()), (length, expected));
        }
    }

    /// `ORPHEUS_DEBUG_BOT_PLATE`: the pool changes go into every pool the
    /// bots send and leave a cleared slot cleared; an unknown change is
    /// skipped.
    #[test]
    fn the_bot_plate_switch_rewrites_the_bots_pools() {
        use orpheus_wire::entity::{pool, pool_component};
        let plate = BotPlate::parse("pool-id=3, pool-kind=2,pool-tail=1.0.1.0,meter=1,bogus");
        assert_eq!(plate.meter, Some(1.0));
        assert!(!plate.no_name && !plate.status);
        let pools = plate.pools(pool_component([
            (0, pool(1, 0, 200.0, 150.0)),
            (1, [0; 20]),
        ]));
        let mut changed = pool(3, 2, 200.0, 150.0);
        changed[16..].copy_from_slice(&[1, 0, 1, 0]);
        assert_eq!(pools.fields[0], Some(ComponentValue::Blob20(changed)));
        assert_eq!(pools.fields[1], Some(ComponentValue::Blob20([0; 20])));
        assert_eq!(BotPlate::parse(""), BotPlate::default());
    }
}
