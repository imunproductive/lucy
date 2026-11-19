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
//!
//! Every other setting a graph node the server runs reads goes on the body
//! at its default from the data ([`SCRIPT_SETTINGS`]): a null there can send
//! a networked node down another branch than the client's (Ana's Nano Boost
//! search found nobody with a cone of `10 × null`;
//! `D:/Work/Coding/refs/reports/HeroSettings.md`). Assumption: retail's
//! server used the defaults too, as no client sends these.

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

/// The settings the client sends (21604), which the server puts on the body
/// as sent.
pub const SENT_SETTINGS: [u64; 2] = [ALLIED_HEALTH_BARS, FRIENDLY_OUTLINES];

/// A hero setting's default (`m_default`), by its class in `054/0126`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SettingDefault {
    /// `STUHeroSettingBool`.
    Bool(bool),
    /// `STUHeroSettingFloat`.
    Float(f32),
    /// `STUHeroSettingIdentifier`: the default entry's `STUIdentifier`, none
    /// for an entry without one.
    Identifier(Option<u64>),
}

/// A hero setting that a graph node the server runs reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScriptSetting {
    /// The hero (`075`) whose `m_heroSpecificSettings` hold it; 0 for the
    /// settings of every hero (`m_142A3CA9`).
    pub hero: u64,
    /// The setting's `STUIdentifier` GUID; its index is the body's entity
    /// variable.
    pub setting: u64,
    pub default: SettingDefault,
}

const fn setting(hero: u64, index: u64, default: SettingDefault) -> ScriptSetting {
    ScriptSetting {
        hero: if hero == 0 {
            0
        } else {
            0x02e0_0000_0000_0000 | hero
        },
        setting: 0x0d80_0000_0000_0000 | index,
        default,
    }
}

/// Every `054/0126` setting that a graph node the server runs (not
/// client-only) reads, with the graphs of those reads: the survey of
/// `D:/Work/Coding/refs/reports/HeroSettings.md`, checked against the data
/// by `orpheus_sim` `the_script_settings_are_the_hero_settings_server_nodes_read`.
/// The other 41 are read by client code or client-only nodes alone.
pub const SCRIPT_SETTINGS: [ScriptSetting; 50] = {
    use SettingDefault::{Bool, Float, Identifier};
    [
        // Every hero
        setting(0x0000, 0x2090, Identifier(None)), // Allied Health Bars: `01CF`
        // Ana (`013B.075`)
        setting(0x013b, 0x1fed, Bool(false)), // Toggle Zoom: `0B94`
        setting(0x013b, 0x1fee, Float(0.3)),  // Relative Aim Sensitivity While Zoomed: `0B94`
        setting(0x013b, 0x2582, Float(1.0)),  // Nano Boost Sensitivity: `0B9E` (cone `10 × #9602e`)
        setting(0x013b, 0x2943, Bool(true)),  // Recoil Recovery Aim Compensation: `0B94`
        setting(0x013b, 0x33da, Float(0.0)),  // Friendly Aim Assist Strength: `0C6B`
        setting(0x013b, 0x5e24, Float(0.3)),  // Relative Gyro Aim Sensitivity While Zoomed: `0B94`
        setting(0x013b, 0x6a44, Bool(false)), // Gyro Only In Ironsights: `0B94`
        setting(0x013b, 0x717c, Bool(false)), // Nano Boost Requires Target Confirmation: `0B9E`
        // Ashe (`0200.075`)
        setting(0x0200, 0x423a, Float(0.6)), // Relative Aim Sensitivity While Zoomed: `14FC`
        setting(0x0200, 0x423b, Bool(false)), // Toggle Zoom: `14FC`
        setting(0x0200, 0x44d3, Bool(true)), // Recoil Recovery Aim Compensation: `14FC`
        setting(0x0200, 0x5e25, Float(0.6)), // Relative Gyro Aim Sensitivity While Zoomed: `14FC`
        setting(0x0200, 0x6a45, Bool(false)), // Gyro Only In Ironsights: `14FC`
        // Baptiste (`0221.075`)
        setting(0x0221, 0x4c74, Bool(true)), // Recoil Recovery Aim Compensation: `1932`
        setting(0x0221, 0x5042, Bool(true)), // Automatic Reload: `1932`
        // Brigitte (`0195.075`)
        setting(0x0195, 0x3951, Bool(false)), // Toggle Barrier: `0EE3`
        setting(0x0195, 0x40d5, Float(1.0)),  // Repair Pack Sensitivity: `0EDD`
        // Cassidy (`0042.075`)
        setting(0x0042, 0x2941, Bool(true)), // Recoil Recovery Aim Compensation: `01D2`
        // Echo (`0206.075`)
        setting(0x0206, 0x717b, Bool(false)), // Duplicate Requires Target Confirmation: `19EB`
        setting(0x0206, 0x717d, Float(1.0)),  // Duplicate Sensitivity: `19EB`
        setting(0x0206, 0x747b, Bool(false)), // Hold for Flight: `1BB6`
        // Genji (`0029.075`)
        setting(0x0029, 0x1dda, Bool(false)), // Automatically Climb Walls: `0157`
        // Hanzo (`0005.075`)
        setting(0x0005, 0x1ddd, Bool(false)), // Automatically Climb Walls: `010C`
        // Junkrat (`0065.075`)
        setting(0x0065, 0x1dde, Bool(false)), // RIP-Tire Automatically Climbs Walls: `0233` (the tire copies its owner's)
        // Lúcio (`0079.075`)
        setting(0x0079, 0x1e56, Bool(false)), // Hold to Crossfade: `0266`
        setting(0x0079, 0x2501, Bool(false)), // Allow Backwards Wall Ride: `0263`
        setting(0x0079, 0x8058, Bool(true)),  // Wall Jump on Release: `0263`
        // Mercy (`0004.075`)
        setting(0x0004, 0x1de2, Bool(false)), // Toggle Beam Connection: `006B`
        setting(0x0004, 0x1e48, Bool(true)),  // Guardian Angel Prefers Beam Target: `006E`
        setting(0x0004, 0x1e64, Bool(true)),  // Toggle Guardian Angel: `006E`
        setting(0x0004, 0x256e, Float(1.0)),  // Guardian Angel Sensitivity: `006E`
        setting(0x0004, 0x256f, Float(1.0)),  // Beam Sensitivity: `006B`
        setting(0x0004, 0x98b7, Bool(false)), // Only Prefer When No Target: `006E`
        // Reinhardt (`0007.075`)
        setting(0x0007, 0x1de1, Bool(false)), // Toggle Barrier: `0095`
        setting(0x0007, 0x2ee6, Bool(true)), // Movement Relative to Camera During Barrier Free Look: `0095`
        // Sigma (`023B.075`)
        setting(0x023b, 0x5c1f, Bool(false)), // Reload Recalls Barrier: `1B55`
        setting(0x023b, 0x5c43, Bool(false)), // Toggle Barrier: `1B55`
        // Soldier: 76 (`006E.075`)
        setting(0x006e, 0x1ddc, Bool(false)), // Hold to Sprint: `0255`
        // Sombra (`012E.075`)
        setting(0x012e, 0x2570, Float(1.0)), // Hack Sensitivity: `0B1B`
        // Widowmaker (`000A.075`)
        setting(0x000a, 0x1ddf, Bool(false)), // Toggle Zoom: `001B`
        setting(0x000a, 0x1de0, Float(0.3)),  // Relative Aim Sensitivity While Zoomed: `001B`
        setting(0x000a, 0x2571, Float(1.0)),  // Grappling Hook Sensitivity: `005A`
        setting(0x000a, 0x2942, Bool(true)),  // Recoil Recovery Aim Compensation: `001B`
        setting(0x000a, 0x5e16, Float(0.3)),  // Relative Gyro Aim Sensitivity While Zoomed: `001B`
        setting(0x000a, 0x6a43, Bool(false)), // Gyro Only In Ironsights: `001B`
        // Wrecking Ball (`01CA.075`)
        setting(0x01ca, 0x3fd4, Bool(false)), // Toggle Grappling Claw: `14E4`
        // Zarya (`0068.075`)
        setting(0x0068, 0x257b, Float(1.0)), // Projected Barrier Sensitivity: `025C`
        // Zenyatta (`0020.075`)
        setting(0x0020, 0x257c, Float(1.0)), // Orb of Harmony Sensitivity: `02F3`
        setting(0x0020, 0x257d, Float(1.0)), // Orb of Discord Sensitivity: `0176`
    ]
};

/// The [`SCRIPT_SETTINGS`] of `hero`'s body: his own and every hero's.
pub fn script_settings(hero: u64) -> impl Iterator<Item = &'static ScriptSetting> {
    SCRIPT_SETTINGS
        .iter()
        .filter(move |setting| setting.hero == 0 || setting.hero == hero)
}

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
