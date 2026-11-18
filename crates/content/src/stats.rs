//! Owner scoreboard fields and on-fire display values for component 64.
//!
//! Ported from ow174/game/stats.py and ow174/game/onfire.py. The retail fire
//! scoring rule and its threshold remain outside this component encoder.
//!
//! Client research (1.74, the Tab board): the client opens the board itself:
//! its HUD makes a local entity (01F1.003, 0x7FF789A1AEE0) and starts graph
//! 041C on it (0x7FF789A0DF86), whose SubScript 01BE holds the board
//! (presenter STU_A4F3808F, screen 0403.05A) while the Scoreboard key (logical
//! button 77, or 78 to toggle) is down. Its rows are every player entity with
//! a slot (component 74) and a team (component 25 from 26), split by relation
//! to the local player (0x7FF7899B6B90); name, level and rank come from his
//! 20308 card, the hero from his component 75. Its own numbers are the local
//! player's component 64 (0x7FF7899B3F90 -> 0x7FF789A1CB00): fields 5-12 name
//! 8 stats (062 GUIDs) with values in 21-28 and a medal each in 37-44; fields
//! 13-18 name 6 more with values in 29-34; GUID 0 is no entry. The GUIDs go
//! as ids (a byte mask and the non-zero bytes): as raw u64 the board showed
//! "UNKNOWN STAT" (seen live upstream, kjs `254fa913`, `tab-board.md`, which
//! also saw this slot assignment named right). The value is printed by the
//! stat's display type (0x7FF789ABFD50). A medal (the stat item's Rank,
//! 005A.05E) is 1 gold, 2 silver, 3 bronze, 0 none. Fields 5-34 and 37-44
//! are owner-only (field flag 0x10). Unverified: the medal rule (OW1's: best
//! three among teammates, above 0, not for a flagged stat like Deaths).
//!
//! On Fire: the player card's fire meter (01CF st37's FireScore,
//! 0x7FF78982EBB0) reads the local player's component 64 (MECGMPStats):
//! `100 * min(score, 1.28 * threshold) / (1.28 * threshold)`, score field 19,
//! threshold field 20 (owner-only); 1.28 is at 0x7FF78B53BD08, and with no
//! threshold the client divides 0 by 0. The portrait burns (OnFire,
//! 0x7FF789BE2370; 01CF st17's effects) while field 36 is set, read through a
//! body's possessor, on any client. The bar shows only in the Tutorial mode
//! (0019.0C5): its FireBarVisible asks GetPvPEntity, which this client always
//! answers with no entity (0x7FF789BD8000). Component 64's field types (type
//! 0x7FF78BFE9DA0): 5 nested `{u64, f32}`, 14 stat GUIDs, 16 f32, u16, u8
//! (36), 8 u8. The server rule that fills the score is not in the client; the data
//! has only the score events and their weights (003F.054).

use std::{collections::HashMap, fs, io, path::Path};

use indexmap::IndexMap;
use serde::Deserialize;

use orpheus_core::guid::parse_guid;
use orpheus_wire::entity::{Component, ComponentValue, GMP_STATS_FIELDS};

pub const GMP_STATS: u8 = 64;
pub const GOLD: u8 = 1;
pub const SILVER: u8 = 2;
pub const BRONZE: u8 = 3;

const MEDAL_SLOTS: usize = 8;
const HERO_SLOTS: usize = 6;
const MEDAL_STAT: usize = 5;
const HERO_STAT: usize = 13;
const MEDAL_VALUE: usize = 21;
const HERO_VALUE: usize = 29;
const MEDAL: usize = 37;
const SCORE: usize = 19;
const THRESHOLD: usize = 20;
const ON_FIRE: usize = 36;
const FULL: f32 = 1.28;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StatSlot {
    pub guid: u64,
    pub no_medal: bool,
}

#[derive(Debug, Default)]
pub struct PlayerStats {
    values: HashMap<u64, f64>,
}

impl PlayerStats {
    pub fn add(&mut self, stat: u64, amount: f64) {
        *self.values.entry(stat).or_default() += amount;
    }

    pub fn set(&mut self, stat: u64, value: f64) {
        self.values.insert(stat, value);
    }

    pub fn value(&self, stat: u64) -> f64 {
        self.values.get(&stat).copied().unwrap_or_default()
    }
}

pub type Medals = HashMap<u64, u8>;

/// The stats (`062`) the server counts, as the Tab board lists them for a
/// player: the default list of `0051.054` `m_FC833C02`, in its order. The
/// stats' display strings say which is which (in 1.74: `0025`
/// Eliminations, `0326` Objective Kills, `0327` Objective Time, `04B8` Hero
/// Damage Done, `01D1` Healing Done, `0029` Deaths); Deaths is the one
/// without a medal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BoardStats {
    pub eliminations: u64,
    pub objective_kills: u64,
    pub objective_time: u64,
    pub hero_damage: u64,
    pub healing: u64,
    pub deaths: u64,
}

impl BoardStats {
    /// The player list's six stats in order; an error when the list no
    /// longer has six stats with only the last (Deaths) without a medal.
    fn from_slots(slots: &[StatSlot]) -> io::Result<Self> {
        let medals = slots.iter().map(|slot| slot.no_medal);
        match slots {
            [
                eliminations,
                objective_kills,
                objective_time,
                hero_damage,
                healing,
                deaths,
            ] if medals.eq([false, false, false, false, false, true]) => Ok(Self {
                eliminations: eliminations.guid,
                objective_kills: objective_kills.guid,
                objective_time: objective_time.guid,
                hero_damage: hero_damage.guid,
                healing: healing.guid,
                deaths: deaths.guid,
            }),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "the player stats are not the six of the Tab board, Deaths last: {slots:?}"
                ),
            )),
        }
    }
}

#[derive(Debug, Default)]
pub struct StatsCatalog {
    player: StatLists,
    heroes: HashMap<u64, StatLists>,
    display: HashMap<u64, Option<u8>>,
}

impl StatsCatalog {
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::from_json(&fs::read_to_string(path)?)
    }

    pub fn from_json(text: &str) -> io::Result<Self> {
        let raw: RawCatalog = serde_json::from_str(text)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        Ok(Self {
            player: raw.player.try_into()?,
            heroes: raw
                .heroes
                .into_iter()
                .map(|(hero, lists)| Ok((guid(&hero)?, lists.try_into()?)))
                .collect::<io::Result<_>>()?,
            display: raw
                .stats
                .into_iter()
                .map(|(stat, display)| Ok((guid(&stat)?, display)))
                .collect::<io::Result<_>>()?,
        })
    }

    /// The stats the server counts for each player; an error when the
    /// player list is not the Tab board's six.
    pub fn board(&self) -> io::Result<BoardStats> {
        BoardStats::from_slots(&self.player.default)
    }

    pub fn player_stats(&self, mode: u64) -> &[StatSlot] {
        self.player.select(mode)
    }

    pub fn hero_stats(&self, hero: u64, mode: u64) -> &[StatSlot] {
        self.heroes
            .get(&hero)
            .map_or(&[], |lists| lists.select(mode))
    }

    pub fn display_type(&self, stat: u64) -> Option<u8> {
        self.display.get(&stat).copied().flatten()
    }

    /// Medal placement among the caller's team, including all players in a
    /// free-for-all. Equal positive scores keep the input order.
    pub fn medals(&self, team: &[&PlayerStats], mode: u64) -> Vec<Medals> {
        let mut medals = vec![Medals::new(); team.len()];
        for slot in self.player_stats(mode).iter().filter(|slot| !slot.no_medal) {
            let mut ranked: Vec<_> = team
                .iter()
                .enumerate()
                .filter_map(|(index, player)| {
                    let value = player.value(slot.guid);
                    (value > 0.0).then_some((index, value))
                })
                .collect();
            ranked.sort_by(|left, right| right.1.total_cmp(&left.1));
            for (medal, (index, _)) in [GOLD, SILVER, BRONZE].into_iter().zip(ranked) {
                medals[index].insert(slot.guid, medal);
            }
        }
        medals
    }

    /// An owner's board. Unused slots carry zero so a hero or mode change
    /// clears the former entries. The on-fire fields are a separate update.
    pub fn component(
        &self,
        record: &PlayerStats,
        mode: u64,
        hero: Option<u64>,
        medals: Option<&Medals>,
    ) -> Component {
        let mut fields = vec![None; GMP_STATS_FIELDS];
        let player_stats = self.player_stats(mode);
        let hero_stats = hero
            .filter(|&hero| hero != 0)
            .map_or(&[][..], |hero| self.hero_stats(hero, mode));
        for (entries, slots, stat_offset, value_offset) in [
            (player_stats, MEDAL_SLOTS, MEDAL_STAT, MEDAL_VALUE),
            (hero_stats, HERO_SLOTS, HERO_STAT, HERO_VALUE),
        ] {
            for index in 0..slots {
                let stat = entries.get(index).map_or(0, |slot| slot.guid);
                let value = if stat == 0 {
                    0.0
                } else {
                    record.value(stat) as f32
                };
                fields[stat_offset + index] = Some(ComponentValue::Id(stat));
                fields[value_offset + index] = Some(ComponentValue::F32(value));
            }
        }
        for index in 0..MEDAL_SLOTS {
            let medal = player_stats
                .get(index)
                .filter(|slot| slot.guid != 0)
                .and_then(|slot| medals?.get(&slot.guid))
                .copied()
                .unwrap_or_default();
            fields[MEDAL + index] = Some(ComponentValue::U8(medal));
        }
        Component {
            index: GMP_STATS,
            fields,
        }
    }
}

/// The retail client's meter in percent. A zero threshold produces NaN.
pub fn meter(score: f32, threshold: f32) -> f32 {
    let top = threshold * FULL;
    if top == 0.0 {
        return f32::NAN;
    }
    let score = if top < score { top } else { score };
    (score / top) * 100.0
}

/// Only the owner's view includes score and threshold. All viewers may
/// receive the on-fire flag. Callers omit each field they are leaving alone.
pub fn on_fire_component(
    score: Option<f32>,
    threshold: Option<f32>,
    on_fire: Option<bool>,
) -> Component {
    let mut fields = vec![None; GMP_STATS_FIELDS];
    fields[SCORE] = score.map(ComponentValue::F32);
    fields[THRESHOLD] = threshold.map(ComponentValue::F32);
    fields[ON_FIRE] = on_fire.map(|flag| ComponentValue::U8(u8::from(flag)));
    Component {
        index: GMP_STATS,
        fields,
    }
}

/// The effective flag on a fresh player controller, before any stats updates.
pub fn initial_on_fire(component: &Component) -> Option<bool> {
    if component.index != GMP_STATS {
        return None;
    }
    match component.fields.get(ON_FIRE).and_then(Option::as_ref) {
        None => Some(false),
        Some(ComponentValue::U8(flag)) => Some(*flag != 0),
        _ => None,
    }
}

#[derive(Debug, Default)]
struct StatLists {
    default: Vec<StatSlot>,
    modes: HashMap<u64, Vec<StatSlot>>,
}

impl StatLists {
    fn select(&self, mode: u64) -> &[StatSlot] {
        self.modes.get(&mode).unwrap_or(&self.default)
    }
}

impl TryFrom<RawLists> for StatLists {
    type Error = io::Error;

    fn try_from(raw: RawLists) -> io::Result<Self> {
        Ok(Self {
            default: slots(raw.default)?,
            modes: raw
                .modes
                .into_iter()
                .map(|(mode, entries)| Ok((guid(&mode)?, slots(entries)?)))
                .collect::<io::Result<_>>()?,
        })
    }
}

#[derive(Deserialize)]
struct RawCatalog {
    player: RawLists,
    heroes: IndexMap<String, RawLists>,
    stats: IndexMap<String, Option<u8>>,
}

#[derive(Deserialize)]
struct RawLists {
    default: Vec<(String, RawNoMedal)>,
    modes: IndexMap<String, Vec<(String, RawNoMedal)>>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawNoMedal {
    Bool(bool),
    Int(i64),
}

fn slots(raw: Vec<(String, RawNoMedal)>) -> io::Result<Vec<StatSlot>> {
    raw.into_iter()
        .map(|(stat, no_medal)| {
            let no_medal = match no_medal {
                RawNoMedal::Bool(value) => value,
                RawNoMedal::Int(value) => value != 0,
            };
            Ok(StatSlot {
                guid: guid(&stat)?,
                no_medal,
            })
        })
        .collect()
}

fn guid(text: &str) -> io::Result<u64> {
    let trimmed = text.trim();
    let digits = trimmed.strip_prefix('+').unwrap_or(trimmed);
    let parsed = match digits.as_bytes() {
        [b'0', b'x' | b'X', ..] => parse_guid(digits),
        [b'_', ..] => None,
        _ => parse_guid(&format!("0x{digits}")),
    };
    parsed.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid stat catalog GUID {text:?}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use orpheus_wire::entity::{WorldError, update};
    use sha2::{Digest, Sha256};

    const ELIMINATIONS: u64 = 0x0860_0000_0000_0025;
    const OBJECTIVE_TIME: u64 = 0x0860_0000_0000_0327;
    const HERO_DAMAGE: u64 = 0x0860_0000_0000_04b8;
    const HEALING: u64 = 0x0860_0000_0000_01d1;
    const DEATHS: u64 = 0x0860_0000_0000_0029;
    const HERO: u64 = 0x90;
    const MODE: u64 = 2;
    const FIXTURE: &str = r#"{
        "player": {
            "default": [
                ["0x0860000000000025",0], ["08600000000004B8",false], ["0x0860000000000029",1]
            ],
            "modes": {"0x2":[["0x0860000000000327",false]]}
        },
        "heroes": {
            "0x90": {"default":[["0x08600000000001D1",0]], "modes":{"0x2":[]}}
        },
        "stats": {"0x0860000000000327":8, "0x0860000000000025":0, "0x0860000000000029":null}
    }"#;

    fn fixture() -> StatsCatalog {
        StatsCatalog::from_json(FIXTURE).unwrap()
    }

    #[test]
    fn reads_ordered_catalog_and_mode_lists_including_empty_overrides() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("game_stats_174.json");
        assert_eq!(
            StatsCatalog::load(&path).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        fs::write(&path, FIXTURE).unwrap();
        let catalog = StatsCatalog::load(&path).unwrap();
        let rows = |list: &[StatSlot]| {
            list.iter()
                .map(|slot| (slot.guid, slot.no_medal))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            rows(catalog.player_stats(0)),
            [(ELIMINATIONS, false), (HERO_DAMAGE, false), (DEATHS, true)]
        );
        assert_eq!(rows(catalog.player_stats(MODE)), [(OBJECTIVE_TIME, false)]);
        assert_eq!(rows(catalog.hero_stats(HERO, 0)), [(HEALING, false)]);
        assert!(catalog.hero_stats(HERO, MODE).is_empty());
        assert!(catalog.hero_stats(3, 0).is_empty());
        assert_eq!(catalog.display_type(OBJECTIVE_TIME), Some(8));
        assert_eq!(catalog.display_type(ELIMINATIONS), Some(0));
        assert_eq!(catalog.display_type(DEATHS), None);
        fs::write(&path, "{").unwrap();
        assert_eq!(
            StatsCatalog::load(&path).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn medals_exclude_flagged_stats_and_keep_tied_players_in_order() {
        let catalog = fixture();
        let team: Vec<_> = [4.0, 9.0, 0.0, 9.0, 2.0, f64::NAN, -1.0]
            .into_iter()
            .map(|value| {
                let mut stats = PlayerStats::default();
                stats.set(ELIMINATIONS, value);
                stats.set(DEATHS, value);
                stats
            })
            .collect();
        let team: Vec<_> = team.iter().collect();
        let medals = catalog.medals(&team, 0);
        let kills: Vec<_> = medals
            .iter()
            .map(|entry| entry.get(&ELIMINATIONS).copied().unwrap_or_default())
            .collect();
        assert_eq!(kills, [BRONZE, GOLD, 0, SILVER, 0, 0, 0]);
        assert!(
            medals
                .iter()
                .all(|medal| !medal.contains_key(&DEATHS) && !medal.contains_key(&HERO_DAMAGE))
        );
    }

    #[test]
    fn owner_scoreboard_writes_full_slots_and_matches_python_at_bit_offset() {
        let catalog = fixture();
        let mut stats = PlayerStats::default();
        stats.add(ELIMINATIONS, 1.0);
        stats.add(ELIMINATIONS, 2.0);
        stats.set(HERO_DAMAGE, 1234.5);
        stats.set(DEATHS, 1.0);
        stats.set(HEALING, 80.125);
        let medals = Medals::from([(ELIMINATIONS, GOLD)]);
        let board = catalog.component(&stats, 0, Some(HERO), Some(&medals));
        let record = update(3, std::slice::from_ref(&board)).unwrap();
        // Python's bytes had the stat GUIDs as raw u64 (1485 bits); they go
        // as ids now, a byte mask and the non-zero bytes, as the client reads
        // them (seen live upstream, kjs `254fa913`: raw u64 named "UNKNOWN
        // STAT"). Every other field is Python's.
        assert_eq!(record.bit_len(), 813);
        assert_eq!(
            format!("{:x}", Sha256::digest(record.as_bytes())),
            "7d2d94c00ae0186167a88de22c147c26c4a930827b68d418be7b450d97d6d32a"
        );
        assert_eq!(
            board.fields[MEDAL_STAT],
            Some(ComponentValue::Id(ELIMINATIONS))
        );
        assert_eq!(board.fields[MEDAL_VALUE], Some(ComponentValue::F32(3.0)));
        assert_eq!(board.fields[MEDAL], Some(ComponentValue::U8(GOLD)));
        assert_eq!(board.fields[HERO_STAT], Some(ComponentValue::Id(HEALING)));
        assert_eq!(board.fields[HERO_VALUE], Some(ComponentValue::F32(80.125)));
        assert_eq!(board.fields[SCORE], None);
        assert_eq!(board.fields[THRESHOLD], None);
        assert_eq!(board.fields[ON_FIRE], None);
        let before_pick = catalog.component(&stats, 0, None, None);
        for index in 0..HERO_SLOTS {
            assert_eq!(
                before_pick.fields[HERO_STAT + index],
                Some(ComponentValue::Id(0))
            );
            assert_eq!(
                before_pick.fields[HERO_VALUE + index],
                Some(ComponentValue::F32(0.0))
            );
        }
        let switch_mode = catalog.component(&stats, MODE, Some(HERO), None);
        assert_eq!(
            switch_mode.fields[MEDAL_STAT + 1],
            Some(ComponentValue::Id(0))
        );
        assert_eq!(switch_mode.fields[HERO_STAT], Some(ComponentValue::Id(0)));
    }

    #[test]
    fn on_fire_updates_match_python_and_leave_other_fields_unwritten() {
        assert_eq!(
            initial_on_fire(&on_fire_component(None, None, None)),
            Some(false)
        );
        assert_eq!(
            initial_on_fire(&on_fire_component(None, None, Some(true))),
            Some(true)
        );
        let owner = update(
            0,
            &[on_fire_component(Some(250.0), Some(500.0), Some(false))],
        )
        .unwrap();
        assert_eq!(
            owner.as_bytes(),
            [
                0x01, 0x40, 0x06, 0x00, 0x00, 0x18, 0x00, 0x10, 0x00, 0x00, 0x00, 0x7a, 0x43, 0x00,
                0x00, 0xfa, 0x43, 0x00,
            ]
        );
        let flag = update(0, &[on_fire_component(None, None, Some(true))]).unwrap();
        assert_eq!(flag.as_bytes(), [0x01, 0x40, 0x06, 0, 0, 0, 0, 0x10, 0, 1]);
        assert_eq!(meter(0.0, 1000.0), 0.0);
        assert_eq!(meter(640.0, 500.0), 100.0);
        assert_eq!(meter(900.0, 500.0), 100.0);
        assert_eq!(meter(320.0, 500.0), 50.0);
        assert!(meter(10.0, 0.0).is_nan());
        assert!(meter(f32::NAN, 500.0).is_nan());
    }

    #[test]
    fn stats_layout_accepts_the_unsigned_scalar_and_rejects_nested_fields() {
        let mut fields = vec![None; GMP_STATS_FIELDS];
        fields[35] = Some(ComponentValue::U16(0xabcd));
        let mut component = Component {
            index: GMP_STATS,
            fields,
        };
        let scalar = update(0, std::slice::from_ref(&component)).unwrap();
        assert_eq!(
            scalar.as_bytes(),
            [1, 0x40, 6, 0, 0, 0, 0, 8, 0, 0xcd, 0xab]
        );
        for field in 0..5 {
            component.fields[field] = Some(ComponentValue::U64(1));
            assert!(matches!(
                update(0, std::slice::from_ref(&component)),
                Err(WorldError::FieldType { component: GMP_STATS, field: bad }) if bad == field
            ));
            component.fields[field] = None;
        }
        component.fields.push(None);
        assert!(matches!(
            update(0, &[component]),
            Err(WorldError::FieldCount {
                component: GMP_STATS,
                ..
            })
        ));
    }

    #[test]
    #[ignore = "requires the derived 104319 stat catalog"]
    fn actual_scoreboard_components_match_the_python_wire_corpus() {
        use crate::maps::PYTHON_PRACTICE_RANGE_MODE;
        use std::collections::BTreeSet;

        let root = orpheus_testkit::runtime_dir();
        let catalog = StatsCatalog::load(root.join("data/game_stats_174.json")).unwrap();
        let modes: BTreeSet<_> = std::iter::once(&catalog.player)
            .chain(catalog.heroes.values())
            .flat_map(|group| group.modes.keys().copied())
            // Python's corpus has its Range mode 0018 (ours is 005A).
            .chain([0, PYTHON_PRACTICE_RANGE_MODE, 0x0230_0000_0000_0017])
            .collect();
        let mut heroes = catalog.heroes.keys().copied().collect::<Vec<_>>();
        heroes.sort_unstable();
        let mut records: [PlayerStats; 5] = std::array::from_fn(|_| PlayerStats::default());
        for &stat in catalog.display.keys() {
            let base = (stat & 0xf) as f64 / 8.0;
            for (index, (record, value)) in records
                .iter_mut()
                .zip([0.25, 1.25, 0.0, 1.25, 0.75])
                .enumerate()
            {
                record.set(stat, if index == 2 { 0.0 } else { base + value });
            }
        }
        let team: Vec<_> = records.iter().collect();
        let mut digest = Sha256::new();
        let mut case = 0;
        for mode in modes.iter().copied() {
            let medals = catalog.medals(&team, mode);
            for hero in std::iter::once(None).chain(heroes.iter().copied().map(Some)) {
                let index = case % records.len();
                let origin = case % 8;
                let board = catalog.component(&records[index], mode, hero, Some(&medals[index]));
                let packet = update(origin, &[board]).unwrap();
                digest.update(mode.to_le_bytes());
                digest.update(hero.unwrap_or_default().to_le_bytes());
                digest.update(u32::try_from(origin).unwrap().to_le_bytes());
                digest.update(u32::try_from(packet.bit_len()).unwrap().to_le_bytes());
                digest.update(packet.as_bytes());
                case += 1;
            }
        }
        assert_eq!((modes.len(), heroes.len(), case), (14, 32, 462));
        // Python's corpus with the stat GUIDs as ids (kjs `254fa913`; see
        // `owner_scoreboard_writes_full_slots_and_matches_python_at_bit_offset`).
        assert_eq!(
            format!("{:x}", digest.finalize()),
            "6d9f46b2040bf950a11664db308c13756d09be0987ea38a3cdee90cb558fcc60"
        );
    }

    #[test]
    fn the_board_stats_are_the_player_list_in_order() {
        let six = FIXTURE.replace(
            r#"["0x0860000000000025",0], ["08600000000004B8",false], ["0x0860000000000029",1]"#,
            r#"["0x25",0], ["0x326",0], ["0x327",0], ["0x4B8",0], ["0x1D1",0], ["0x29",1]"#,
        );
        assert_eq!(
            StatsCatalog::from_json(&six).unwrap().board().unwrap(),
            BoardStats {
                eliminations: 0x25,
                objective_kills: 0x326,
                objective_time: 0x327,
                hero_damage: 0x4b8,
                healing: 0x1d1,
                deaths: 0x29,
            }
        );
        // Three stats, or Deaths not last, are not the board.
        assert_eq!(
            fixture().board().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let moved = six.replace(r#"["0x1D1",0], ["0x29",1]"#, r#"["0x29",1], ["0x1D1",0]"#);
        assert!(StatsCatalog::from_json(&moved).unwrap().board().is_err());
    }

    /// The install's player list holds the six stats whose display strings
    /// are Eliminations, Objective Kills, Objective Time, Hero Damage Done,
    /// Healing Done and Deaths (read from each `062`'s `m_name`).
    #[test]
    #[ignore = "requires the derived 104319 stat catalog"]
    fn the_derived_board_stats_are_the_install_s() {
        let root = orpheus_testkit::runtime_dir();
        let catalog = StatsCatalog::load(root.join("data/game_stats_174.json")).unwrap();
        assert_eq!(
            catalog.board().unwrap(),
            BoardStats {
                eliminations: ELIMINATIONS,
                objective_kills: 0x0860_0000_0000_0326,
                objective_time: OBJECTIVE_TIME,
                hero_damage: HERO_DAMAGE,
                healing: HEALING,
                deaths: DEATHS,
            }
        );
    }
}
