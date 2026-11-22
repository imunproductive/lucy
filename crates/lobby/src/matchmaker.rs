//! The matchmaker's rules: queue cards, the roster search, open matches and
//! map choice. Ports the pure parts of `ow174/lobby/matchmaker.py`.
//!
//! A queue card's rules (`data/game_queues_174.json`,
//! `tools/extract_game_queues.py`) give its teams, the players per team, the
//! role slots of a role queue (two of each role per team) and its maps. A party
//! always plays on one team. A match starts once the waiting parties fill
//! every team exactly and, in a role queue, every player gets one of the roles
//! they picked. Parties that waited longest go first. For tests, a minimum
//! number of players starts a match early with whoever is waiting.

use std::{
    collections::{BTreeMap, HashMap},
    fs, io,
    path::Path,
};

use serde_json::Value as Json;

use crate::queue::{DAMAGE_ROLE, SUPPORT_ROLE, TANK_ROLE, role_number};
use lucy_content::maps::{GameMap, MapCatalog};

pub const QUICK_PLAY: u64 = 0x0630_0000_0000_00ed;
/// What "All roles" picks: tank, damage, support.
pub const ANY_ROLE: [u8; 3] = [TANK_ROLE, DAMAGE_ROLE, SUPPORT_ROLE];
/// Roster search steps per attempt.
const SEARCH_LIMIT: usize = 20_000;
/// The teams of the queue data, in team index order.
const TEAMS: [&str; 2] = ["blue", "red"];

/// One way a card is played: its game mode and the players each team takes.
#[derive(Clone, Debug, PartialEq)]
pub struct Ruleset {
    /// The game ruleset (`0C0`); 0: none known.
    pub ruleset: u64,
    pub mode: u64,
    pub mode_name: String,
    /// Per team; free for all: one entry, the number of players.
    pub team_sizes: Vec<u8>,
    pub free_for_all: bool,
}

impl Default for Ruleset {
    fn default() -> Self {
        Self {
            ruleset: 0,
            mode: 0,
            mode_name: String::new(),
            team_sizes: vec![6, 6],
            free_for_all: false,
        }
    }
}

impl Ruleset {
    /// Places per team: free for all gives every player a team of their own.
    fn capacity(&self) -> Vec<usize> {
        if self.free_for_all {
            vec![1; usize::from(self.team_sizes[0])]
        } else {
            self.team_sizes
                .iter()
                .map(|&size| usize::from(size))
                .collect()
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct QueueRules {
    pub name: String,
    pub rulesets: Vec<Ruleset>,
    /// Role number to slots per team; empty means any roles.
    pub roles: BTreeMap<u8, usize>,
    /// Map GUID and the modes it is played in.
    pub maps: Vec<(u64, Vec<u64>)>,
    pub competitive: bool,
    /// The teams the server fills with AI heroes, by team index (0 blue, 1
    /// red), and their AI level (1 to 3: Easy, Medium, Hard). The Practice
    /// vs AI cards (`0x0630000000000070`..`72`) have the red team at their
    /// level (`game_queues_174.json` `ai_teams`, from the card's
    /// `m_7FB46D96`).
    pub ai_teams: BTreeMap<u8, u8>,
}

impl QueueRules {
    /// A Practice vs AI card: its matches have an AI team.
    pub fn versus_ai(&self) -> bool {
        !self.ai_teams.is_empty()
    }

    /// The players of a ruleset the matchmaker places: none on the AI
    /// teams.
    pub fn human_ruleset(&self, ruleset: &Ruleset) -> Ruleset {
        let mut humans = ruleset.clone();
        for (team, size) in humans.team_sizes.iter_mut().enumerate() {
            if u8::try_from(team).is_ok_and(|team| self.ai_teams.contains_key(&team)) {
                *size = 0;
            }
        }
        humans
    }
}

impl Default for QueueRules {
    fn default() -> Self {
        Self {
            name: String::new(),
            rulesets: vec![Ruleset::default()],
            roles: BTreeMap::new(),
            maps: Vec::new(),
            competitive: false,
            ai_teams: BTreeMap::new(),
        }
    }
}

/// A queue card's own matchmaking settings, set in the dashboard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModeSettings {
    /// 0: none of its own, the server-wide number counts.
    pub players_to_start: usize,
    /// Unranked: players who search join its matches that are on and have
    /// room.
    pub fill_running: bool,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn hex(value: &Json) -> io::Result<u64> {
    value
        .as_str()
        .and_then(|text| {
            u64::from_str_radix(text.trim_start_matches("0x").trim_start_matches("0X"), 16).ok()
        })
        .ok_or_else(|| invalid(format!("invalid GUID in the queue rules: {value}")))
}

fn whole(value: &Json) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
}

/// The queue cards' rules from `game_queues_174.json`; none when the file is
/// missing. Cards without rulesets are left out.
pub fn load_rules(path: &Path) -> io::Result<HashMap<u64, QueueRules>> {
    match fs::read_to_string(path) {
        Ok(text) => parse_rules(&text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(error) => Err(error),
    }
}

pub fn parse_rules(text: &str) -> io::Result<HashMap<u64, QueueRules>> {
    let table: Json = serde_json::from_str(text).map_err(|error| invalid(error.to_string()))?;
    let Some(cards) = table.get("cards").and_then(Json::as_object) else {
        return Ok(HashMap::new());
    };
    let mut rules = HashMap::new();
    for (key, entry) in cards {
        let has_rulesets = entry
            .get("rulesets")
            .and_then(Json::as_array)
            .is_some_and(|rulesets| !rulesets.is_empty());
        if has_rulesets {
            rules.insert(hex(&Json::String(key.clone()))?, card_rules(entry)?);
        }
    }
    Ok(rules)
}

fn card_rules(entry: &Json) -> io::Result<QueueRules> {
    let mut roles = BTreeMap::new();
    for (guid, count) in entry
        .get("roles")
        .and_then(Json::as_object)
        .into_iter()
        .flatten()
    {
        let number = role_number(hex(&Json::String(guid.clone()))?);
        let count = whole(count).ok_or_else(|| invalid("invalid role slot count"))?;
        if let (Some(number), Ok(count @ 1..)) = (number, usize::try_from(count)) {
            roles.insert(number, count);
        }
    }
    let mut maps = Vec::new();
    for item in entry
        .get("maps")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
    {
        let map = hex(item.get("map").unwrap_or(&Json::Null))?;
        let modes = item
            .get("modes")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .map(hex)
            .collect::<io::Result<_>>()?;
        maps.push((map, modes));
    }
    let rulesets = entry["rulesets"]
        .as_array()
        .into_iter()
        .flatten()
        .map(ruleset)
        .collect::<io::Result<_>>()?;
    let mut ai_teams = BTreeMap::new();
    for (side, level) in entry
        .get("ai_teams")
        .and_then(Json::as_object)
        .into_iter()
        .flatten()
    {
        let team = TEAMS
            .iter()
            .position(|name| name == side)
            .ok_or_else(|| invalid(format!("unknown AI team {side}")))?;
        let level = whole(level)
            .and_then(|level| u8::try_from(level).ok())
            .ok_or_else(|| invalid("invalid AI level"))?;
        ai_teams.insert(team as u8, level);
    }
    Ok(QueueRules {
        name: entry
            .get("name")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .into(),
        rulesets,
        roles,
        maps,
        competitive: entry
            .get("competitive")
            .and_then(Json::as_bool)
            .unwrap_or(false),
        ai_teams,
    })
}

fn ruleset(item: &Json) -> io::Result<Ruleset> {
    let size = |value: &Json| {
        whole(value)
            .and_then(|size| u8::try_from(size).ok())
            .ok_or_else(|| invalid("invalid team size"))
    };
    let free_for_all = item
        .get("free_for_all")
        .and_then(Json::as_bool)
        .unwrap_or(false);
    let team_sizes = if free_for_all {
        match item.get("players") {
            Some(players) if whole(players).is_some_and(|players| players != 0) => {
                vec![size(players)?]
            }
            _ => vec![8],
        }
    } else {
        let by_team = item.get("team_sizes").and_then(Json::as_object);
        let sizes = TEAMS
            .into_iter()
            .filter_map(|team| by_team.and_then(|teams| teams.get(team)))
            .map(size)
            .collect::<io::Result<Vec<_>>>()?;
        if sizes.is_empty() { vec![6, 6] } else { sizes }
    };
    Ok(Ruleset {
        ruleset: item.get("ruleset").map(hex).transpose()?.unwrap_or(0),
        mode: hex(item.get("mode").unwrap_or(&Json::Null))?,
        mode_name: item
            .get("mode_name")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .into(),
        team_sizes,
        free_for_all,
    })
}

/// The cards' own settings the dashboard saved:
/// `{"0x...": {"players_to_start", "fill_running"}}`. A file that cannot be
/// read gives no settings; an entry that cannot be read is skipped.
pub fn load_modes(path: &Path) -> HashMap<u64, ModeSettings> {
    let Ok(text) = fs::read_to_string(path) else {
        return HashMap::new();
    };
    let Ok(Json::Object(saved)) = serde_json::from_str::<Json>(&text) else {
        return HashMap::new();
    };
    saved
        .iter()
        .filter_map(|(key, value)| {
            let card = hex(&Json::String(key.clone())).ok()?;
            let players = usize::try_from(whole(value.get("players_to_start")?)?).ok()?;
            let fill = value.get("fill_running")?;
            let fill = fill
                .as_bool()
                .or_else(|| whole(fill).map(|fill| fill != 0))?;
            Some((
                card,
                ModeSettings {
                    players_to_start: players,
                    fill_running: fill,
                },
            ))
        })
        .collect()
}

/// Write the cards' settings as `load_modes` reads them, sorted by card, in
/// the layout of Python's `json.dumps(indent=2)`.
pub fn save_modes(path: &Path, modes: &HashMap<u64, ModeSettings>) -> io::Result<()> {
    let mut cards: Vec<_> = modes.iter().collect();
    cards.sort_unstable_by_key(|(card, _)| **card);
    let entries: Vec<String> = cards
        .into_iter()
        .map(|(card, mode)| {
            format!(
                "  \"0x{card:X}\": {{\n    \"players_to_start\": {},\n    \"fill_running\": {}\n  }}",
                mode.players_to_start, mode.fill_running
            )
        })
        .collect();
    let text = if entries.is_empty() {
        "{}\n".to_owned()
    } else {
        format!("{{\n{}\n}}\n", entries.join(",\n"))
    };
    fs::write(path, text)
}

/// Give each player one of their roles within the slots: the role of each
/// player in order, or `None`.
pub fn match_roles(players: &[Vec<u8>], slots: &BTreeMap<u8, usize>) -> Option<Vec<u8>> {
    let mut taken: BTreeMap<u8, Vec<usize>> =
        slots.keys().map(|&role| (role, Vec::new())).collect();

    fn place(
        index: usize,
        players: &[Vec<u8>],
        slots: &BTreeMap<u8, usize>,
        taken: &mut BTreeMap<u8, Vec<usize>>,
        seen: &mut Vec<u8>,
    ) -> bool {
        for &role in &players[index] {
            if !slots.contains_key(&role) || seen.contains(&role) {
                continue;
            }
            seen.push(role);
            if taken[&role].len() < slots[&role] {
                taken.get_mut(&role).expect("slot role").push(index);
                return true;
            }
            for other in taken[&role].clone() {
                let holders = taken.get_mut(&role).expect("slot role");
                let at = holders
                    .iter()
                    .position(|&held| held == other)
                    .expect("holder");
                holders.remove(at);
                if place(other, players, slots, taken, seen) {
                    taken.get_mut(&role).expect("slot role").push(index);
                    return true;
                }
                taken.get_mut(&role).expect("slot role").push(other);
            }
        }
        false
    }

    for index in 0..players.len() {
        if !place(index, players, slots, &mut taken, &mut Vec::new()) {
            return None;
        }
    }
    let mut given = vec![0; players.len()];
    for (role, indexes) in taken {
        for index in indexes {
            given[index] = role;
        }
    }
    Some(given)
}

/// A party that waits: the roles each member picked, best first (empty: any).
pub type Seekers = [Vec<u8>];

/// One player of a roster: the party (its index among the waiting parties),
/// the member (its index in the party) and the role it plays (0: any).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pick {
    pub party: usize,
    pub member: usize,
    pub role: u8,
}

/// The players of each team.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Roster {
    pub teams: Vec<Vec<Pick>>,
}

fn picks_or_any(roles: &[u8]) -> Vec<u8> {
    if roles.is_empty() {
        ANY_ROLE.to_vec()
    } else {
        roles.to_vec()
    }
}

/// Pick waiting parties that fill the teams (oldest first), or `None` when
/// they can't yet. `minimum` > 0 starts a match with fewer players once that
/// many wait (never more than the mode holds): as many of the waiting players
/// as fit go in, and a role queue's slots don't count.
pub fn build_roster(
    parties: &[&Seekers],
    ruleset: &Ruleset,
    roles: &BTreeMap<u8, usize>,
    minimum: usize,
) -> Option<Roster> {
    let capacity = ruleset.capacity();
    let full: usize = capacity.iter().sum();
    let role_queue = !roles.is_empty();
    if minimum == 0 || minimum >= full {
        return fill(parties, ruleset, &capacity, roles, full, role_queue);
    }
    let waiting: usize = parties.iter().map(|party| party.len()).sum();
    (minimum..=waiting.min(full)).rev().find_map(|goal| {
        fill(
            parties,
            ruleset,
            &capacity,
            &BTreeMap::new(),
            goal,
            role_queue,
        )
    })
}

struct Fill<'a> {
    parties: &'a [&'a Seekers],
    free_for_all: bool,
    capacity: &'a [usize],
    roles: &'a BTreeMap<u8, usize>,
    goal: usize,
    chosen: Vec<Vec<(usize, usize)>>,
    steps: usize,
}

impl Fill<'_> {
    fn picks(&self, team: &[(usize, usize)]) -> Vec<Vec<u8>> {
        team.iter()
            .map(|&(party, member)| picks_or_any(&self.parties[party][member]))
            .collect()
    }

    fn fits(&self, team: usize, extra: &[(usize, usize)]) -> bool {
        if self.chosen[team].len() + extra.len() > self.capacity[team] {
            return false;
        }
        if self.roles.is_empty() {
            return true;
        }
        let mut members = self.chosen[team].clone();
        members.extend_from_slice(extra);
        match_roles(&self.picks(&members), self.roles).is_some()
    }

    fn search(&mut self, start: usize, count: usize) -> bool {
        if count >= self.goal {
            return true;
        }
        let teams = self.capacity.len();
        for index in start..self.parties.len() {
            self.steps += 1;
            if self.steps > SEARCH_LIMIT {
                return false;
            }
            let extra: Vec<_> = (0..self.parties[index].len())
                .map(|member| (index, member))
                .collect();
            if self.free_for_all {
                // Every player on a team of their own.
                let free: Vec<_> = (0..teams)
                    .filter(|&team| self.chosen[team].is_empty())
                    .collect();
                if free.len() < extra.len() {
                    continue;
                }
                for (&team, &entry) in free.iter().zip(&extra) {
                    self.chosen[team].push(entry);
                }
                if self.search(index + 1, count + extra.len()) {
                    return true;
                }
                for &team in &free[..extra.len()] {
                    self.chosen[team].clear();
                }
                continue;
            }
            let mut order: Vec<usize> = (0..teams).collect();
            order.sort_by_key(|&team| {
                self.chosen[team].len() as isize - self.capacity[team] as isize
            });
            for team in order {
                if self.fits(team, &extra) {
                    self.chosen[team].extend_from_slice(&extra);
                    if self.search(index + 1, count + extra.len()) {
                        return true;
                    }
                    let kept = self.chosen[team].len() - extra.len();
                    self.chosen[team].truncate(kept);
                }
            }
        }
        false
    }
}

/// At least `goal` players from the waiting parties, oldest first: a party
/// always on one team, the emptiest team first, and each player in a role they
/// picked while `roles` gives the slots.
fn fill(
    parties: &[&Seekers],
    ruleset: &Ruleset,
    capacity: &[usize],
    roles: &BTreeMap<u8, usize>,
    goal: usize,
    role_queue: bool,
) -> Option<Roster> {
    let mut search = Fill {
        parties,
        free_for_all: ruleset.free_for_all,
        capacity,
        roles,
        goal,
        chosen: vec![Vec::new(); capacity.len()],
        steps: 0,
    };
    if !search.search(0, 0) {
        return None;
    }
    let teams = search
        .chosen
        .iter()
        .map(|team| {
            let picks = search.picks(team);
            let given = if !roles.is_empty() {
                match_roles(&picks, roles).unwrap_or_else(|| vec![0; team.len()])
            } else if role_queue {
                // Starting with fewer players drops a role queue's slots: each
                // player plays the first role they picked (hero select offers
                // its heroes; a competitive card's rank is the role's).
                picks.iter().map(|pick| pick[0]).collect()
            } else {
                vec![0; team.len()]
            };
            team.iter()
                .zip(given)
                .map(|(&(party, member), role)| Pick {
                    party,
                    member,
                    role,
                })
                .collect()
        })
        .collect();
    Some(Roster { teams })
}

/// A match that still has free places, as the matchmaker sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenMatch {
    /// Still waiting for its players: hero select has not started counting
    /// down.
    pub waiting: bool,
    /// Free places per team (one entry for free for all).
    pub free: Vec<usize>,
    /// Per team: role queue role to players playing it.
    pub roles: Vec<BTreeMap<u8, usize>>,
}

/// Where a whole party fits into an open match: the team and the role of each
/// member; the team with the most room first. With role slots, only a team
/// that has free slots in roles the members picked.
pub fn place(
    party: &Seekers,
    room: &OpenMatch,
    slots: &BTreeMap<u8, usize>,
    role_queue: bool,
) -> Option<(usize, Vec<u8>)> {
    let picks: Vec<_> = party.iter().map(|roles| picks_or_any(roles)).collect();
    let mut order: Vec<usize> = (0..room.free.len()).collect();
    order.sort_by_key(|&team| std::cmp::Reverse(room.free[team]));
    for team in order {
        if room.free[team] < picks.len() {
            continue;
        }
        if slots.is_empty() {
            let given = if role_queue {
                picks.iter().map(|pick| pick[0]).collect()
            } else {
                vec![0; picks.len()]
            };
            return Some((team, given));
        }
        let taken = room.roles.get(team);
        let left: BTreeMap<u8, usize> = slots
            .iter()
            .map(|(&role, &count)| {
                let used = taken
                    .and_then(|taken| taken.get(&role))
                    .copied()
                    .unwrap_or(0);
                (role, count.saturating_sub(used))
            })
            .filter(|&(_, count)| count > 0)
            .collect();
        if let Some(given) = match_roles(&picks, &left) {
            return Some((team, given));
        }
    }
    None
}

/// The maps the server can host from these maps, each with the ruleset it is
/// played in. A map with no modes allows any ruleset (so a forced map is tried
/// against every mode the card plays).
pub fn map_choices<'a>(
    catalog: &MapCatalog,
    maps: &[(u64, Vec<u64>)],
    rulesets: &'a [Ruleset],
) -> Vec<(GameMap, &'a Ruleset)> {
    let mut choices = Vec::new();
    for (map, modes) in maps {
        for ruleset in rulesets {
            if !modes.is_empty() && !modes.contains(&ruleset.mode) {
                continue;
            }
            let found = catalog.game_map(
                *map,
                ruleset.mode,
                &ruleset.mode_name,
                ruleset.free_for_all,
                &ruleset.team_sizes,
            );
            if let Some(mut found) = found {
                // The card's ruleset sets the mode's values, unless the map
                // plays a mode of its own (a forced Practice Range).
                if found.mode_guid == ruleset.mode {
                    found.ruleset = ruleset.ruleset;
                }
                choices.push((found, ruleset));
            }
        }
    }
    choices
}

/// A map of the card that the server can host, with the ruleset it is played
/// in. If the dashboard forced a map, that one is used with any of the card's
/// rulesets it supports; when the forced map can't be hosted in this card's
/// modes, the card's own random pick is used so a match still starts.
/// `choose(n)` picks one of `n` choices.
pub fn pick_map<'a>(
    catalog: &MapCatalog,
    rules: &'a QueueRules,
    forced: Option<u64>,
    mut choose: impl FnMut(usize) -> usize,
) -> Option<(GameMap, &'a Ruleset)> {
    if let Some(forced) = forced {
        let mut choices = map_choices(catalog, &[(forced, Vec::new())], &rules.rulesets);
        if !choices.is_empty() {
            let index = choose(choices.len());
            return Some(choices.swap_remove(index));
        }
    }
    let mut choices = map_choices(catalog, &rules.maps, &rules.rulesets);
    if choices.is_empty() {
        return None;
    }
    let index = choose(choices.len());
    Some(choices.swap_remove(index))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lucy_content::maps::{GameModes, PRACTICE_RANGE_MAP};

    fn slots(tank: usize, damage: usize, support: usize) -> BTreeMap<u8, usize> {
        BTreeMap::from([
            (TANK_ROLE, tank),
            (DAMAGE_ROLE, damage),
            (SUPPORT_ROLE, support),
        ])
    }

    fn ruleset(sizes: &[u8]) -> Ruleset {
        Ruleset {
            team_sizes: sizes.to_vec(),
            ..Ruleset::default()
        }
    }

    #[test]
    fn role_matching_moves_a_flexible_player_aside() {
        let slots = BTreeMap::from([(TANK_ROLE, 1), (DAMAGE_ROLE, 1)]);
        let players = [vec![TANK_ROLE, DAMAGE_ROLE], vec![TANK_ROLE]];
        assert_eq!(
            match_roles(&players, &slots),
            Some(vec![DAMAGE_ROLE, TANK_ROLE])
        );
        assert_eq!(
            match_roles(&[vec![TANK_ROLE], vec![TANK_ROLE]], &slots),
            None
        );
        assert_eq!(match_roles(&[vec![SUPPORT_ROLE]], &slots), None);
    }

    #[test]
    fn a_full_role_queue_needs_two_of_each_role_per_team() {
        let damage = vec![vec![DAMAGE_ROLE]];
        let parties: Vec<&Seekers> = (0..12).map(|_| damage.as_slice()).collect();
        assert_eq!(
            build_roster(&parties, &ruleset(&[6, 6]), &slots(2, 2, 2), 0),
            None
        );

        let tank = vec![vec![TANK_ROLE]];
        let support = vec![vec![SUPPORT_ROLE]];
        let parties: Vec<&Seekers> = [&tank, &tank, &damage, &damage, &support, &support]
            .into_iter()
            .cycle()
            .take(12)
            .map(|party| party.as_slice())
            .collect();
        let roster = build_roster(&parties, &ruleset(&[6, 6]), &slots(2, 2, 2), 0).unwrap();
        for team in &roster.teams {
            let mut roles: Vec<_> = team.iter().map(|pick| pick.role).collect();
            roles.sort_unstable();
            assert_eq!(roles, [1, 1, 2, 2, 3, 3]);
        }
    }

    #[test]
    fn a_party_stays_on_one_team_and_the_oldest_go_first() {
        let three = vec![Vec::new(); 3];
        let one = vec![Vec::new()];
        let parties: Vec<&Seekers> = vec![&one, &three, &one, &one, &one, &three];
        let roster = build_roster(&parties, &ruleset(&[3, 3]), &BTreeMap::new(), 0).unwrap();
        for team in &roster.teams {
            assert_eq!(team.len(), 3);
        }
        let team_of = |party| {
            roster
                .teams
                .iter()
                .position(|team| team.iter().any(|pick| pick.party == party))
        };
        assert_eq!(
            [0, 1, 2, 3, 4, 5].map(team_of),
            [Some(0), Some(1), Some(0), Some(0), None, None]
        );
    }

    #[test]
    fn players_to_start_takes_whoever_waits_and_drops_role_slots() {
        let damage = vec![vec![DAMAGE_ROLE, TANK_ROLE]];
        let parties: Vec<&Seekers> = vec![&damage];
        assert_eq!(
            build_roster(&parties, &ruleset(&[6, 6]), &slots(2, 2, 2), 2),
            None
        );
        let roster = build_roster(&parties, &ruleset(&[6, 6]), &slots(2, 2, 2), 1).unwrap();
        assert_eq!(
            roster.teams,
            [
                vec![Pick {
                    party: 0,
                    member: 0,
                    role: DAMAGE_ROLE
                }],
                vec![]
            ]
        );
        // Without role slots, no role is given.
        let roster = build_roster(&parties, &ruleset(&[6, 6]), &BTreeMap::new(), 1).unwrap();
        assert_eq!(roster.teams[0][0].role, 0);
        // A minimum at or above the mode's size needs full teams.
        let parties: Vec<&Seekers> = vec![&damage; 3];
        let roster = build_roster(&parties, &ruleset(&[1, 1]), &BTreeMap::new(), 2).unwrap();
        assert_eq!(
            roster.teams.iter().map(Vec::len).collect::<Vec<_>>(),
            [1, 1]
        );
    }

    fn sizes(roster: &Roster) -> Vec<usize> {
        roster.teams.iter().map(Vec::len).collect()
    }

    #[test]
    fn flex_players_take_the_role_that_is_missing() {
        let (tank, damage, support) = (
            vec![vec![TANK_ROLE]],
            vec![vec![DAMAGE_ROLE]],
            vec![vec![SUPPORT_ROLE]],
        );
        let flex = vec![ANY_ROLE.to_vec()];
        let mut parties: Vec<&Seekers> = vec![&tank; 2];
        parties.extend([&damage; 4].map(Vec::as_slice));
        parties.extend([&support; 4].map(Vec::as_slice));
        parties.extend([&flex; 2].map(Vec::as_slice));
        let roster = build_roster(&parties, &ruleset(&[6, 6]), &slots(2, 2, 2), 0).unwrap();
        assert_eq!(sizes(&roster), [6, 6]);
    }

    #[test]
    fn uneven_teams_fill_exactly() {
        let (one, two, three, five) = (
            vec![vec![]],
            vec![vec![]; 2],
            vec![vec![]; 3],
            vec![vec![]; 5],
        );
        let parties: Vec<&Seekers> = vec![&two, &one, &three];
        let roster = build_roster(&parties, &ruleset(&[3, 3]), &BTreeMap::new(), 0).unwrap();
        assert_eq!(sizes(&roster), [3, 3]);
        let parties: Vec<&Seekers> = vec![&five, &one];
        let roster = build_roster(&parties, &ruleset(&[1, 5]), &BTreeMap::new(), 0).unwrap();
        assert_eq!(sizes(&roster), [1, 5]);
        let parties: Vec<&Seekers> = vec![&five, &three, &two, &one];
        assert_eq!(
            build_roster(&parties[..2], &ruleset(&[6, 6]), &BTreeMap::new(), 0),
            None
        );
    }

    #[test]
    fn players_to_start_takes_everyone_who_fits_and_keeps_the_picked_roles() {
        let (one, two) = (vec![vec![]], vec![vec![]; 2]);
        let parties: Vec<&Seekers> = vec![&one, &one, &two, &one];
        let roster = build_roster(&parties, &ruleset(&[6, 6]), &BTreeMap::new(), 2).unwrap();
        assert_eq!(sizes(&roster), [3, 2]);
        let (tank, damage_or_support) =
            (vec![vec![TANK_ROLE]], vec![vec![DAMAGE_ROLE, SUPPORT_ROLE]]);
        let parties: Vec<&Seekers> = vec![&tank, &damage_or_support];
        let roster = build_roster(&parties, &ruleset(&[6, 6]), &slots(2, 2, 2), 2).unwrap();
        let mut roles: Vec<u8> = roster
            .teams
            .iter()
            .flatten()
            .map(|pick| pick.role)
            .collect();
        roles.sort_unstable();
        assert_eq!(roles, [DAMAGE_ROLE, TANK_ROLE]);
        // A free-for-all never waits for more than it holds.
        let ffa = Ruleset {
            team_sizes: vec![8],
            free_for_all: true,
            ..Ruleset::default()
        };
        let parties: Vec<&Seekers> = vec![&one; 8];
        let roster = build_roster(&parties, &ffa, &BTreeMap::new(), 12).unwrap();
        assert_eq!(sizes(&roster), [1; 8]);
        assert_eq!(build_roster(&parties[..7], &ffa, &BTreeMap::new(), 0), None);
    }

    #[test]
    #[ignore = "requires an authorized 104319 runtime in LUCY_RUNTIME_DIR"]
    fn quick_play_rules_and_maps_from_the_runtime() {
        const ILIOS: u64 = 0x0800_0000_0000_066d;
        const CHATEAU_GUILLARD: u64 = 0x0800_0000_0000_07a4;
        let data = lucy_testkit::runtime_dir().join("data");
        let rules = load_rules(&data.join("game_queues_174.json")).unwrap();
        let quick = &rules[&QUICK_PLAY];
        assert!(
            quick
                .rulesets
                .iter()
                .all(|ruleset| ruleset.team_sizes == [6, 6])
        );
        assert_eq!(quick.roles, slots(2, 2, 2));
        let catalog = MapCatalog::load(&data).unwrap();
        let (map, ruleset) = pick_map(&catalog, quick, None, |_| 0).unwrap();
        assert_eq!(map.mode_guid, ruleset.mode);
        let (map, _) = pick_map(&catalog, quick, Some(ILIOS), |_| 0).unwrap();
        assert_eq!((map.map_guid, map.mode_name.as_str()), (ILIOS, "Control"));
        // Not in any Quick Play mode: the queue keeps its pick.
        let (map, _) = pick_map(&catalog, quick, Some(CHATEAU_GUILLARD), |_| 0).unwrap();
        assert_ne!(map.map_guid, CHATEAU_GUILLARD);
        let (map, _) = pick_map(&catalog, quick, Some(PRACTICE_RANGE_MAP), |_| 0).unwrap();
        assert_eq!(map, catalog.practice_range().unwrap());
    }

    #[test]
    fn free_for_all_puts_everyone_on_a_team_of_their_own() {
        let pair = vec![Vec::new(); 2];
        let one = vec![Vec::new()];
        let ffa = Ruleset {
            team_sizes: vec![3],
            free_for_all: true,
            ..Ruleset::default()
        };
        let parties: Vec<&Seekers> = vec![&pair, &one, &one];
        let roster = build_roster(&parties, &ffa, &BTreeMap::new(), 0).unwrap();
        assert_eq!(
            roster.teams.iter().map(Vec::len).collect::<Vec<_>>(),
            [1, 1, 1]
        );
        assert_eq!(roster.teams[2][0].party, 1);
    }

    #[test]
    fn an_open_match_takes_a_party_where_its_roles_are_free() {
        let room = OpenMatch {
            waiting: true,
            free: vec![1, 2],
            roles: vec![BTreeMap::new(), BTreeMap::from([(SUPPORT_ROLE, 2)])],
        };
        let support = [vec![SUPPORT_ROLE]];
        assert_eq!(
            place(&support, &room, &slots(2, 2, 2), true),
            Some((0, vec![SUPPORT_ROLE]))
        );
        let pair = [vec![TANK_ROLE], vec![DAMAGE_ROLE]];
        assert_eq!(
            place(&pair, &room, &slots(2, 2, 2), true),
            Some((1, vec![TANK_ROLE, DAMAGE_ROLE]))
        );
        assert_eq!(
            place(&[vec![], vec![], vec![]], &room, &BTreeMap::new(), false),
            None
        );
        assert_eq!(
            place(&[vec![]], &room, &BTreeMap::new(), true),
            Some((1, vec![TANK_ROLE]))
        );
    }

    const QUEUES: &str = r#"{"cards": {
        "0x06300000000000ED": {"name": "Quick Play", "competitive": false,
            "rulesets": [{"mode": "0x0230000000000014", "mode_name": "Assault",
                "team_sizes": {"blue": 6, "red": 6}}],
            "roles": {"0x0D80000000000850": 2, "0x0D8000000000084E": 2, "0x0D80000000000851": 2},
            "maps": [{"map": "0x0800000000000080", "modes": ["0x0230000000000014"]}]},
        "0x0630000000000002": {"name": "Deathmatch", "rulesets": [{"mode": "0x0230000000000020",
            "free_for_all": true, "players": 8}], "maps": []},
        "0x0630000000000003": {"name": "Empty", "rulesets": []}
    }}"#;

    #[test]
    fn rules_read_teams_role_slots_and_maps() {
        let rules = parse_rules(QUEUES).unwrap();
        assert_eq!(rules.len(), 2);
        let quick = &rules[&QUICK_PLAY];
        assert_eq!(quick.name, "Quick Play");
        assert_eq!(quick.roles, slots(2, 2, 2));
        assert_eq!(quick.rulesets[0].team_sizes, [6, 6]);
        assert_eq!(
            quick.maps,
            [(0x0800_0000_0000_0080, vec![0x0230_0000_0000_0014])]
        );
        let deathmatch = &rules[&0x0630_0000_0000_0002];
        assert!(deathmatch.rulesets[0].free_for_all);
        assert_eq!(deathmatch.rulesets[0].team_sizes, [8]);
    }

    /// A Practice vs AI card fills its red team with AI heroes: the
    /// matchmaker places players on the blue team only, and starts the match
    /// with whoever searches.
    #[test]
    fn a_versus_ai_card_places_its_players_on_the_human_team_only() {
        let rules = parse_rules(
            r#"{"cards": {"0x0630000000000070": {"name": "Play vs AI (AI level 1)",
                "rulesets": [{"mode": "0x0230000000000014", "mode_name": "Assault",
                    "team_sizes": {"blue": 6, "red": 6}}],
                "ai_teams": {"red": 1}, "maps": []}}}"#,
        )
        .unwrap();
        let versus = &rules[&0x0630_0000_0000_0070];
        assert!(versus.versus_ai());
        assert_eq!(versus.ai_teams, BTreeMap::from([(1, 1)]));
        let humans = versus.human_ruleset(&versus.rulesets[0]);
        assert_eq!(humans.team_sizes, [6, 0]);
        let (two, one) = (vec![Vec::new(); 2], vec![Vec::new()]);
        let parties: Vec<&Seekers> = vec![&two, &one];
        let roster = build_roster(&parties, &humans, &BTreeMap::new(), 1).unwrap();
        assert_eq!(roster.teams[0].len(), 3);
        assert!(roster.teams[1].is_empty());
        assert!(!parse_rules(QUEUES).unwrap()[&QUICK_PLAY].versus_ai());
    }

    #[test]
    fn a_forced_map_plays_in_any_mode_and_falls_back_to_the_cards_pick() {
        // The catalog has only the Practice Range.
        let catalog = MapCatalog::from_json(
            r#"{"0x0800000000000688": {"name": "Practice Range",
                "modes": ["0x023000000000005A"],
                "placeables_by_mode": {"0x023000000000005A": [136]},
                "spawns": [{"team": 0, "position": [0, 0, 0], "rotation": [0, 0, 0, 1]}]}}"#,
        )
        .unwrap()
        .with_modes(
            GameModes::from_json(
                r#"{"modes": {"0x023000000000005A": {"teams": [
                    {"team": 0, "controller": null, "players": 6}
                ], "scripted_loading": false}}}"#,
            )
            .unwrap(),
        );
        let rules = parse_rules(QUEUES).unwrap();
        let quick = &rules[&QUICK_PLAY];
        // The catalog knows no map of the card, so only a forced Practice Range loads.
        assert!(pick_map(&catalog, quick, None, |_| 0).is_none());
        assert!(pick_map(&catalog, quick, Some(0x0800_0000_0000_0080), |_| 0).is_none());
        let (map, ruleset) = pick_map(&catalog, quick, Some(PRACTICE_RANGE_MAP), |_| 0).unwrap();
        assert_eq!(map, catalog.practice_range().unwrap());
        assert_eq!(ruleset.mode_name, "Assault");
    }

    #[test]
    fn mode_settings_survive_a_save() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("matchmaking.json");
        assert!(load_modes(&path).is_empty());
        let modes = HashMap::from([
            (
                QUICK_PLAY,
                ModeSettings {
                    players_to_start: 1,
                    fill_running: true,
                },
            ),
            (
                0x0630_0000_0000_0002,
                ModeSettings {
                    players_to_start: 4,
                    fill_running: false,
                },
            ),
        ]);
        save_modes(&path, &modes).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("{\n  \"0x630000000000002\": {\n    \"players_to_start\": 4,"));
        assert_eq!(load_modes(&path), modes);
        save_modes(&path, &HashMap::new()).unwrap();
        assert!(load_modes(&path).is_empty());
        fs::write(&path, "{").unwrap();
        assert!(load_modes(&path).is_empty());
    }
}
