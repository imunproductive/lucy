//! The client's hero settings (Options > Controls > HERO), which it sends
//! the game server as ClientOutInstance 21604.
//!
//! 21604 is `{+0x78 [{+0x0 u64, +0x8 u64, +0x10 u64}], +0x90 bool, +0x91
//! bool}` (`game_schemas.json`). Its entries have the layout of a saved hero
//! setting, `STULocalPersistedHeroSetting {m_hero, m_settingId, m_value}`
//! (`stu/data/types.txt`); the settings the lobby saves (22200, 20802 +0x78
//! +0x0) hold the same entries, e.g. `{0, 0D80…1DE8, 0.08 as f64 bits}`. So
//! an entry is: the hero it is for (0: every hero), the setting's
//! `STUIdentifier`, its value. What the two booleans mean is not known.
//!
//! A hero setting is an entity variable of the hero's body, whose id is the
//! setting identifier's index: no graph writes it, and the graphs compare it
//! with the setting's choices. `orpheus script query var 8336` gives
//! "identifier 2090.01C", the ALLIED HEALTH BARS setting.
//!
//! What the client sends (seen in the game, 2026-11-11): one entry per
//! setting that is not at its DEFAULT, for every hero, after it joins and
//! after each change; at DEFAULT it sends an empty list. So each 21604 is
//! the whole list, and a setting it leaves out is back at DEFAULT. An
//! identifier setting's value is the chosen entry's `STUIdentifier` GUID.
//! Only two settings come: ALLIED HEALTH BARS and SHOW FRIENDLY OUTLINES,
//! the only two of `054/0126` with `m_84E9992A = 1` (every other one has 0),
//! so that flag likely marks the settings the game server must know.
//! Assumption: an entry with a hero GUID is that hero's own choice.

use orpheus_wire::{jam::codec::Value, messages::Message};

pub const HERO_SETTINGS: u32 = 21604;

/// HERO > ALLIED HEALTH BARS (`054/0126` `STUHeroSettingIdentifier`
/// `2090.01C`, name `4AE4.07C`; entries DEFAULT `4AE0.07C` without an id,
/// ALWAYS `2092.01C`, OFF `6E2D.01C`). Body graph `01CF` (networked entry
/// rid165) reads it as `#8336e`: a support (target tag `0042`) with OFF
/// drops tag `0042` (st115 → st114), any other hero with ALWAYS takes it
/// (st116 → st117). The nameplate `0D24` shows an ally's health bar to a
/// viewer with that tag (`#17553`).
pub const ALLIED_HEALTH_BARS: u64 = 0x0d80_0000_0000_2090;
pub const ALLIED_HEALTH_BARS_ALWAYS: u64 = 0x0d80_0000_0000_2092;
pub const ALLIED_HEALTH_BARS_OFF: u64 = 0x0d80_0000_0000_6e2d;

/// HERO > SHOW FRIENDLY OUTLINES (`054/0126` `STUHeroSettingIdentifier`
/// `6BB0.01C`, name `F048.07C`; `m_default` and DEFAULT `6BAC.01C`, OFF
/// `6BBB.01C`, OCCLUDED `6BAE.01C`, ALWAYS `6BAF.01C`; values seen in the
/// game, DEFAULT as an empty list). Only the client reads it: graph `2206`
/// (client-only, under `01CF` st110) copies the local view's `#27568e`
/// into `#27568` (st31) and switches on it (st27) to pick the outline of an
/// allied player's body.
pub const FRIENDLY_OUTLINES: u64 = 0x0d80_0000_0000_6bb0;
pub const FRIENDLY_OUTLINES_OFF: u64 = 0x0d80_0000_0000_6bbb;
pub const FRIENDLY_OUTLINES_OCCLUDED: u64 = 0x0d80_0000_0000_6bae;
pub const FRIENDLY_OUTLINES_ALWAYS: u64 = 0x0d80_0000_0000_6baf;

/// The hero settings the server puts on the body: the two the client sends.
pub const SCRIPT_SETTINGS: [u64; 2] = [ALLIED_HEALTH_BARS, FRIENDLY_OUTLINES];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeroSetting {
    /// The hero GUID the entry is for; 0 for every hero.
    pub hero: u64,
    /// The setting's `STUIdentifier` GUID.
    pub setting: u64,
    pub value: u64,
}

/// The entries of a 21604; none for another message or a malformed one.
pub fn read(message: &Message) -> Option<Vec<HeroSetting>> {
    if message.id != HERO_SETTINGS {
        return None;
    }
    let Value::Array(entries) = message.fields.get(&0x78)? else {
        return None;
    };
    entries
        .iter()
        .map(|entry| {
            let Value::Struct(fields) = entry else {
                return None;
            };
            let number = |offset| match fields.get(&offset) {
                Some(Value::Unsigned(value)) => Some(*value),
                _ => None,
            };
            Some(HeroSetting {
                hero: number(0)?,
                setting: number(8)?,
                value: number(0x10)?,
            })
        })
        .collect()
}

/// A player's hero settings as his client last sent them.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HeroSettings(Vec<HeroSetting>);

impl HeroSettings {
    /// Take a 21604: it lists every setting that isn't at DEFAULT, so it
    /// replaces what the client sent before.
    pub fn replace(&mut self, entries: &[HeroSetting]) {
        self.0.clear();
        self.0.extend_from_slice(entries);
    }

    /// The value `hero` plays `setting` with: his own entry, else the one for
    /// every hero; none at DEFAULT.
    pub fn value(&self, hero: u64, setting: u64) -> Option<u64> {
        let find = |hero| {
            self.0
                .iter()
                .find(|entry| (entry.hero, entry.setting) == (hero, setting))
                .map(|entry| entry.value)
        };
        (hero != 0)
            .then(|| find(hero))
            .flatten()
            .or_else(|| find(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orpheus_wire::jam::codec::Fields;

    use crate::heroes::{ANA, SOLDIER};

    fn entry(hero: u64, setting: u64, value: u64) -> Value {
        Value::Struct(Fields::from([
            (0, Value::Unsigned(hero)),
            (8, Value::Unsigned(setting)),
            (0x10, Value::Unsigned(value)),
        ]))
    }

    fn message(entries: Vec<Value>) -> Message {
        Message {
            id: HERO_SETTINGS,
            fields: Fields::from([
                (0x78, Value::Array(entries)),
                (0x90, Value::Bool(false)),
                (0x91, Value::Bool(false)),
            ]),
        }
    }

    #[test]
    fn a_settings_message_gives_its_entries() {
        let sent = message(vec![
            entry(0, ALLIED_HEALTH_BARS, ALLIED_HEALTH_BARS_ALWAYS),
            entry(SOLDIER, 0x0d80_0000_0000_1ddc, 1),
        ]);
        assert_eq!(
            read(&sent),
            Some(vec![
                HeroSetting {
                    hero: 0,
                    setting: ALLIED_HEALTH_BARS,
                    value: ALLIED_HEALTH_BARS_ALWAYS
                },
                HeroSetting {
                    hero: SOLDIER,
                    setting: 0x0d80_0000_0000_1ddc,
                    value: 1
                },
            ])
        );
        assert_eq!(read(&message(Vec::new())), Some(Vec::new()));
        let mut malformed = message(vec![Value::Unsigned(1)]);
        assert_eq!(read(&malformed), None);
        malformed.id = 21603;
        assert_eq!(read(&malformed), None);
    }

    #[test]
    fn a_heros_own_entry_comes_before_the_general_one_and_each_list_replaces() {
        let mut settings = HeroSettings::default();
        assert_eq!(settings.value(SOLDIER, ALLIED_HEALTH_BARS), None);
        let general = |setting, value| HeroSetting {
            hero: 0,
            setting,
            value,
        };
        settings.replace(&[
            general(ALLIED_HEALTH_BARS, ALLIED_HEALTH_BARS_ALWAYS),
            HeroSetting {
                hero: ANA,
                ..general(ALLIED_HEALTH_BARS, ALLIED_HEALTH_BARS_OFF)
            },
            general(FRIENDLY_OUTLINES, FRIENDLY_OUTLINES_OCCLUDED),
        ]);
        assert_eq!(
            settings.value(SOLDIER, ALLIED_HEALTH_BARS),
            Some(ALLIED_HEALTH_BARS_ALWAYS)
        );
        assert_eq!(
            settings.value(ANA, ALLIED_HEALTH_BARS),
            Some(ALLIED_HEALTH_BARS_OFF)
        );
        assert_eq!(
            settings.value(ANA, FRIENDLY_OUTLINES),
            Some(FRIENDLY_OUTLINES_OCCLUDED)
        );
        // The client sends OFF for the outlines and leaves the bars at
        // DEFAULT: they are no longer listed.
        settings.replace(&[general(FRIENDLY_OUTLINES, FRIENDLY_OUTLINES_OFF)]);
        assert_eq!(settings.value(SOLDIER, ALLIED_HEALTH_BARS), None);
        assert_eq!(settings.value(ANA, ALLIED_HEALTH_BARS), None);
        assert_eq!(
            settings.value(SOLDIER, FRIENDLY_OUTLINES),
            Some(FRIENDLY_OUTLINES_OFF)
        );
        // Everything at DEFAULT: an empty list.
        settings.replace(&[]);
        assert_eq!(settings.value(SOLDIER, FRIENDLY_OUTLINES), None);
    }
}
