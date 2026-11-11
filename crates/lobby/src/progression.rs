//! What a match that ended by its rules gives a player's profile: the match
//! in his counts, experience and the levels it makes with a loot box each,
//! the competitive rating and points, and the leaves of his last matches.
//! Python keeps none of this (`ow174` has no match results); the rules are
//! the public ones in `docs/notes/game-rules.md`, sections 9, 10, 16 and 17,
//! and each constant names its rule. Where a rule leaves a number open, the
//! choice is marked as an assumption.
//!
//! What the client is told, all through messages the lobby already builds:
//!
//! - 24313 `{+0x78 level, +0x80 experience}` (names from `ow174/content/
//!   __init__.py` `live_messages`). The experience sent is the experience
//!   into the current level [assumption: the client's meaning of +0x80 is
//!   not researched]. The login's 24300 keeps the captured +0x48/+0x50 (0 and
//!   2000), whose meaning is not known.
//! - 24302 the boxes (one per level gained), merged into the client's list.
//! - 36302/36300 the ratings, tiers and placements (`ranked.rs`), 24308 the
//!   competitive points.
//! - 20813 the leave policy: retail sent `{+0x78 0, +0x80 20, +0x88 0.3,
//!   +0x90 0.2}`. The research notes (`docs/notes/ow1-preservation.md`, the
//!   boi027 mapping) call +0x78 the leave mask and +0x80 the window ("leaver
//!   rule: last 20 games"), and the two floats thresholds; without the
//!   message the client shows "-75% XP / 0 matches remaining" from a leave
//!   rate of 0/0. The mask sent holds one bit per match of the window, the
//!   newest lowest, set for a leave [assumption: bit order]. The floats go
//!   as captured.
//!
//! Not done, for want of a rule or of data: medal experience (rule 177: the
//! match results carry no medals), endorsement levels (rule 168: levels 1-5
//! "with decay", but the gain is not public, `game-rules.md` gaps), the
//! endorsement-level boxes (rule 171: irregular cadence), SR decay (rule 79:
//! uncertain in 2022), competitive suspensions (rules 87, 89: no message is
//! known that refuses the queue for it), the level-25 gate on Competitive
//! (rule 80: a queue rule, not a result).

use std::io;

use orpheus_wire::jam::codec::{
    Fields,
    Value::{Float64, Unsigned},
};
use serde_json::Value as Json;

use crate::{
    accounts::Profile,
    clock::DAY,
    ranked::{self, MAX_MATCHES, Queue, draws_of, matches_of, wins_of},
};

/// Rule 177: 4.01 experience per second in the match.
const XP_PER_HUNDRED_SECONDS: i64 = 401;
/// Rule 177: a finished match gives 250 (Fextralife; Dot Esports says 150).
pub const MATCH_FINISH_XP: i64 = 250;
/// Rule 177: a win gives 500 more.
pub const WIN_XP: i64 = 500;
/// Rule 177: the first win of the day gives 1500. The sources conflict on
/// whether the first match or the first win earns it; it is the first win
/// here, as Dot Esports has it [assumption]. A day is the lobby clock's UTC
/// day (`clock::server_time`, the dashboard's date when it sets one).
pub const FIRST_WIN_XP: i64 = 1500;
/// Rule 177: playing in a group gives 20% more on the match's experience
/// [assumption: not on the first-win bonus].
const GROUP_PERCENT: i64 = 120;
/// Rule 125: Practice vs AI always gives 90%.
const VERSUS_AI_PERCENT: i64 = 90;
/// Rule 86: a player in leaver status gets 75% less on later matches.
const LEAVER_PERCENT: i64 = 25;
/// Rule 178: experience from each level to the next, levels 1 to 18 (the
/// launch table, PCS:13). Since November 2016 levels 14-100 cost "slightly"
/// less, by amounts not published; the launch values stay [assumption].
const LEVEL_XP: [i64; 18] = [
    1500, 3000, 4500, 6000, 7500, 8500, 9500, 10_500, 11_500, 12_500, 13_500, 14_500, 15_500,
    16_500, 17_500, 18_500, 19_000, 19_500,
];
/// Rule 178: level 19 and every later one cost 20000 (a flat 20000 after
/// level 100 since November 2016; the curve no longer resets).
const LATER_LEVEL_XP: i64 = 20_000;
/// Rule 179: one loot box per level, a standard one (`BOX_TYPES` id 0).
pub const LEVEL_UP_BOX: i32 = 0;
/// The leave policy's window (20813 +0x80, retail's 20): the matches whose
/// leaves count.
pub const LEAVE_WINDOW: u32 = 20;
/// The leave policy's two thresholds (20813 +0x88, +0x90) as retail sent
/// them. The server's leaver status (rule 86, "repeated early leaving")
/// starts at the first: 30% of the window, 6 leaves in the last 20 matches
/// [assumption: the thresholds' meaning is not researched].
const LEAVE_THRESHOLDS: [f64; 2] = [0.3, 0.2];

/// How a finished match went for a player's team.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Win,
    Loss,
    Draw,
}

/// A player's part in a match that ended by its rules.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MatchPlay {
    /// The queue the match counts in (`ranked::Queue`).
    pub queue: Queue,
    /// His team's result.
    pub outcome: Outcome,
    /// Still in the match at its end. A leaver's match is a loss (rule 88).
    pub finished: bool,
    /// A competitive match: the rating and the points move (section 9).
    pub competitive: bool,
    /// Practice vs AI (rule 125).
    pub versus_ai: bool,
    /// He played it with his group (rule 177).
    pub group: bool,
    /// Seconds he was in the match.
    pub seconds: f64,
}

/// What a match gave, for the log.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Earned {
    pub experience: i64,
    pub levels: i64,
    pub rating: i64,
    pub comp_points: i64,
    pub left: bool,
}

/// The match into the profile. `now` is the lobby's time (seconds since
/// 1970, `clock::server_time`).
pub fn apply(profile: &mut Profile, play: &MatchPlay, now: u64) -> io::Result<Earned> {
    let outcome = if play.finished {
        play.outcome
    } else {
        Outcome::Loss
    };
    let mut earned = Earned {
        left: !play.finished,
        ..Earned::default()
    };
    if play.competitive {
        earned.rating = ranked::rate(profile, play.queue, outcome, !play.finished)?;
        earned.comp_points = ranked::award_points(profile, outcome);
    }
    count(profile, play.queue, outcome)?;
    // Rule 86: the penalty falls on the matches after the leaves.
    let leaver = leaver(profile.leave_mask);
    profile.leave_mask = remember(profile.leave_mask, !play.finished);
    if !play.finished {
        return Ok(earned);
    }
    let day = i64::try_from(now / DAY).unwrap_or(i64::MAX);
    let first_win = outcome == Outcome::Win && profile.first_win_day != day;
    if first_win {
        profile.first_win_day = day;
    }
    earned.experience = experience(play, outcome, first_win, leaver);
    earned.levels = gain(profile, earned.experience)?;
    Ok(earned)
}

/// Rule 177: the match's experience.
fn experience(play: &MatchPlay, outcome: Outcome, first_win: bool, leaver: bool) -> i64 {
    let seconds = play.seconds.max(0.0) as i64;
    let mut xp = seconds * XP_PER_HUNDRED_SECONDS / 100 + MATCH_FINISH_XP;
    if outcome == Outcome::Win {
        xp += WIN_XP;
    }
    if play.group {
        xp = xp * GROUP_PERCENT / 100;
    }
    if first_win {
        xp += FIRST_WIN_XP;
    }
    if play.versus_ai {
        xp = xp * VERSUS_AI_PERCENT / 100;
    }
    if leaver {
        xp = xp * LEAVER_PERCENT / 100;
    }
    xp
}

/// Experience from `level` to the next (rule 178).
pub fn level_experience(level: i64) -> i64 {
    usize::try_from(level - 1)
        .ok()
        .and_then(|index| LEVEL_XP.get(index))
        .copied()
        .unwrap_or(if level < 1 {
            LEVEL_XP[0]
        } else {
            LATER_LEVEL_XP
        })
}

/// The experience into the profile, and the levels it makes, each with a
/// standard box (rule 179). Returns the levels gained.
fn gain(profile: &mut Profile, experience: i64) -> io::Result<i64> {
    profile.experience = profile.experience.max(0).saturating_add(experience);
    let mut levels = 0;
    loop {
        let needed = level_experience(profile.level);
        if profile.experience < needed {
            break;
        }
        profile.experience -= needed;
        profile.level = profile.level.max(0) + 1;
        levels += 1;
    }
    profile.add_boxes(LEVEL_UP_BOX, levels as u64)?;
    Ok(levels)
}

/// One more match in the queue, and one more win or draw. Counts go on from
/// what the lobby reads (a queue without a saved count reads 25 matches),
/// and stop at the most the competitive screen shows.
fn count(profile: &mut Profile, queue: Queue, outcome: Outcome) -> io::Result<()> {
    let matches = (i64::from(matches_of(profile, queue)?) + 1).min(MAX_MATCHES);
    let more = match outcome {
        Outcome::Win => Some((wins_of(profile, queue)?, &mut profile.wins)),
        Outcome::Draw => Some((draws_of(profile, queue)?, &mut profile.draws)),
        Outcome::Loss => None,
    };
    if let Some((done, counts)) = more {
        let done = (i64::from(done) + 1).min(matches);
        counts.insert(queue.name().into(), Json::from(done));
    }
    profile
        .matches
        .insert(queue.name().into(), Json::from(matches));
    Ok(())
}

/// The window's matches, one bit each.
const fn window_bits() -> u64 {
    (1 << LEAVE_WINDOW) - 1
}

/// The leave mask with one more match, the newest in the lowest bit.
fn remember(mask: u64, left: bool) -> u64 {
    ((mask << 1) | u64::from(left)) & window_bits()
}

/// Rule 86: leaver status, from the leaves among the window's matches.
pub fn leaver(mask: u64) -> bool {
    let leaves = (mask & window_bits()).count_ones();
    f64::from(leaves) / f64::from(LEAVE_WINDOW) >= LEAVE_THRESHOLDS[0]
}

/// 20813: the leave policy with the profile's leave mask.
pub fn leave_policy(profile: &Profile) -> Fields {
    Fields::from([
        (0x78, Unsigned(profile.leave_mask & window_bits())),
        (0x80, Unsigned(u64::from(LEAVE_WINDOW))),
        (0x88, Float64(LEAVE_THRESHOLDS[0])),
        (0x90, Float64(LEAVE_THRESHOLDS[1])),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    use crate::{
        ranked::{rating_of, tier_of},
        templates::{LOBBY, RetailTemplates},
    };

    const NOON: u64 = 19_000 * DAY + DAY / 2;

    fn play(outcome: Outcome) -> MatchPlay {
        MatchPlay {
            queue: Queue::Open,
            outcome,
            finished: true,
            competitive: false,
            versus_ai: false,
            group: false,
            seconds: 0.0,
        }
    }

    /// A profile that has had its first win today, at level 30 with no
    /// experience into it.
    fn profile() -> Profile {
        Profile {
            level: 30,
            experience: 0,
            first_win_day: (NOON / DAY) as i64,
            ..Profile::default()
        }
    }

    fn earned(profile: &mut Profile, play: MatchPlay) -> Earned {
        apply(profile, &play, NOON).unwrap()
    }

    /// Rule 177: 4.01 a second, 250 for the finish, 500 more for a win; a
    /// draw is a finish only.
    #[test]
    fn a_match_gives_time_finish_and_win_experience() {
        let ten_minutes = |outcome| MatchPlay {
            seconds: 600.9,
            ..play(outcome)
        };
        let mut player = profile();
        assert_eq!(
            earned(&mut player, ten_minutes(Outcome::Loss)).experience,
            2406 + 250
        );
        assert_eq!(
            earned(&mut player, ten_minutes(Outcome::Draw)).experience,
            2406 + 250
        );
        assert_eq!(
            earned(&mut player, ten_minutes(Outcome::Win)).experience,
            2406 + 250 + 500
        );
        assert_eq!(player.experience, 3 * 2656 + 500);
    }

    /// Rule 177: the first win of a day gives 1500 more, once that day.
    #[test]
    fn the_first_win_of_the_day_gives_1500_once() {
        let mut player = Profile {
            first_win_day: 0,
            ..profile()
        };
        assert_eq!(earned(&mut player, play(Outcome::Loss)).experience, 250);
        assert_eq!(
            earned(&mut player, play(Outcome::Win)).experience,
            750 + 1500
        );
        assert_eq!(earned(&mut player, play(Outcome::Win)).experience, 750);
        assert_eq!(
            apply(&mut player, &play(Outcome::Win), NOON + DAY)
                .unwrap()
                .experience,
            750 + 1500
        );
    }

    /// Rules 177 and 125: a group gets 20% more, Practice vs AI 90%.
    #[test]
    fn groups_get_more_and_versus_ai_less() {
        let mut player = profile();
        let group = MatchPlay {
            group: true,
            ..play(Outcome::Win)
        };
        assert_eq!(earned(&mut player, group).experience, 900);
        let versus_ai = MatchPlay {
            versus_ai: true,
            ..play(Outcome::Win)
        };
        assert_eq!(earned(&mut player, versus_ai).experience, 675);
    }

    /// Rule 178: levels 1-18 from the launch table, 20000 from level 19;
    /// rule 179: one standard box per level gained.
    #[test]
    fn experience_makes_levels_with_a_box_each() {
        assert_eq!(level_experience(1), 1500);
        assert_eq!(level_experience(13), 15_500);
        assert_eq!(level_experience(18), 19_500);
        assert_eq!(level_experience(19), 20_000);
        assert_eq!(level_experience(250), 20_000);

        let mut player = Profile {
            level: 1,
            experience: 1000,
            loot_boxes: Vec::new(),
            ..profile()
        };
        // 1000 + 4260 = 5260: 1500 to level 2, 3000 to level 3, 760 left.
        let long = MatchPlay {
            seconds: 1000.0,
            ..play(Outcome::Loss)
        };
        let earned = earned(&mut player, long);
        assert_eq!((earned.experience, earned.levels), (4260, 2));
        assert_eq!((player.level, player.experience), (3, 760));
        assert_eq!(player.loot_boxes.len(), 2);
        assert!(player.loot_boxes.iter().all(|item| item.box_type == 0));
    }

    /// Rule 88: a leaver's match counts as a loss and gives nothing else;
    /// rule 86: 6 leaves in the last 20 matches make the next matches give
    /// 25%, until finished matches push the leaves out of the window.
    #[test]
    fn leaves_count_as_losses_and_repeated_leaves_cut_experience() {
        let mut player = profile();
        player.matches.insert("open".into(), json!(40));
        player.wins.insert("open".into(), json!(20));
        let left = MatchPlay {
            finished: false,
            ..play(Outcome::Win)
        };
        for leaves in 1..=6 {
            let earned = earned(&mut player, left);
            assert_eq!((earned.experience, earned.left), (0, true));
            assert_eq!(leaver(player.leave_mask), leaves == 6);
        }
        assert_eq!(player.leave_mask, 0b11_1111);
        assert_eq!(matches_of(&player, Queue::Open).unwrap(), 46);
        assert_eq!(wins_of(&player, Queue::Open).unwrap(), 20);

        assert_eq!(earned(&mut player, play(Outcome::Win)).experience, 750 / 4);
        // Up to the 20th match since the first leave, all 6 stay in the
        // window.
        for _ in 0..14 {
            assert_eq!(earned(&mut player, play(Outcome::Loss)).experience, 250 / 4);
        }
        // The 21st pushes the first leave out: 5 of 20 is under the line.
        assert_eq!(earned(&mut player, play(Outcome::Loss)).experience, 250);
        assert_eq!(player.leave_mask.count_ones(), 4);
    }

    /// 20813 is retail's captured leave policy with the profile's mask.
    #[test]
    fn the_leave_policy_is_the_captured_one_with_the_mask() {
        let mut player = profile();
        assert_eq!(
            leave_policy(&player),
            RetailTemplates::captured().first(LOBBY, 20813).unwrap()
        );
        player.leave_mask = 0b101 | (1 << 25);
        assert_eq!(leave_policy(&player)[&0x78], Unsigned(0b101));
    }

    /// Section 9 on a competitive match: the rating moves (none on a draw),
    /// the points come (15 a win, 5 a draw), the leaver loses 50.
    #[test]
    fn competitive_matches_move_the_rating_and_give_points() {
        let mut player = profile();
        player.ratings.insert("damage".into(), json!(2600));
        player.comp_points = 100;
        let competitive = |outcome| MatchPlay {
            queue: Queue::Damage,
            competitive: true,
            ..play(outcome)
        };
        let win = earned(&mut player, competitive(Outcome::Win));
        assert_eq!((win.rating, win.comp_points), (25, 15));
        let draw = earned(&mut player, competitive(Outcome::Draw));
        assert_eq!((draw.rating, draw.comp_points), (0, 5));
        let loss = earned(&mut player, competitive(Outcome::Loss));
        assert_eq!((loss.rating, loss.comp_points), (-25, 0));
        let left = earned(
            &mut player,
            MatchPlay {
                finished: false,
                ..competitive(Outcome::Win)
            },
        );
        assert_eq!((left.rating, left.comp_points), (-50, 0));
        assert_eq!(rating_of(&player, Queue::Damage).unwrap(), 2550);
        assert_eq!(tier_of(&player, Queue::Damage).unwrap(), 3);
        assert_eq!(player.comp_points, 120);

        // Not competitive: no rating, no points.
        let quick = earned(&mut player, play(Outcome::Win));
        assert_eq!((quick.rating, quick.comp_points), (0, 0));
    }
}
