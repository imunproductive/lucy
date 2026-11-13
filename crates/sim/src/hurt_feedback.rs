//! The live hurt feedback switch (the `.hurt` chat command): what a hit a
//! player's body survives does to it, to compare in the game with the
//! behaviour before the hurt feedback (2026-11-12), when the server sent a
//! player's body no damage message.
//!
//! - `message`: the body's script gets the damage message `0011.025`
//!   ([`crate::combat::damage_message`]). Off, it gets nothing: no flinch,
//!   no knockback, no pain line, no "recently hurt" (`01CF`).
//! - `flinch`: the message carries the hit's damage tags. `02C5` flinches
//!   (#1024) only for its gun tags and starts the knockback (st7) only for
//!   `078D`, so without them the body neither flinches nor is knocked back.
//!   `004B` rates the hurt from the amount and `01CA`/`14B1` leave out only
//!   tags `191F` and `0262`, so the pain line still comes.
//! - `pain`: the body's pain criteria (`0608` minor, `0609` severe) give
//!   voice lines ([`crate::voice_lines`]). Off, the criteria are dropped.
//!
//! The training bots' and AI bots' hurt is not switched.

/// The criteria of the minor and severe pain lines.
const PAIN: [u64; 2] = [0x0d80_0000_0000_0608, 0x0d80_0000_0000_0609];

/// What a hit a player's body survives does to it. All on by default.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HurtFeedback {
    pub message: bool,
    pub flinch: bool,
    pub pain: bool,
}

impl Default for HurtFeedback {
    fn default() -> Self {
        Self {
            message: true,
            flinch: true,
            pain: true,
        }
    }
}

impl HurtFeedback {
    /// A change as the chat command gives it: `on` or `off` (the damage
    /// message, and with `on` the flinch and the pain line too), `flinch
    /// on|off`, `pain on|off`. An empty text or `show` changes nothing.
    pub fn change(&mut self, text: &str) -> Result<(), String> {
        let words: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        let switch = |word: &str| match word {
            "on" => Ok(true),
            "off" => Ok(false),
            other => Err(format!("'{other}' is not on or off.")),
        };
        match words[..] {
            [] | ["show"] => {}
            ["on"] => *self = Self::default(),
            ["off"] => self.message = false,
            ["flinch", value] => self.flinch = switch(value)?,
            ["pain", value] => self.pain = switch(value)?,
            _ => {
                return Err(
                    "Use .hurt, .hurt on|off, .hurt flinch on|off or .hurt pain on|off.".to_owned(),
                );
            }
        }
        Ok(())
    }

    /// What is in force, for the chat answer.
    pub fn summary(&self) -> String {
        let word = |on: bool| if on { "on" } else { "off" };
        if !self.message {
            return "Hurt feedback off: hits send your body no damage message.".to_owned();
        }
        format!(
            "Hurt feedback on: flinch and knockback {}, pain line {}.",
            word(self.flinch),
            word(self.pain)
        )
    }

    /// Whether the body says a line for `criteria`.
    pub fn speaks(&self, criteria: u64) -> bool {
        self.pain || !PAIN.contains(&criteria)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chat_changes_one_switch_and_on_restores_all() {
        let mut hurt = HurtFeedback::default();
        hurt.change("Pain OFF").unwrap();
        hurt.change("flinch off").unwrap();
        assert_eq!(
            hurt,
            HurtFeedback {
                message: true,
                flinch: false,
                pain: false
            }
        );
        assert!(!hurt.speaks(PAIN[1]));
        // A wheel line's criteria still speaks.
        assert!(hurt.speaks(0x0d80_0000_0000_0001));
        hurt.change("off").unwrap();
        assert!(!hurt.message);
        hurt.change("show").unwrap();
        assert!(!hurt.message);
        hurt.change("on").unwrap();
        assert_eq!(hurt, HurtFeedback::default());
        assert!(hurt.change("pain maybe").is_err());
        assert!(hurt.change("loud").is_err());
        assert_eq!(hurt, HurtFeedback::default());
    }
}
