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
//!   for a flag word on each candidate entity: leafall903/ORPHEUS (MIT)
//!   `docs/COMMUNICATION_WHEEL.txt` ran the spray query natively; `1B60A50`
//!   requires a candidate's flags (entity `+270`) to overlap the query's
//!   allow mask, `0x0001000000000040`, and miss its deny mask. The weapons'
//!   ray profiles (`097`, `STU_8EFC337B`: `m_BC69BB40` allow, `m_5CE47069`
//!   deny) allow only bit 48 on nearly every one (`0001`, `0015`, `0019`,
//!   `002A`, …, the Helix Rocket's and Ana's darts' included) and deny
//!   0x10 (the dead bit above), 0x4 or 0x80. That `+270` word is component
//!   1's, and every sync of the filter word into it (`0x7FF78AA1A940`)
//!   copies only its low 48 bits (mask `0xFFFFFFFFFFFF`; the high bits
//!   come from component 1's constructor), so no bit of component 26 at
//!   48 or above can reach it: our bots with bit 48 in component 26 were
//!   still shot through (checked in the game, 2026-11-12), and
//!   ow1-preservation's bots, whose component 26 was `0x11040001`, showed
//!   impact sparks on their body (its run 343). Bit 48 is the client's
//!   own: it sets it in component 1's word when the entity's model
//!   component (component 4, `STUModelComponent`) attaches (`0x7FF78A9FE8C0`
//!   @`0x7FF78A9FEB89`; the other attachments set bits 49 and 51-54, bit 55
//!   is the default), so the server can't set or clear it, and a bot with
//!   a model has it (HitPredict.md, "Client tools pass 2 (2026-11-16)"). No
//!   graph filter names it (their masks stop at `0x3FFFFFFF`).
//! - So every body, a player's or a training bot's (the bots are heroes,
//!   `02E0…016B`-`016E`), gets its team bit and [`BODY_KINDS`]
//!   ([`body_filter`]); a body that dies gets [`DEAD`] too, with the pools
//!   of its death. The client doesn't need it: it clears 0x10 from the
//!   word and sets it itself when the entity's component 51 has a maximum
//!   and no current health (`0x7FF789A9B020`, `0x7FF789A9BEB0`), which our
//!   death pools give; the bit is redundant for the client, and the
//!   server's VM reads it (`PlayerWorld::entity_filter_flags`; the Python
//!   reference sent the team bit alone).
//!   Checked in the game: with its team bit alone, then with 0x41, then
//!   with bit 48 too, a Training Bot was not hit by the client's own shots
//!   (Soldier: 76's bullets and Ana's darts left a hole and the map's
//!   impact sound on the wall behind it, Helix Rockets flew through it),
//!   though the server hit it. The filter bits were not the cause; nor was
//!   the record cadence (`bots.rs` `Bot::record_due`: every frame now;
//!   Helix Rockets then exploded on the bot, perhaps only at the server's
//!   release point, but bullets still went through), nor the bots' entity
//!   id class (`bots.rs` module notes), nor the spring steps (the bot's
//!   collision capsule had sat 2.5 m too low, `Bot::movement`).
//! - 0x4 marks a body its graphs hold a status on (`STU_9028E035`, either
//!   type: the spawn room's invulnerability `0C8E` st11, Wraith Form,
//!   Recall, ...): [`SHIELDED`]. The client's explosion knockback
//!   (`0x7FF789A433F0`, gate @`0x7FF789A439CE`) pushes a body only when its
//!   comp25 `+0x58` word has 0x40 and neither 0x10 nor 0x4, and no code on
//!   that path reads a status; the word is component 26's. Checked in the
//!   game (2026-11-17, Soldier: 76's Helix Rocket at his feet,
//!   refs/reports/OwnerPrediction.md): in the spawn room with 0x4 the gate
//!   read 0x810045 and queued no push, outside it read 0x810041 and his
//!   client lifted him 2.3 m. The graphs' damage searches leave 0x4 out
//!   with the dead (`m_4EA686DB` 0x14 in 96 filters: quick melee `0043`,
//!   Death Blossom `0044`, ...); 76 leave out 0x10 alone (Lúcio's aura
//!   search `073A` st17, the room's own `0D62`), and the weapons' ray
//!   profiles deny it. So 0x4 is taken as the native mark of a status
//!   (assumption: the retail server sets it from the status; the client
//!   never sets it itself, `0x7FF789A9B020`).
//! - The client's own hitscan finds a body only through its filter word's
//!   bit 0x10000 (checked in the game, 2026-11-17, HitPredict.md "Night"):
//!   the client files an entity under spatial groups by the word's bits
//!   0x10000, 0x1, 0x40, 0x40000 and 0x100000 (`0x7FF789B862F0`, table
//!   `0x7FF78B56B290`), and the volley's ray builder (`0x7FF789AFEEB0`)
//!   walks group 0x10000 alone (`0x7FF789B83720(0x10000, …)`), then tests
//!   each body's model hit shapes (`0x7FF789B00C50`). The physics-world
//!   rays of a shot meet only the map, props and ragdolls (categories 0x1,
//!   0x80, 0x100); a living body there is one 0x2000 capsule no shot asks
//!   for. With 0x41 alone the bots were in groups 0x1 and 0x40 and every
//!   client shot went through them; with 0x10000 the client's hit-shape
//!   test hits them. So every body gets it ([`BODY_KINDS`]).
//! - A moving map placeable (the Practice Range's platforms, definition
//!   `0516`, moved by its `STUMirroredIdleAnimComponent`) has client shapes
//!   of category 0x1000, picked from model data (`0x7FF78A9CC460`), and a
//!   shot's world ray asks only for 0x81, so it never meets them. The shot's
//!   entity query does: it keeps the weapon profile's body mask (its
//!   `m_CBCB2AB9`: `0x1081` for `0001`, measured `0x1883` in the query of
//!   `0002`, both with 0x1000; `orpheus_assets` `ray_profiles`), accepts an
//!   entity whose moving bodies' categories overlap it
//!   (`0x7FF789B01DD0`) and tests those bodies (`0x7FF789B00C50`); but it
//!   walks only group 0x10000. A placeable created from the map has
//!   components 25 and 26 (the network create adds them), and the client
//!   files it by the component 26 word when its spawn callback runs
//!   (`0x7FF789A9C530`; component 26 changes before that are dropped,
//!   `0x7FF789A9C9D0` waits for comp26 +0x98). So the placeable's create
//!   carries component 26 with [`MOVING_KINDS`] ([`placeable_components`]).
//! - The client rewinds a remote body for its own shots from a history of
//!   bounding spheres in the body's component 24 (`+0x70` ring of 20
//!   spheres, `+0x1D0` their frames; read by `0x7FF789AE1BC0` in the
//!   hitscan group walk `0x7FF789B83200`, which drops a body whose sphere at
//!   the shot's view frame misses the ray). A new sample is appended only
//!   when its frame is above the newest one's; otherwise it overwrites the
//!   newest and keeps that frame (`0x7FF789B80F70` @`0x7FF789B81216`,
//!   unsigned). A create without a movement state leaves the body at frame
//!   0xFFFFFFFF, so every later record overwrote that sample and the ring
//!   kept only two: one at the world origin from the entity's creation
//!   frame (its model bounds before the transform) and the newest. The
//!   rewind then interpolated between them by frame and put the sphere near
//!   the origin: the client's shots skipped a bot they crossed and left
//!   their hole on the wall behind it (checked in the game, 2026-11-17,
//!   HitPredict.md "Day"). So a bot's create carries its movement state at
//!   frame 0 ([`FRAME_ZERO_BACK`]), and its movement records, at
//!   their packet frames, append to the history; so does another
//!   client's create of a player's body (`body::body_create`).
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
//! - The elimination label (`1FC0`) names the victim by `STU_CD7F3617`
//!   of its body, which finds the body's controller (its component 44, or
//!   the reverse table component 44 builds) and reads the controller's
//!   name; a bot body has no name component of its own. So each training
//!   bot gets a stand-in controller entity ([`bot_controller_create`]).
//! - The kill feed (`0DEA`) instead resolves each side's controller first
//!   (`STU_F76C486C`, else `STU_DB742151`) and takes the name
//!   (`STU_CD7F3617`) and the hero icon (`STU_DC20DD7E` of `STU_93DCDDFD`)
//!   of that controller itself. With a stand-in that had only 26 (the team
//!   bit), 29 and 44, the game drew the bot's side of the line as an empty
//!   box, as killer and as victim (2026-11-13, screenshots
//!   `killfeed-player-kills-bot.png`, `killfeed-bot-kills-player.png`),
//!   though "ELIMINATED TRAINING BOT" showed. The one setup seen drawing a
//!   bot's name and icon there is leafall903/ORPHEUS (MIT) `kjs` 2f85cee
//!   (`offline/research/lizrollin-runtime.md` section 16, runs 22-23;
//!   `offline/tools/lobby/vendor/ow174/game/match.py` `_target_updates`,
//!   `game_struct`): each bot's controller a full participant, definition
//!   `000E` created with the participant flag and without the player path
//!   one, 26 = a player's filter bits, 74 with a roster slot of its own,
//!   44 the body, 75 the bot's hero, and the bot named in the 20300/20301
//!   roster at that slot. Assumption: the client names and draws a
//!   controller in the feed only as a participant. Component 75 on a
//!   stand-in without 74 crashed the game (2026-11-13, the removed
//!   `.plate hero`); kjs sent 75 with 74 and did not crash, nor did our
//!   earlier AI-definition stand-in (`1404` with 26/44/75).
//! - The stand-in keeps 74 but holds no roster slot (-1), and the roster
//!   doesn't name the bots: the Tab board's rows are the entities with
//!   component 44 (`0x7FF7899B6B90` walks that pool) whose 74 holds a slot
//!   other than -1 and that have a team (`0x7FF789A1D3B0`), so a slot put
//!   a row per bot on the board, which retail's Practice Range doesn't have
//!   (the user, 2026-11-13 and 2026-11-18). The kill feed reads no slot:
//!   `STU_CD7F3617` names a controller without component 43 by its own 29
//!   and 75, and the feed drew "RESEARCHER -> TRAINING BOT" in the game
//!   with the participant stand-in (`refs/reports/KillFeed.md`, "Client
//!   tools pass").

use std::{collections::HashMap, io};

use crate::{
    body::yaw_rotation,
    bot_plate::BotPlate,
    bots::{BOT_CONTROLLER_ENTITY, BOT_ENTITY, Bot},
    instance::FREE_FOR_ALL_TEAM,
    map_areas::PlacedScript,
};
use orpheus_content::{
    maps::{GameMap, SpawnChoice},
    stats::GMP_STATS,
};
use orpheus_core::bits::BitWriter;
use orpheus_script::{
    graph::GraphStore,
    target_tags::TargetTagLists,
    vitals::Vitals,
    wire::{self, Instance, Layout, Variables},
};
use orpheus_wire::{
    entity::{
        Component, ComponentValue, Create, EntityUpdate, FRAME_ZERO_BACK, GAME_MODE_ENTITY,
        Movement, Operation, PLAYER_PATH, SkinTheme, WITH_PARTICIPANT, WITH_SCALE, WITH_TRANSFORM,
    },
    jam::codec::Fields,
};

const PLAYER_DEFINITION: u64 = 0x0400_0000_0000_000e;
const GAME_MODE_DEFINITION: u64 = 0x0400_0000_0000_16b2;
const PLAYER_FILTER: u64 = 0x0004_0001;
const TEAM_BIT: u64 = 0x0080_0000;
/// The kind bits of a living body (see the module notes): a hero (0x1) that
/// can be hit (0x40), in the client's hitscan group (0x10000).
pub const BODY_KINDS: u64 = 0x1_0041;
/// The filter bit of a dead body (see the module notes).
pub const DEAD: u64 = 0x10;
/// The filter bit of a body its graphs hold a status on (`STU_9028E035`:
/// the spawn room's invulnerability, Wraith Form, Recall, ...; see the
/// module notes).
pub const SHIELDED: u64 = 0x4;
/// The kind bits of a moving map placeable (the Practice Range's platforms):
/// the client's hitscan group (0x10000), so a shot's entity query tests
/// its moving bodies (see the module notes).
pub const MOVING_KINDS: u64 = 0x1_0000;
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

/// The roster slot of an entity that has none ([`bot_controller_create`]):
/// the client's slot readers take -1 (`0xFF`) as "no slot"
/// (`0x7FF789A1D3B0` returns false for it, `0x7FF789B46650` returns it as
/// its own "none", `0x7FF789ADB8B0` refuses it).
const NO_ROSTER_SLOT: i8 = -1;

/// The create of the bot's stand-in controller in `slot`, linked to its
/// current body, so the client names the bot in "ELIMINATED <name>" and
/// names it and draws its hero icon in the kill feed (see the module
/// notes; the Python reference sent none). A participant as a player's
/// entity is, from kjs: definition `000E`, created with the participant
/// flag and without the player path one (that is the local player's own
/// entity); 26 a player's filter bits with the bot's team bit, 29 a
/// stand-in account and the bot's name, 74 with fields 6 and 7 on (as a
/// player's and kjs's) and no roster slot, 44 the body, 75 the bot's hero
/// (no skin) as a player's [`Possession`] has them. No slot keeps it off
/// the Tab board (module notes).
pub fn bot_controller_create(bot: &Bot, slot: usize) -> io::Result<EntityUpdate> {
    use ComponentValue::*;
    let entity = bot_controller_entity(slot)?;
    let account = BOT_ACCOUNT + (entity - BOT_CONTROLLER_ENTITY);
    let mut components = vec![
        fields(
            26,
            vec![Some(Id(PLAYER_FILTER | team_bit(bot.spawn.team)?))],
        ),
        fields(
            29,
            vec![
                Some(DbId(u64::from(account), ACCOUNT_HIGH)),
                Some(Text(bot.spawn.name.clone())),
            ],
        ),
        fields(
            74,
            vec![
                None,
                None,
                Some(I8(NO_ROSTER_SLOT)),
                None,
                None,
                None,
                Some(U8(1)),
                Some(U8(1)),
            ],
        ),
    ];
    components.extend(
        Possession {
            body: bot.entity,
            hero: bot.spawn.hero,
            skin: None,
        }
        .components(),
    );
    let mut definition = Create::new(PLAYER_DEFINITION, components);
    definition.flags = WITH_PARTICIPANT | WITH_TRANSFORM | WITH_SCALE;
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

fn possession_link(body: u32) -> Component {
    fields(44, vec![Some(ComponentValue::Entity(body)); 3])
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

/// The mode entity's create: its component 114 with the mode (field 1)
/// and the match's value of field 9 `m_CFF0C4E3`, the float the client's
/// `STU_B182C5C9` reads (`Eval_STU_B182C5C9` reads component `+0x170` of
/// the first component 114; ModeGraphGaps.md, "Client tools pass 2
/// (2026-11-16)"), which the server's worlds answer too.
pub fn mode_create(mode: u64, mode_random: f32) -> EntityUpdate {
    let mut values = vec![None; 10];
    values[1] = Some(ComponentValue::Id(mode));
    values[9] = Some(ComponentValue::F32(mode_random));
    create(
        GAME_MODE_ENTITY,
        Create::new(GAME_MODE_DEFINITION, vec![fields(114, values)]),
    )
}

/// One match's value of the mode entity's component 114 field 9: uniform
/// in [0, 1), from 24 random bits so that each value is an exact `f32`.
/// Retail's draw is server-side and not known; `0F4A` uid 1299 keeps it
/// as the payload's random speed factor #29093 for the whole match.
pub fn draw_mode_random() -> f32 {
    (random_bits() >> 40) as f32 / (1u32 << 24) as f32
}

/// One match's random table index (`STU_CBF6FF14`, server-only; Control
/// `0EC2` picks its stages from the table at it): 14 random bits, the
/// table's index range, so that the graph's `index + round` stays exact as
/// an `f32`. Retail's value is not known.
pub fn draw_match_seed() -> u16 {
    (random_bits() >> 50) as u16
}

/// 64 bits from the OS-seeded hasher keys.
fn random_bits() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish()
}

/// A bot's create: its body with its full pools, as the live nameplate tool
/// has them ([`BotPlate::pools`]), its filter bits, and its movement state
/// at frame 0 ([`FRAME_ZERO_BACK`]).
pub fn bot_create(bot: &Bot, plate: &BotPlate) -> io::Result<EntityUpdate> {
    let pools = plate.pools(&Vitals::new(bot.spawn.health, 0.0, 0.0));
    let body = BOT_BODY.unwrap_or(bot.spawn.body);
    let mut definition = Create::new(body, vec![pools, bot_filter(bot)?]);
    definition.position = bot.position.map(|position| position as f32);
    definition.rotation = yaw_rotation(bot.yaw.round_ties_even());
    definition.movement = Some(Movement {
        input_frame: None,
        throttles: [0; 2],
        input_throttles: [0; 2],
        ..bot.movement(0)
    });
    definition.frame_back = FRAME_ZERO_BACK;
    Ok(create(bot.entity, definition))
}

/// Build the initial world before the local player picks a hero; each bot's
/// stand-in controller right after its body.
pub fn initial_world(
    map: &GameMap,
    player: &PlayerAppearance<'_>,
    board: Component,
    bots: &[Bot],
    plate: &BotPlate,
    mode_random: f32,
) -> io::Result<Vec<EntityUpdate>> {
    let mut updates = Vec::with_capacity(map.placeables.len() + 2 + 2 * bots.len());
    updates.extend(map.placeables.iter().map(|&index| EntityUpdate {
        entity: 0x8000_0000 | index,
        operation: Some(Operation::Placeable(index, Vec::new())),
        ..EntityUpdate::default()
    }));
    updates.push(player_create(map, player, None, Some(board))?);
    updates.push(mode_create(map.mode_guid, mode_random));
    for (slot, bot) in bots.iter().enumerate() {
        updates.push(bot_create(bot, plate)?);
        updates.push(bot_controller_create(bot, slot)?);
    }
    Ok(updates)
}

/// The modes whose graph the mode entity carries so the Tab board opens: the
/// mode, its graph, the remote slot of the graph's state that posts game
/// message `06DF.025` (`STU_38EE1100`), and the state index of the
/// SubScript that starts `0CB6` in it (None: the graph starts `0CB6` in a
/// client-only state of its own). The client holds the board shut until
/// `06DF.025` is posted, and only a server-sent state of the mode graph
/// posts it (upstream kjs `254fa913`, `827ef2d7`, `tab-board.md`; seen
/// there in the Range, Team Deathmatch, Deathmatch and Control). Assault,
/// Escort and Hybrid (`0F4A`) are left out: a board state without its phase
/// state draws an "UNSPECIFIED 0.0" objective header. Upstream names 0x1D
/// Deathmatch and 0x1E Team Deathmatch, 0x07 Elimination and 0x09 Capture
/// the Flag; the modes' teams (`0C5` `m_teams`) say the reverse: 0x1E has
/// one free-for-all team of 12 and `0F73` asks that team's size, 0x09 has
/// Elimination's controller `0D6F`. The graph per mode id is the same.
/// `0CE8` (0x07) and `1A59` (0x46) are not in the graph bundle, so those
/// modes keep the plain frame. The Practice Range is not listed: its `13C0`
/// runs whole and its own frames carry the poster state
/// (`crate::mode_script`).
const BOARD_POSTERS: [(u64, u32, usize, Option<u16>); 8] = [
    (0x0230_0000_0000_0007, 0x0ce8, 1, Some(14)), // Capture the Flag
    (0x0230_0000_0000_0009, 0x0d0b, 6, Some(3)),  // Elimination
    (0x0230_0000_0000_0010, 0x0d76, 2, Some(15)), // Skirmish
    (0x0230_0000_0000_0017, 0x0ec2, 1, None),     // Control
    (0x0230_0000_0000_001d, 0x0f60, 0, Some(13)), // Team Deathmatch
    (0x0230_0000_0000_001e, 0x0f73, 0, Some(10)), // Deathmatch
    (0x0230_0000_0000_0046, 0x1a59, 6, Some(2)),  // Freezethaw Elimination
    (0x0230_0000_0000_0070, 0x0f73, 0, Some(10)), // Bounty Hunter
];

/// The game mode entity's H=0 full frame: the mode's own graph with its
/// board state on and `0CB6` under it ([`BOARD_POSTERS`]), else `0CB6`
/// alone with its client-only UI entry dependencies retained.
pub fn mode_ui(graphs: &GraphStore, mode: u64, variables: &Variables) -> io::Result<BitWriter> {
    let graph = graphs
        .graph(MODE_UI)
        .ok_or_else(|| invalid("missing game mode UI graph 0CB6"))?;
    let poster = BOARD_POSTERS
        .iter()
        .find(|row| row.0 == mode)
        .and_then(|&(_, index, slot, host)| Some((graphs.graph(index)?, slot, host)));
    if let Some((own, slot, host)) = poster {
        let posts_board = own
            .remote_positions()
            .get(slot)
            .and_then(|&pos| own.node(pos))
            .is_some_and(|node| node.class == "STU_38EE1100");
        if !posts_board {
            return Err(invalid("the mode graph's board state is not a 06DF poster"));
        }
        let mut instance = Instance::new(1, Layout::try_from(own).map_err(invalid)?, None);
        let mut active = BitWriter::new();
        // Present and active, then the state's one payload bit: no optional
        // integer (calibrated live upstream on `0D28`).
        active.bits(3, 2).map_err(invalid)?;
        active.bit(false);
        instance.states.insert(slot, active);
        let mut instances = vec![instance];
        if let Some(host) = host {
            let layout = Layout::try_from(graph).map_err(invalid)?;
            instances.push(Instance::new(2, layout, Some((1, host))));
        }
        return wire::remote_full(&instances, variables).map_err(invalid);
    }
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

/// The components of a server placeable's create: a moving one gets
/// [`MOVING_KINDS`].
pub fn placeable_components(placed: &PlacedScript) -> Vec<Component> {
    if placed.moving {
        vec![filter_component(MOVING_KINDS)]
    } else {
        Vec::new()
    }
}

/// A body's target tags component (76): the tags its states add, then the
/// ones they suppress. The client keeps a tag as the low 32 bits of its
/// `06C` GUID, the index (`0042.06C` is 0x42).
pub fn target_tags_component(lists: &TargetTagLists) -> Component {
    let ids = |tags: &[u64]| {
        Some(ComponentValue::U32Array(
            tags.iter().map(|&tag| tag as u32).collect(),
        ))
    };
    fields(76, vec![ids(&lists.added), ids(&lists.suppressed)])
}

/// A training bot's filter bits component now: dead or alive.
pub fn bot_filter(bot: &Bot) -> io::Result<Component> {
    Ok(filter_component(body_filter(
        team_bit(bot.spawn.team)?,
        bot.dead,
    )))
}

/// A debugging switch: `ORPHEUS_DEBUG_BOT_BODY=<hex GUID>` creates every
/// training bot with that body definition instead of its own (the server
/// still runs its own body's scripts and shapes). ow1-preservation's bots,
/// which took the client's bullets, were `04000000000014D2`; ours are
/// `1403`/`14ED`, which carry first-person and weapon components that
/// `14D2` has not.
pub static BOT_BODY: std::sync::LazyLock<Option<u64>> = std::sync::LazyLock::new(|| {
    let text = std::env::var("ORPHEUS_DEBUG_BOT_BODY").ok()?;
    let body = u64::from_str_radix(text.trim().trim_start_matches("0x"), 16);
    match body {
        Ok(body) => {
            tracing::warn!("debugging: training bots created as body {body:016X}");
            Some(body)
        }
        Err(_) => {
            tracing::warn!("ORPHEUS_DEBUG_BOT_BODY: not a hex GUID: {text:?}");
            None
        }
    }
});

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
        // Python's Range was mode 0018, ours is 005A (`PRACTICE_RANGE_MODE`):
        // the mode entity's create and the Tab board name the mode, so the
        // frame is built with Python's mode to compare with its bytes.
        let map = GameMap {
            mode_guid: orpheus_content::maps::PYTHON_PRACTICE_RANGE_MODE,
            ..GameMap::practice_range()
        };
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
        // bits were their team bit alone; ours add the body's kind, and our
        // bot creates carry a movement state at frame 0, where Python's had
        // none (module notes). Its bots were `0xC0001000 + n`; ours are in
        // id class 0 (`bots.rs` module notes). Its mode entity had component
        // 114 field 1 alone; ours adds field 9 ([`mode_create`]). The other
        // records match the Python frame byte for byte.
        let updates =
            initial_world(&map, &player, board, &bots, &BotPlate::default(), 0.25).unwrap();
        const PYTHON_BOT_ENTITY: u32 = 0xc000_1000;
        let controllers = BOT_CONTROLLER_ENTITY..BOT_ENTITY;
        let python: Vec<_> = updates
            .iter()
            .filter(|update| !controllers.contains(&update.entity))
            .cloned()
            .map(|mut update| {
                if update.entity == GAME_MODE_ENTITY {
                    assert_eq!(update, mode_create(map.mode_guid, 0.25));
                    let Some(Operation::Create(create)) = &mut update.operation else {
                        unreachable!("a create");
                    };
                    create.components[0].fields =
                        vec![None, Some(ComponentValue::Id(map.mode_guid))];
                }
                let bot = bots.iter().find(|bot| bot.entity == update.entity);
                if let (Some(bot), Some(Operation::Create(create))) = (bot, &mut update.operation) {
                    let team = team_bit(bot.spawn.team).unwrap();
                    let filter = &mut create.components[1];
                    assert_eq!(*filter, filter_component(team | BODY_KINDS));
                    *filter = filter_component(team);
                    let movement = create.movement.take().expect("a movement state");
                    assert_eq!(movement.position, bot.position);
                    assert_eq!(create.frame_back, u32::MAX);
                    create.frame_back = 1;
                    update.entity = update.entity - BOT_ENTITY + PYTHON_BOT_ENTITY;
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
        .chain(PYTHON_BOT_ENTITY..PYTHON_BOT_ENTITY + 15)
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
        let encoded = frame(1234, 1234, 0, &[], &[], &python.iter().collect::<Vec<_>>()).unwrap();
        // Python's frame was 1674 bytes (digest b82987cb…). The one
        // deliberate difference left is the player's Tab board (component
        // 64): its 14 stat GUID fields go as ids, a byte mask and the
        // non-zero bytes, where Python wrote raw u64 (port 1b, upstream kjs
        // `254fa913`: raw u64 showed "UNKNOWN STAT"); the frame is 76 bytes
        // shorter for it.
        assert_eq!(encoded.len(), 1598);
        assert_eq!(
            format!("{:x}", Sha256::digest(&encoded)),
            "bd3c71aafd2797e9bd34511e9c52903ff372dccaddedf8f56f8c17f9a0887919",
        );
        let mut remaining = updates.as_slice();
        let mut transmitted = Vec::new();
        while !remaining.is_empty() {
            let selected = select_frame(
                FrameCandidates {
                    tick: 1234,
                    clock: 0,
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

    /// The player stand-in (module notes): a participant without the player
    /// path, a player's filter bits with the bot's team bit, a stand-in
    /// account with the bot's name, no roster slot (no Tab board row), its
    /// body and its hero.
    #[test]
    fn a_player_stand_in_names_the_bot_and_controls_its_body_as_its_hero() {
        use ComponentValue::*;
        let bots = fixture_bots();
        let update = bot_controller_create(&bots[0], 0).unwrap();
        assert_eq!(update.entity, BOT_CONTROLLER_ENTITY);
        let create = created(&update);
        assert_eq!(create.definition, PLAYER_DEFINITION);
        assert_eq!(create.flags, WITH_PARTICIPANT | WITH_TRANSFORM | WITH_SCALE);
        let training_bot = 0x02e0_0000_0000_016b;
        assert_eq!(
            create.components,
            [
                fields(26, vec![Some(Id(0x0004_0001 | TEAM_BIT << 1))]),
                fields(
                    29,
                    vec![
                        Some(DbId(u64::from(BOT_ACCOUNT), 1 << 56)),
                        Some(Text("Training Bot".into())),
                    ],
                ),
                fields(
                    74,
                    vec![
                        None,
                        None,
                        Some(I8(-1)),
                        None,
                        None,
                        None,
                        Some(U8(1)),
                        Some(U8(1)),
                    ],
                ),
                link(BOT_ENTITY),
                fields(
                    75,
                    vec![Some(Id(training_bot)), None, None, Some(Id(training_bot))],
                ),
            ]
        );
        let other = bot_controller_create(&bots[1], 1).unwrap();
        assert_eq!(other.entity, BOT_CONTROLLER_ENTITY + 1);
        let components = &created(&other).components;
        let Some(Some(DbId(account, _))) = components[1].fields.first() else {
            panic!("no account");
        };
        assert_eq!(*account, u64::from(BOT_ACCOUNT) + 1);
        assert_eq!(components[2].fields[2], Some(I8(-1)));
        // The wire takes every component.
        frame(1234, 1234, 0, &[], &[], &[&update, &other]).unwrap();
    }

    /// A bot's body is a hero that can be hit, on its team (module notes):
    /// its create's filter bits; a bot created dead also says so.
    #[test]
    fn a_bot_body_is_a_hero_that_can_be_hit_until_it_dies() {
        let mut bots = fixture_bots();
        let filter = |bot: &Bot| {
            created(&bot_create(bot, &BotPlate::default()).unwrap()).components[1].clone()
        };
        assert_eq!(
            filter(&bots[0]),
            fields(26, vec![Some(ComponentValue::Id(0x0101_0041))])
        );
        assert_eq!(
            filter(&bots[1]),
            fields(26, vec![Some(ComponentValue::Id(0x0081_0041))])
        );
        bots[0].died();
        assert_eq!(
            filter(&bots[0]),
            fields(26, vec![Some(ComponentValue::Id(0x0101_0051))])
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
        let entities: Vec<u32> =
            initial_world(&map, &player, board, &bots, &BotPlate::default(), 0.5)
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
        let hex = |frame: &BitWriter| -> String {
            frame
                .as_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect()
        };
        let assault = 0x0230_0000_0000_0014;
        let cases = [
            (Variables::new(), 38, "085b06000c"),
            (
                Variables::from([(15545, Value::Float(3.0))]),
                74,
                "085b0600cbe5a901c000",
            ),
        ];
        for (vars, length, expected) in cases {
            let frame = mode_ui(&graphs, assault, &vars).unwrap();
            assert_eq!((frame.bit_len(), hex(&frame).as_str()), (length, expected));
        }
        // The Range has no poster row: `13C0` runs whole and its own frames
        // carry the board poster st6 and `0CB6` under st21 (test
        // `mode_script::tests::the_range_mode_graph_drives_the_mode_entity_with_its_board_poster`).
        // Its hand-built frame, which a client gets only if `13C0` stops,
        // is `0CB6` alone with no variables: Assault's first case, bit for
        // bit.
        for range in [0x0230_0000_0000_0018, 0x0230_0000_0000_005a] {
            let frame = mode_ui(&graphs, range, &Variables::new()).unwrap();
            assert_eq!((frame.bit_len(), hex(&frame).as_str()), (38, "085b06000c"));
        }
        // Every listed mode whose graph is in the bundle names a 06DF poster.
        for (mode, graph, ..) in BOARD_POSTERS {
            let frame = mode_ui(&graphs, mode, &Variables::new()).unwrap();
            let plain = graphs.graph(graph).is_none();
            assert_eq!(frame.bit_len() == 38, plain, "mode {mode:X}");
        }
    }
}
