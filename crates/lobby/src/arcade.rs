//! Arcade catalog, dated rotation and wire messages for retail build 104319.
//!
//! The cards come from the install (`arcade_cards_174.json`, the `0C7`
//! assets); which ones the menu shows and when is the Python reference's
//! rotation.
//!
//! The Arcade screen has seven slots. The client files each card under its
//! menu group (the 0EE menu nodes of catalog 10B.063) and each group takes one
//! slot; the everyday cards are each their own group. The everyday cards
//! follow the two retail Arcades known (the January 2022 capture and
//! OverwatchArcade.today's record of 3 April 2022). With five event groups
//! (the anniversary) the last two everyday cards do not fit; retail's Arcade
//! for that case is not known. A card's flags +0xA8.. are: permanent, the
//! seven from the asset, two unused, role queue; the capture sets the first on
//! No Limits, Quick Play Classic and Mystery Heroes. The competitive cards
//! (asset flag 0, card +0xA9) are offered by the group finder as game types
//! (0x7FF789AA3D80), each running a season.

use std::{
    collections::{BTreeMap, HashSet},
    fs::File,
    io::{self, BufReader},
    path::Path,
};

use serde::Deserialize;

use crate::{
    accounts::Profile,
    clock::DAY,
    passes::Pool,
    ranked::{CARD_BASE, Season},
    transport::LobbyMessage,
};
use lucy_core::guid::parse_guid;
use lucy_wire::jam::codec::{
    Fields,
    Value::{self, Array, Bool, Float32, Signed, Struct, Unsigned},
};

pub const ARCADE: u32 = 0x00a8_654e;
const SLOTS: usize = 7;
const CARDS: u32 = 39802;
const BUSY: u32 = 39804;
const WINDOWS: u32 = 39810;
/// Quick Play's card, the list's first row of its own (39802 `+0x18`).
const QUICK_PLAY: u32 = 0xed;
/// How busy each listed card shows (39804): every card the same, as the
/// server knows no other players' choices (retail sent each card's share).
const BUSYNESS: f32 = 0.1;
const LUNAR: &[u32] = &[0x1a3, 0x25, 0x10c];
const COMPETITIVE_CTF: &[u32] = &[0x1c3];
const WINTER: &[u32] = &[0x98, 0x107, 0x4b, 0x5b, 0x1f];
const JUNKENSTEIN: &[u32] = &[
    0x3c, 0x3d, 0x4a, 0x3e, 0x43, 0x44, 0x49, 0x45, 0x16b, 0x16c, 0x16d, 0x16e, 0x16f, 0x170,
];
const LUCIOBALL: &[u32] = &[0x132, 0x35, 0x1ad];
const ARCHIVES: &[u32] = &[
    0x29, 0x24, 0x2a, 0x2b, 0x2c, 0x28, 0x2d, 0x2e, 0x58, 0x3a, 0x59, 0x5a, 0x66, 0x67, 0x68, 0x69,
    0x8a, 0x8b, 0x8c, 0x8d, 0x8e, 0x8f, 0x90, 0x91,
];
const CHALLENGES: &[u32] = &[
    0x119, 0x112, 0x1a9, 0x116, 0x127, 0x1aa, 0x111, 0x11b, 0x1ab,
];
const SEASONAL: &[&[u32]] = &[WINTER, JUNKENSTEIN, LUCIOBALL, ARCHIVES, LUNAR];
const EVENTS: &[(&str, &[&[u32]])] = &[
    ("lunar", &[LUNAR, COMPETITIVE_CTF]),
    ("winter", &[WINTER]),
    ("halloween", &[JUNKENSTEIN]),
    ("summer", &[LUCIOBALL]),
    ("archives", &[ARCHIVES, CHALLENGES]),
    ("anniversary", SEASONAL),
    ("anniversary_remix_1", SEASONAL),
    ("anniversary_remix_2", SEASONAL),
    ("malevento", &[&[0x1ae], &[0x1b0]]),
];
const FIXED_DAILY: [u32; 2] = [0x15c, 0x34];
const ROTATING: [u32; 12] = [
    0x1c, 0x64, 0xad, 0x1, 0x33, 0x1e, 0x3b, 0x1d, 0x30, 0xa6, 0x7a, 0x47,
];
const PERMANENT: [u32; 2] = [0xef, 0x2];

#[derive(Deserialize)]
struct CardSource {
    identifier: String,
    display: String,
    rewards: Vec<String>,
    flags: [bool; 7],
    #[serde(default)]
    roles: bool,
    #[serde(default)]
    passes: Option<String>,
}

struct Card {
    identifier: u64,
    display: u64,
    rewards: Vec<u64>,
    flags: [bool; 7],
    roles: bool,
    pool: Option<Pool>,
}

pub struct Arcade {
    cards: BTreeMap<u32, Card>,
}

pub struct ArcadeState {
    pub messages: [LobbyMessage; 3],
    pub competitive_cards: Vec<u64>,
}

impl Arcade {
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        let source: BTreeMap<String, CardSource> =
            serde_json::from_reader(BufReader::new(File::open(path)?)).map_err(invalid)?;
        let guid = |key: &str| {
            parse_guid(key).ok_or_else(|| invalid(format!("invalid arcade GUID {key}")))
        };
        let cards = source
            .into_iter()
            .map(|(key, card)| {
                let index = u32::try_from(guid(&key)?).map_err(invalid)?;
                let pool = card
                    .passes
                    .as_deref()
                    .map(guid)
                    .transpose()?
                    .and_then(Pool::from_guid);
                Ok((
                    index,
                    Card {
                        identifier: guid(&card.identifier)?,
                        display: guid(&card.display)?,
                        rewards: card
                            .rewards
                            .iter()
                            .map(|key| guid(key))
                            .collect::<io::Result<_>>()?,
                        flags: card.flags,
                        roles: card.roles,
                        pool,
                    },
                ))
            })
            .collect::<io::Result<BTreeMap<_, _>>>()?;
        let required = EVENTS
            .iter()
            .flat_map(|(_, groups)| groups.iter())
            .flat_map(|group| group.iter().copied())
            .chain(FIXED_DAILY)
            .chain(ROTATING)
            .chain(PERMANENT)
            .chain([QUICK_PLAY]);
        for card in required {
            if !cards.contains_key(&card) {
                return Err(invalid(format!("missing arcade card {card:X}")));
            }
        }
        Ok(Self { cards })
    }

    pub fn has_roles(&self, card: u64) -> bool {
        self.cards
            .get(&(card as u32))
            .is_some_and(|card| card.roles)
    }

    pub fn pass_pool(&self, card: u64) -> Option<Pool> {
        self.cards.get(&(card as u32)).and_then(|card| card.pool)
    }

    /// The Arcade's cards the menu offers now (full GUIDs): event groups
    /// first, then everyday ones (`ow174/content/arcade.py` `cards`).
    pub fn cards(&self, profile: &Profile, now: u64) -> Vec<u64> {
        card_order(&profile.events, now)
            .into_iter()
            .map(full)
            .collect()
    }

    pub fn state(&self, profile: &Profile, season: Season, now: u64) -> io::Result<ArcadeState> {
        let cards = card_order(&profile.events, now);
        let competitive_cards = cards
            .iter()
            .filter_map(|&index| self.cards[&index].flags[0].then_some(full(index)))
            .collect();
        let windows = cards
            .iter()
            .map(|&index| {
                Struct(Fields::from([
                    (0, Unsigned(full(index))),
                    (8, Unsigned(now.saturating_sub(DAY))),
                    (0x10, Unsigned(now.saturating_add(30 * DAY))),
                ]))
            })
            .collect();
        Ok(ArcadeState {
            messages: [
                (ARCADE, CARDS, self.card_lists(&cards, season)?),
                (ARCADE, WINDOWS, Fields::from([(0x78, Array(windows))])),
                (ARCADE, BUSY, self.busy(&cards, season)),
            ],
            competitive_cards,
        })
    }

    /// 39802: the Arcade's cards (`+0x0`), Quick Play (`+0x18`) and the
    /// competitive rows (`+0xD0`): the season's card, naming its open queue
    /// card (`+0x88`), and that card, naming the season's (`+0x80`).
    fn card_lists(&self, cards: &[u32], season: Season) -> io::Result<Fields> {
        let mut main = self.row(season.card as u32, true)?;
        main.insert(
            0x88,
            Array(season.open_card.map(Unsigned).into_iter().collect()),
        );
        main.insert(0xb2, Bool(season.has_roles()));
        let mut competitive = vec![Struct(main)];
        if let Some(open_card) = season.open_card {
            let mut open = self.row(open_card as u32, true)?;
            open.insert(0x80, Unsigned(season.card));
            competitive.push(Struct(open));
        }
        Ok(Fields::from([(
            0x78,
            Struct(Fields::from([
                (
                    0,
                    Array(
                        cards
                            .iter()
                            .map(|&index| self.card(index))
                            .collect::<io::Result<_>>()?,
                    ),
                ),
                (0x18, Struct(self.row(QUICK_PLAY, true)?)),
                (0xd0, Array(competitive)),
            ])),
        )]))
    }

    fn card(&self, index: u32) -> io::Result<Value> {
        let permanent = matches!(index, 0x1c | 0xef | 0x2);
        self.row(index, permanent).map(Struct)
    }

    /// A card's row: its asset's identifier, display entry, rewards and
    /// flags, how long a daily card stays (24 hours) and whether it is
    /// permanent and has a role queue.
    fn row(&self, index: u32, permanent: bool) -> io::Result<Fields> {
        let card = self
            .cards
            .get(&index)
            .ok_or_else(|| invalid(format!("missing arcade card {index:X}")))?;
        let daily = FIXED_DAILY.contains(&index) || ROTATING.contains(&index);
        let mut fields = Fields::from([
            (0, Unsigned(full(index))),
            (0x8, Unsigned(0)),
            (0x10, Unsigned(card.identifier)),
            (0x18, Unsigned(card.display)),
            (
                0x20,
                Array(card.rewards.iter().copied().map(Unsigned).collect()),
            ),
            (0x38, Array(Vec::new())),
            (0x50, Array(Vec::new())),
            (0x68, Array(Vec::new())),
            (0x80, Unsigned(0)),
            (0x88, Array(Vec::new())),
            (0xa0, Signed(0)),
            (0xa4, Unsigned(if daily { 24 } else { 0 })),
        ]);
        for (offset, flag) in (0xa8..).zip(
            [permanent]
                .into_iter()
                .chain(card.flags)
                .chain([false, false, card.roles]),
        ) {
            fields.insert(offset, Bool(flag));
        }
        Ok(fields)
    }

    /// 39804: how busy each card is, for every card the lists show.
    fn busy(&self, cards: &[u32], season: Season) -> Fields {
        let listed = cards
            .iter()
            .map(|&index| full(index))
            .chain([full(QUICK_PLAY), season.card])
            .chain(season.open_card);
        let mut seen = HashSet::new();
        let entries = listed
            .filter(|&card| seen.insert(card))
            .map(|card| Struct(Fields::from([(0, Unsigned(card)), (8, Float32(BUSYNESS))])))
            .collect();
        Fields::from([(0x78, Array(entries))])
    }
}

fn card_order(events: &[String], now: u64) -> Vec<u32> {
    let mut groups = Vec::new();
    for event in events {
        if let Some((_, seasonal)) = EVENTS.iter().find(|(name, _)| *name == event) {
            for &group in *seasonal {
                if !groups.contains(&group) {
                    groups.push(group);
                }
            }
        }
    }
    let rotating = SLOTS.saturating_sub(groups.len() + FIXED_DAILY.len() + PERMANENT.len());
    let offset = ((now / DAY) % ROTATING.len() as u64) as usize;
    let room = SLOTS.saturating_sub(groups.len());
    groups
        .into_iter()
        .take(SLOTS)
        .flat_map(|group| group.iter().copied())
        .chain(
            FIXED_DAILY
                .into_iter()
                .chain((0..rotating).map(|i| ROTATING[(offset + i) % ROTATING.len()]))
                .chain(PERMANENT)
                .take(room),
        )
        .collect()
}

fn full(index: u32) -> u64 {
    CARD_BASE | u64::from(index)
}

fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests {

    use serde_json::json;

    use super::*;
    use crate::{passes, ranked::Ranked};
    use lucy_wire::jam::codec::Schemas;

    #[test]
    fn rotation_counts_groups_instead_of_cards_and_deduplicates_events() {
        assert_eq!(
            card_order(&[], 0),
            [0x15c, 0x34, 0x1c, 0x64, 0xad, 0xef, 0x2]
        );
        assert_eq!(card_order(&[], DAY)[2..5], [0x64, 0xad, 0x1]);
        assert_eq!(
            card_order(&["lunar".into(), "lunar".into()], 0),
            [0x1a3, 0x25, 0x10c, 0x1c3, 0x15c, 0x34, 0x1c, 0xef, 0x2]
        );
        let anniversary = card_order(&["anniversary".into()], 0);
        assert_eq!(
            anniversary.len(),
            SEASONAL.iter().map(|group| group.len()).sum::<usize>() + 2
        );
        assert!(anniversary.ends_with(&FIXED_DAILY));
    }

    /// The Quick Play and competitive rows are the cards' assets with the
    /// row flags (permanent, role queue) and links; they are the rows the
    /// 1.68 retail capture holds for season 32. Every state encodes, for
    /// each event and season.
    #[test]
    #[ignore = "requires the game data derived from a 1.74 install (just test-game <install>)"]
    fn the_rows_come_from_the_cards_for_every_event_and_season() {
        let data = lucy_testkit::runtime_dir().join("data");
        let schemas = Schemas::lobby();
        let ranked = Ranked::load(&data).unwrap();
        let arcade = Arcade::load(data.join("arcade_cards_174.json")).unwrap();
        assert!(arcade.has_roles(CARD_BASE | 0xed));
        assert_eq!(arcade.pass_pool(CARD_BASE | 0xed), Some(Pool::QuickPlay));
        assert!(!arcade.has_roles(2));
        assert_eq!(arcade.pass_pool(2), None);

        let row = |card: u64, display: u64, flags: [bool; 11], links: [(u32, Value); 2]| {
            let mut row = Fields::from([
                (0, Unsigned(CARD_BASE | card)),
                (0x8, Unsigned(0)),
                (0x10, Unsigned(0)),
                (0x18, Unsigned(display)),
                (0x20, Array(Vec::new())),
                (0x38, Array(Vec::new())),
                (0x50, Array(Vec::new())),
                (0x68, Array(Vec::new())),
                (0xa0, Signed(0)),
                (0xa4, Unsigned(0)),
            ]);
            row.extend(links);
            row.extend((0xa8..).zip(flags.map(Bool)));
            Struct(row)
        };
        let (t, f) = (true, false);
        let profile = Profile::default();
        let state = arcade
            .state(&profile, ranked.season(&profile), 1_700_000_000)
            .unwrap();
        let Struct(lists) = &state.messages[0].2[&0x78] else {
            panic!("the lists")
        };
        assert_eq!(
            lists[&0x18],
            row(
                0xed,
                0,
                [t, f, f, t, t, t, t, t, f, f, t],
                [(0x80, Unsigned(0)), (0x88, Array(Vec::new()))]
            )
        );
        assert_eq!(
            lists[&0xd0],
            Array(vec![
                row(
                    0x1b3,
                    0,
                    [t, t, f, f, f, f, f, t, f, f, t],
                    [
                        (0x80, Unsigned(0)),
                        (0x88, Array(vec![Unsigned(CARD_BASE | 0x1b4)]))
                    ]
                ),
                row(
                    0x1b4,
                    0x01b0_0000_0000_004b,
                    [t, t, f, f, f, f, f, t, f, f, f],
                    [
                        (0x80, Unsigned(CARD_BASE | 0x1b3)),
                        (0x88, Array(Vec::new()))
                    ]
                ),
            ])
        );

        let events: &[&[&str]] = &[
            &[],
            &["lunar"],
            &["winter"],
            &["halloween"],
            &["summer"],
            &["archives"],
            &["anniversary"],
            &["malevento"],
            &["archives", "malevento", "lunar", "lunar"],
        ];
        for (n, event) in events.iter().enumerate() {
            for season in [1, 17, 18, 22, 23, 32, 36] {
                let profile = Profile {
                    events: event.iter().map(|value| (*value).into()).collect(),
                    season,
                    priority_passes: [("quick_play".into(), json!("7"))].into_iter().collect(),
                    ..Profile::default()
                };
                let now = 1_700_000_000 + n as u64 * DAY;
                let season = ranked.season(&profile);
                let state = arcade.state(&profile, season, now).unwrap();
                // Every card the lists show is as busy as the others.
                let Array(busy) = &state.messages[2].2[&0x78] else {
                    panic!("busyness")
                };
                let listed = card_order(&profile.events, now).len()
                    + 2
                    + usize::from(season.open_card.is_some());
                assert_eq!(busy.len(), listed);
                for (crc, id, fields) in state
                    .messages
                    .into_iter()
                    .chain([passes::counts(&profile).unwrap()])
                {
                    schemas.encode(crc, id, &fields).unwrap();
                }
            }
        }
    }
}
