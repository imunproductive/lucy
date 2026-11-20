//! The dashboard's matchmaking controls: how many searching players start a
//! match, each queue card's own settings, the map every queue loads, and
//! ending the matches. Ports `ow174/dashboard/service.py` `matchmaking`,
//! `mode_settings`, `set_map`, `end_matches` and the matchmaking part of
//! `_server_info`.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value as Json, json};

use super::{Api, ApiError, ApiResult, parse_bool, parse_guid, parse_int};
use crate::lobby::Lobby;
use orpheus_lobby::matchmaker::{QUICK_PLAY, Ruleset};

/// A queue ruleset's teams as the dashboard shows them: "6 v 6", "1 v 5",
/// "FFA 8".
fn teams_text(ruleset: &Ruleset) -> String {
    match ruleset.team_sizes.as_slice() {
        [players] if ruleset.free_for_all => format!("FFA {players}"),
        // The players' one team plays against the AI.
        [players] => format!("{players} v AI"),
        sizes => sizes
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(" v "),
    }
}

fn zero() -> Json {
    Json::from(0)
}

impl Api<'_> {
    fn lobby(&self) -> Result<&Lobby, ApiError> {
        self.lobby
            .ok_or_else(|| ApiError::new(409, "Matchmaking is off."))
    }

    /// How many searching players start a match of a mode without a number of
    /// its own: 0 = full teams only.
    pub fn matchmaking(&self, data: &Map<String, Json>) -> ApiResult {
        let value = data.get("test_players").cloned().unwrap_or_else(zero);
        let players = parse_int(&value, "Players to start", 0, 12)? as usize;
        let lobby = self.lobby()?;
        lobby.matchmaker.set_minimum_players(lobby, players);
        Ok(json!({"message": if players == 0 {
            "Matches start with full teams.".to_owned()
        } else {
            format!("Matches start as soon as {players} player(s) search.")
        }}))
    }

    /// A queue card's own settings: how many searching players start its
    /// match (0 = the number for every mode), and whether players who search
    /// join its matches that are on (unranked only).
    pub fn mode_settings(&self, data: &Map<String, Json>) -> ApiResult {
        let lobby = self.lobby()?;
        let card = match data.get("card") {
            Some(Json::Number(number)) => parse_guid(&Json::Number(number.clone()))?,
            Some(Json::String(text)) => parse_guid(&Json::String(text.trim().to_owned()))?,
            _ => parse_guid(&Json::String(String::new()))?,
        };
        let rules = lobby
            .matchmaker
            .rules(card)
            .ok_or_else(|| ApiError::new(400, "The data does not know that queue."))?;
        let value = data.get("players_to_start").cloned().unwrap_or_else(zero);
        let players = parse_int(&value, "Players to start", 0, 12)? as usize;
        let fill = parse_bool(data.get("fill_running").unwrap_or(&Json::Bool(false)))?;
        if fill && rules.competitive {
            return Err(ApiError::new(
                400,
                "Competitive matches never take players once they are on.",
            ));
        }
        lobby.matchmaker.set_mode(lobby, card, players, fill)?;
        let name = if rules.name.is_empty() {
            format!("0x{card:X}")
        } else {
            rules.name.clone()
        };
        let start = if players == 0 {
            "the number for every mode".to_owned()
        } else {
            format!("{players} player(s)")
        };
        let joining = if fill {
            "; players who search join its matches that are on"
        } else {
            ""
        };
        Ok(json!({"message": format!("{name}: matches start with {start}{joining}.")}))
    }

    /// The map every queue loads, or each queue's own random pick ("random"
    /// or empty). A queue whose modes the map is not played in keeps its own
    /// pick; the Practice Range loads in every queue.
    pub fn set_map(&self, data: &Map<String, Json>) -> ApiResult {
        let lobby = self.lobby()?;
        let value = match data.get("map") {
            Some(Json::String(text)) => Json::String(text.trim().to_owned()),
            Some(Json::Number(number)) => Json::Number(number.clone()),
            _ => Json::String(String::new()),
        };
        if matches!(&value, Json::String(text) if text.is_empty() || text.eq_ignore_ascii_case("random"))
        {
            lobby.matchmaker.set_forced_map(None);
            return Ok(json!({"message": "Matches use their queue's random map."}));
        }
        let guid = parse_guid(&value)?;
        let name = lobby
            .matchmaker
            .map_name(guid)
            .ok_or_else(|| ApiError::new(400, "The data does not know that map."))?
            .to_owned();
        lobby.matchmaker.set_forced_map(Some(guid));
        Ok(json!({"message": format!("Matches load {name} when their queue's modes allow it.")}))
    }

    /// The Workshop script players go into from the menu (`--workshop`):
    /// its file, or none (empty). A player already in the menu goes at his
    /// next login.
    pub fn set_workshop(&self, data: &Map<String, Json>) -> ApiResult {
        let lobby = self.lobby()?;
        let path = data
            .get("path")
            .and_then(Json::as_str)
            .map(|path| path.trim().trim_matches('"'))
            .filter(|path| !path.is_empty());
        lobby
            .matchmaker
            .set_workshop(path.map(std::path::Path::new))
            .map_err(|error| ApiError::new(400, error.to_string()))?;
        Ok(json!({"message": match path {
            Some(path) => format!("Players go into the match of {path} from the menu."),
            None => "No Workshop script: players stay in the menu.".to_owned(),
        }}))
    }

    pub fn end_matches(&self) -> ApiResult {
        let lobby = self
            .lobby
            .filter(|lobby| lobby.game.is_some())
            .ok_or_else(|| ApiError::new(409, "The game server is off."))?;
        let told = lobby.matchmaker.send_home(lobby, None);
        Ok(json!({"message": format!("Sent {told} game(s) back to the menu.")}))
    }

    /// The queue cards the menu offers now that the server can host (Quick
    /// Play, the season's competitive cards, the Arcade's) and the cards with
    /// settings of their own, with their teams and matchmaking settings.
    pub(super) fn mode_rows(&self) -> Vec<Json> {
        let Some(lobby) = self.lobby else {
            return Vec::new();
        };
        let matchmaker = &lobby.matchmaker;
        let modes = matchmaker.modes();
        let mut cards = {
            let profile = self.selected().profile().clone();
            let season = lobby.ranked.season(&profile);
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |now| now.as_secs());
            let mut cards = vec![QUICK_PLAY, season.card];
            cards.extend(season.open_card);
            cards.extend(lobby.arcade.cards(&profile, now));
            cards
        };
        let mut own: Vec<u64> = modes.keys().copied().collect();
        own.sort_unstable();
        cards.extend(own);
        let mut seen = Vec::new();
        let mut rows = Vec::new();
        for card in cards {
            if seen.contains(&card) {
                continue;
            }
            seen.push(card);
            let Some(rules) = matchmaker.rules(card) else {
                continue;
            };
            if card == 0 || !(modes.contains_key(&card) || matchmaker.can_host(card)) {
                continue;
            }
            let mut teams: Vec<String> = Vec::new();
            for text in rules.rulesets.iter().map(teams_text) {
                if !teams.contains(&text) {
                    teams.push(text);
                }
            }
            let settings = modes.get(&card).copied().unwrap_or_default();
            rows.push(json!({
                "card": format!("0x{card:X}"),
                "name": if rules.name.is_empty() { format!("0x{card:X}") } else { rules.name.clone() },
                "teams": teams.join(" / "),
                "competitive": rules.competitive,
                "players_to_start": settings.players_to_start,
                "fill_running": settings.fill_running,
            }));
        }
        rows
    }

    /// Every map the data knows, plus the Practice Range, for the map picker.
    pub(super) fn map_rows(&self) -> Vec<Json> {
        self.lobby.map_or_else(Vec::new, |lobby| {
            lobby
                .matchmaker
                .map_catalog()
                .into_iter()
                .map(|map| json!({"guid": format!("0x{:X}", map.guid), "name": map.name}))
                .collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teams_read_as_the_dashboard_shows_them() {
        let ruleset = |sizes: &[u8], free_for_all| Ruleset {
            team_sizes: sizes.to_vec(),
            free_for_all,
            ..Ruleset::default()
        };
        assert_eq!(teams_text(&ruleset(&[6, 6], false)), "6 v 6");
        assert_eq!(teams_text(&ruleset(&[1, 5], false)), "1 v 5");
        assert_eq!(teams_text(&ruleset(&[8], true)), "FFA 8");
        assert_eq!(teams_text(&ruleset(&[4], false)), "4 v AI");
    }
}
