//! The experience a match gives and the experience each level costs, from
//! the install (`data/game_progression_174.json`, derived from the XP gains
//! settings `054/0008` and the player progression settings `054/0018` by
//! `lucy_assets`).
//!
//! The XP gains name each gain by its display string. The lobby gives
//! Time, Match Finish, Win, the three medals and First win of the day. It
//! does not give Backfill (200) or Consecutive Match (150): it does not know
//! when they apply.
//!
//! The level curve (`m_88922C14`) has 25 entries: 0, 0, 2000, 3500, 5000,
//! ... 20000. Entry `n` is the experience from level `n - 1` to level `n`,
//! and levels after the last entry cost the last one [assumption, from the
//! data and rule 178: entry 0 has no level and level 1 is where a player
//! starts, so both are 0; with this reading, levels 5 to 12 cost what the
//! launch table gives (7500 to 14500), and the cost drops at the step to
//! level 14 (15000, not 15500), where the November 2016 patch made levels
//! "slightly" cheaper. The last entry is 20000, which rule 178 gives for
//! every later level. Levels 1 to 4 cost 500 more than the launch table
//! (2000, 3500, 5000, 6500), which no public source explains.]

use std::{
    collections::HashMap,
    fs::File,
    io::{self, BufReader, Read},
    path::Path,
};

use serde::Deserialize;

pub const FILE: &str = "game_progression_174.json";

/// The match experience and level costs the lobby gives.
#[derive(Clone, Debug, PartialEq)]
pub struct XpTable {
    /// Experience per second in the match ("Time").
    pub per_second: f64,
    /// "Match Finish".
    pub match_finish: i64,
    /// "Win".
    pub win: i64,
    /// "Gold Medal", "Silver Medal", "Bronze Medal".
    pub medals: [i64; 3],
    /// "First win of the day".
    pub first_win: i64,
    /// Entry `n`: the experience from level `n - 1` to level `n`.
    levels: Vec<i64>,
}

#[derive(Deserialize)]
struct Raw {
    xp_gains: HashMap<String, RawGain>,
    level_experience: Vec<i64>,
}

#[derive(Deserialize)]
struct RawGain {
    amount: f64,
}

impl XpTable {
    pub fn load(data_dir: &Path) -> io::Result<Self> {
        let path = data_dir.join(FILE);
        let file = File::open(&path).map_err(|error| {
            io::Error::new(error.kind(), format!("{}: {error}", path.display()))
        })?;
        Self::from_reader(BufReader::new(file))
    }

    pub fn from_reader(reader: impl Read) -> io::Result<Self> {
        let raw: Raw = serde_json::from_reader(reader)
            .map_err(|error| invalid(format!("invalid {FILE}: {error}")))?;
        let gain = |name: &str| {
            raw.xp_gains
                .get(name)
                .map(|gain| gain.amount)
                .ok_or_else(|| invalid(format!("{FILE} has no XP gain {name}")))
        };
        let whole = |name: &str| -> io::Result<i64> {
            let amount = gain(name)?;
            if amount.fract() != 0.0 || amount < 0.0 {
                return Err(invalid(format!(
                    "{FILE}: {name} is {amount}, not a whole amount"
                )));
            }
            Ok(amount as i64)
        };
        // Levels 0 and 1 are not reached by experience; level 2 on are.
        if raw.level_experience.len() < 3 || raw.level_experience.iter().any(|&xp| xp < 0) {
            return Err(invalid(format!("{FILE}: an unexpected level curve")));
        }
        Ok(Self {
            per_second: gain("time")?,
            match_finish: whole("match_finish")?,
            win: whole("win")?,
            medals: [
                whole("gold_medal")?,
                whole("silver_medal")?,
                whole("bronze_medal")?,
            ],
            first_win: whole("first_win_of_the_day")?,
            levels: raw.level_experience,
        })
    }

    /// The experience from `level` to the next. A level below 1 costs what
    /// level 1 does; levels past the curve cost its last entry.
    pub fn level_experience(&self, level: i64) -> i64 {
        let next = usize::try_from(level.max(1) + 1).unwrap_or(usize::MAX);
        self.levels[next.min(self.levels.len() - 1)]
    }
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_catalog;

    #[test]
    fn the_curve_entry_after_a_level_is_its_cost() {
        let table = test_catalog::xp_table();
        assert_eq!(table.level_experience(0), 2000);
        assert_eq!(table.level_experience(1), 2000);
        assert_eq!(table.level_experience(2), 3500);
        assert_eq!(table.level_experience(13), 15_000);
        assert_eq!(table.level_experience(23), 20_000);
        assert_eq!(table.level_experience(24), 20_000);
        assert_eq!(table.level_experience(500), 20_000);
    }

    #[test]
    fn a_missing_gain_or_a_short_curve_is_an_error() {
        let missing =
            r#"{"xp_gains": {"time": {"amount": 1.75}}, "level_experience": [0, 0, 2000]}"#;
        assert!(XpTable::from_reader(missing.as_bytes()).is_err());
        let short = r#"{"xp_gains": {}, "level_experience": [0, 0]}"#;
        assert!(XpTable::from_reader(short.as_bytes()).is_err());
    }
}
