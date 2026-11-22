//! The chat notices of a match (ow174/game/notices.py): a player joined or
//! left the game, and a hero switch. The clients have the texts; the server
//! only triggers them.
//!
//! 20308 {4 flags, card} keeps the card and, with +0x78 and +0x7B on, posts
//! the card's name and " left the game." (+0x79 off) or " joined the game."
//! (+0x79 and +0x7A on). The hero switch line is graph data of the PvP
//! controller 0C90: the server sends 037F.025 to the switching player's
//! controller script, whose rid220 keeps three presence variables (the old
//! hero, the new one and the player) that a client-only Watch posts once
//! (`lucy_script::controller::ControllerScript::hero_switched`). They go
//! in one controller frame only, and only to a client whose last frame had
//! none of them.
//!
//! Client research (1.74): 20308 is handled at 0x7FF7896E88B0; " left the
//! game." is 112C.07C (+0x79 off), " joined the game." 112B.07C (+0x7A on),
//! " started spectating." 2C0B.07C (+0x7A off). These are the only "NAME +
//! text" lines of a match; the client's other ones (table 0x7FF78B5209B0) come
//! from the lobby. "%1$s switched to %2$s (was %3$s)." (0AA4.07C): 0C90
//! stores v4513 (old hero), v4514 (new) and v4515 (player) from game message
//! 037F.025, and a client-only Watch on v4514 posts the line (action
//! STU_7106C356, 0x7FF7899E15D0) when v4514 changed and the old hero is a
//! different hero. The name is that entity's component 29 m_battleTag
//! (0x7FF789CB3F80); the line shows only if the controller running the graph
//! is the local player or a teammate (0x7FF789B2C0E0; in free-for-all
//! everyone is an enemy, team 4, 0x7FF789B2CF60). A full frame first clears
//! the network-set variables, telling the Watch at once (0x7FF78AAB45B0 ->
//! 0x7FF78AA9ED40), so re-sending the three would post the line again. Retail
//! most likely sent 037F to the switching player's controller; only modes
//! whose teams use 0C90 have the notice (free for all 0F74, capture the flag
//! 0CA2 and elimination 0D2A use other controllers).

use std::time::Duration;

use crate::scene::PlayerCard;
use lucy_content::maps::GameMode;
use lucy_script::controller::HeroSwitch;
use lucy_wire::{
    jam::codec::{Fields, Value, id16},
    messages::Message,
};

/// ClientInGame 20308: a player's card for the client's player list.
pub const PLAYER_CARD: u32 = 20308;

/// The PvP controller graph `0C90` (an `01B`), whose Watch posts the hero
/// switch line.
const SWITCH_CONTROLLER: u64 = 0x0580_0000_0000_0c90;

/// A notice frame waits until the client has every controller frame sent
/// before it, and a little more: the Watch's events of one frame have to run
/// before the next frame changes the variables again.
pub(crate) const NOTICE_GAP: Duration = Duration::from_millis(100);

/// Whether a mode shows hero switches: every team's controller graph is 0C90
/// (`m_teams[].m_controllerScript` of the mode's `0C5`; notices.py
/// `SWITCH_MODES`). Free for all (0F74), capture the flag (0CA2),
/// elimination (0D2A) and the rest have other controllers without it, and
/// the solo Practice Range (0018) has 13C1 for its blue team. The group
/// Practice Range the server runs (005A) has 0C90 for both teams, so it
/// shows them (Python ran 0018, without them).
pub fn switch_notices(mode: Option<&GameMode>) -> bool {
    mode.is_some_and(|mode| {
        !mode.teams.is_empty()
            && mode
                .teams
                .iter()
                .all(|team| team.controller == Some(SWITCH_CONTROLLER))
    })
}

/// The card struct of 20308 +0x80. The lobby supplies it (its player
/// record); without one, only the ids and the name, as match.py's Player
/// builds it.
pub(crate) fn card_record(card: &PlayerCard) -> Fields {
    if !card.record.is_empty() {
        return card.record.clone();
    }
    let [low, high] = card.account;
    Fields::from([
        (0x00, Value::Struct(id16(low, high))),
        (0x10, Value::Struct(id16(low, high))),
        (0x40, Value::String(card.battle_tag.clone())),
    ])
}

/// 20308 with a card and no text (match.py card_message).
pub fn card_message(card: Fields) -> Message {
    message([Some(false), None, None, None], card)
}

/// 20308: "<name> joined the game." (+0x79 and +0x7A on).
pub fn joined_message(card: Fields) -> Message {
    message([Some(true); 4], card)
}

/// 20308: "<name> left the game." (+0x79 off).
pub fn left_message(card: Fields) -> Message {
    message([Some(true), Some(false), Some(false), Some(true)], card)
}

fn message(flags: [Option<bool>; 4], card: Fields) -> Message {
    let mut fields: Fields = (0x78..)
        .zip(flags)
        .filter_map(|(offset, flag)| Some((offset, Value::Bool(flag?))))
        .collect();
    fields.insert(0x80, Value::Struct(card));
    Message {
        id: PLAYER_CARD,
        fields,
    }
}

/// One hero switch as the switching player's 0C90 keeps it (#4513 the old
/// hero, #4514 the new one, #4515 his controller entity, whose component
/// 29 names him).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Switch {
    pub kept: HeroSwitch,
    /// Counts the player's switches, so each client takes each one once.
    pub serial: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use lucy_content::maps::{GameModes, MODES_FILE, ModeTeam};

    #[test]
    fn cards_carry_the_python_flags() {
        let card = Fields::from([(0x40, Value::String("left#1234".into()))]);
        let flags = |message: &Message| {
            (0x78..0x7c)
                .map(|offset| message.fields.get(&offset).cloned())
                .collect::<Vec<_>>()
        };
        let on = Some(Value::Bool(true));
        let off = Some(Value::Bool(false));
        let plain = card_message(card.clone());
        assert_eq!(plain.id, PLAYER_CARD);
        assert_eq!(flags(&plain), [off.clone(), None, None, None]);
        assert_eq!(plain.fields[&0x80], Value::Struct(card.clone()));
        assert_eq!(flags(&joined_message(card.clone())), vec![on.clone(); 4]);
        assert_eq!(
            flags(&left_message(card)),
            [on.clone(), off.clone(), off, on]
        );
    }

    #[test]
    fn a_lobby_card_goes_as_it_is_and_a_missing_one_has_ids_and_name() {
        let mut card = PlayerCard {
            account: [7, 0x0100_0000_0000_0000],
            battle_tag: "rust#2345".into(),
            ..PlayerCard::default()
        };
        assert_eq!(
            card_record(&card),
            Fields::from([
                (0, Value::Struct(id16(7, 0x0100_0000_0000_0000))),
                (0x10, Value::Struct(id16(7, 0x0100_0000_0000_0000))),
                (0x40, Value::String("rust#2345".into())),
            ])
        );
        card.record = Fields::from([(0x38, Value::Signed(25))]);
        assert_eq!(card_record(&card), card.record);
    }

    #[test]
    fn only_the_0c90_switch_modes_post_hero_switches() {
        let mode = |controllers: &[Option<u64>]| GameMode {
            teams: controllers
                .iter()
                .enumerate()
                .map(|(team, &controller)| ModeTeam {
                    team: team as i64,
                    controller,
                    body: None,
                    players: 6,
                })
                .collect(),
            scripted_loading: false,
        };
        let pvp = Some(SWITCH_CONTROLLER);
        assert!(switch_notices(Some(&mode(&[pvp, pvp]))));
        // The solo Practice Range (0018): 13C1 for the player's team.
        assert!(!switch_notices(Some(&mode(&[
            Some(0x0580_0000_0000_13c1),
            pvp
        ]))));
        assert!(!switch_notices(Some(&mode(&[pvp, None]))));
        assert!(!switch_notices(Some(&mode(&[]))));
        assert!(!switch_notices(None));
    }

    /// The install's 0C90 modes are the ones notices.py lists.
    #[test]
    #[ignore = "requires the derived 104319 game modes"]
    fn the_switch_modes_are_the_install_s() {
        let modes =
            GameModes::load(lucy_testkit::runtime_dir().join("data").join(MODES_FILE)).unwrap();
        let mut switching: Vec<u64> = modes
            .iter()
            .filter(|(_, mode)| switch_notices(Some(mode)))
            .map(|(guid, _)| guid)
            .collect();
        switching.sort_unstable();
        assert_eq!(
            switching,
            [
                0x0230_0000_0000_0010,
                0x0230_0000_0000_0014,
                0x0230_0000_0000_0015,
                0x0230_0000_0000_0016,
                0x0230_0000_0000_0017,
                0x0230_0000_0000_005a,
                0x0230_0000_0000_0067,
            ]
        );
    }
}
