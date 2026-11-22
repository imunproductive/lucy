//! The host's live view and controls (`refs/reports/Dashboard.md` §7):
//! `GET /api/live`, a match's chat and General's (`/api/matches/<id>/chat`,
//! `/api/lobby/chat`), and the actions: a system line to a match or a
//! team, a message to everyone, a kick, a flow control and a stop.
//!
//! The page polls `/api/live?rev=N` about once a second: the reply is the
//! whole view with its revision, or only `{rev}` when nothing changed since
//! revision N. Polling, not server-sent events: the dashboard's server
//! gives each response [`super::http`]'s 30 s deadline and serves the
//! requests one at a time, so a held stream would need a path of its own
//! around both, for no gain at this rate.

use std::{
    collections::{BTreeMap, HashMap},
    sync::{Mutex, PoisonError},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{Map, Value as Json, json};

use super::api::{ApiError, ApiResult};
use crate::{
    game_service::{Driven, FlowAction, FlowRefusal, MatchId, MatchSnapshot},
    lobby::{
        Lobby,
        chat_log::{ChatLine, LineChannel, LineKind},
        host::{self, Audience, Scope},
    },
};
use lucy_sim::match_flow::Phase;

/// The live view's revisions: the last view, its number, and when each
/// match started (fixed once seen, so the view doesn't change with the
/// clock's rounding).
#[derive(Default)]
pub(super) struct Revisions {
    state: Mutex<RevisionState>,
}

#[derive(Default)]
struct RevisionState {
    rev: u64,
    last: Option<Json>,
    started: HashMap<MatchId, u64>,
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_secs())
}

fn lobby_of(lobby: Option<&Lobby>) -> Result<&Lobby, ApiError> {
    lobby.ok_or_else(|| ApiError::new(503, "The lobby is not running."))
}

fn matches_of(lobby: &Lobby) -> Result<Vec<MatchSnapshot>, ApiError> {
    match lobby.game.as_deref() {
        None => Ok(Vec::new()),
        Some(game) => game.snapshot().map_err(|error| {
            ApiError::new(503, format!("The game server did not answer: {error}"))
        }),
    }
}

pub(super) fn match_id_text(id: MatchId) -> String {
    format!("{:016x}{:016x}", id[0], id[1])
}

fn parse_match_id(text: &str) -> Option<MatchId> {
    if text.len() != 32 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some([
        u64::from_str_radix(&text[..16], 16).ok()?,
        u64::from_str_radix(&text[16..], 16).ok()?,
    ])
}

fn phase_name(phase: Phase) -> &'static str {
    match phase {
        Phase::Waiting => "waiting",
        Phase::Assemble => "assemble",
        Phase::Setup => "setup",
        Phase::Round => "round",
        Phase::RoundEnd => "round_end",
        Phase::MatchEnd => "match_end",
    }
}

/// `GET /api/live?rev=N`.
pub(super) fn live(
    lobby: Option<&Lobby>,
    revisions: &Revisions,
    query: &BTreeMap<String, String>,
) -> ApiResult {
    let lobby = lobby_of(lobby)?;
    let matches = matches_of(lobby)?;
    let online: Vec<Json> = {
        let state = lobby.social.lock();
        let online: Vec<u32> = state.sessions().collect();
        online
            .into_iter()
            .filter_map(|account| {
                let name = lobby.accounts.by_id(account).ok().flatten()?.name.clone();
                Some(json!({
                    "account": account.to_string(),
                    "name": name,
                    "in_match": state.match_chat(account).is_some(),
                }))
            })
            .collect()
    };
    let mut revision = revisions
        .state
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let now = unix_now();
    revision
        .started
        .retain(|id, _| matches.iter().any(|found| found.id == *id));
    let heroes = lobby.assets.heroes();
    let matches: Vec<Json> = matches
        .iter()
        .map(|found| {
            let started_at = *revision
                .started
                .entry(found.id)
                .or_insert_with(|| now.saturating_sub(found.age.as_secs()));
            let flow = &found.flow;
            json!({
                "id": match_id_text(found.id),
                "map": found.map,
                "mode": found.mode,
                "mode_graph": flow.mode_graph.map(|graph| format!("{graph:04X}")),
                "driven": match flow.driven {
                    Driven::Graph => "graph",
                    Driven::Flow => "flow",
                    Driven::None => "none",
                },
                "phase": flow.phase.map(phase_name),
                "clock": (flow.clock * 10.0).round() / 10.0,
                "paused": flow.paused,
                "round": flow.round,
                "attackers": flow.attackers,
                "objective": flow.objective,
                "overtime": flow.overtime,
                "winner": flow.winner,
                "completion_off": flow.completion_off,
                "workshop_rules": found.workshop_rules,
                "started_at": started_at,
                "ended": flow.ended,
                "players": found.players.iter().map(|player| json!({
                    "account": if player.ai { String::new() } else { player.account.to_string() },
                    "name": player.name,
                    "hero": heroes.get(player.hero).map_or("", |hero| hero.name.as_str()),
                    "team": player.team,
                    "state": player.state.as_str(),
                    "ai": player.ai,
                })).collect::<Vec<_>>(),
                "chat_seq": lobby.chat_log.last(found.id),
                "controls": flow.controls.iter().map(|control| control.name()).collect::<Vec<_>>(),
            })
        })
        .collect();
    let view = json!({"lobby": {"online": online}, "matches": matches});
    if revision.last.as_ref() != Some(&view) {
        revision.rev += 1;
        revision.last = Some(view.clone());
    }
    let rev = revision.rev;
    if query.get("rev").and_then(|text| text.parse::<u64>().ok()) == Some(rev) {
        return Ok(json!({"rev": rev}));
    }
    let mut reply = view;
    reply["rev"] = json!(rev);
    Ok(reply)
}

fn line_json(line: &ChatLine) -> Json {
    json!({
        "seq": line.seq,
        "at": line.at,
        "kind": match line.kind {
            LineKind::Chat => "chat",
            LineKind::System => "system",
        },
        "channel": match line.channel {
            LineChannel::Match => "match",
            LineChannel::Team(0) => "team1",
            LineChannel::Team(_) => "team2",
            LineChannel::General => "general",
        },
        "from": line.from.as_ref().map(|(account, name)| json!({
            "account": account.to_string(),
            "name": name,
        })),
        "text": line.text,
    })
}

fn page_json((lines, next): (Vec<ChatLine>, u64)) -> Json {
    json!({"lines": lines.iter().map(line_json).collect::<Vec<_>>(), "next": next})
}

fn after(query: &BTreeMap<String, String>) -> Result<u64, ApiError> {
    query.get("after").map_or(Ok(0), |text| {
        text.parse()
            .map_err(|_| ApiError::new(400, "after must be a whole number"))
    })
}

/// `GET /api/lobby/chat?after=SEQ`.
pub(super) fn lobby_chat(lobby: Option<&Lobby>, query: &BTreeMap<String, String>) -> ApiResult {
    let lobby = lobby_of(lobby)?;
    let after = after(query)?;
    let state = lobby.social.lock();
    lobby.chat_log.prune(&state, lobby.clock.now());
    drop(state);
    Ok(page_json(lobby.chat_log.general_page(after)))
}

/// A match route: `/api/matches/<id>/<action>`.
pub(super) fn match_route(path: &str) -> Option<(MatchId, &str)> {
    let (id, action) = path.strip_prefix("/api/matches/")?.split_once('/')?;
    Some((parse_match_id(id)?, action))
}

fn no_match() -> ApiError {
    ApiError::new(404, "No such match is running.")
}

fn running(lobby: &Lobby, id: MatchId) -> Result<MatchSnapshot, ApiError> {
    matches_of(lobby)?
        .into_iter()
        .find(|found| found.id == id)
        .ok_or_else(no_match)
}

/// `GET /api/matches/<id>/chat?after=SEQ`: its lines while it runs and for
/// half an hour after (`chat_log.rs`).
pub(super) fn match_chat(
    lobby: Option<&Lobby>,
    id: MatchId,
    query: &BTreeMap<String, String>,
) -> ApiResult {
    let lobby = lobby_of(lobby)?;
    let after = after(query)?;
    let state = lobby.social.lock();
    lobby.chat_log.prune(&state, lobby.clock.now());
    drop(state);
    match lobby.chat_log.match_page(id, after) {
        Some(page) => Ok(page_json(page)),
        None => {
            running(lobby, id)?;
            Ok(page_json((Vec::new(), 0)))
        }
    }
}

fn text_of(data: &Map<String, Json>) -> Result<String, ApiError> {
    data.get("text")
        .and_then(Json::as_str)
        .and_then(host::one_line)
        .ok_or_else(|| ApiError::new(400, "Write a message first."))
}

fn team_of(value: Option<&Json>) -> Result<u8, ApiError> {
    match value.and_then(Json::as_u64) {
        Some(team @ (0 | 1)) => Ok(team as u8),
        _ => Err(ApiError::new(400, "team must be 0 or 1")),
    }
}

fn account_of(data: &Map<String, Json>) -> Result<u32, ApiError> {
    data.get("account")
        .and_then(|value| match value {
            Json::String(text) => text.parse().ok(),
            Json::Number(number) => number.as_u64().and_then(|value| u32::try_from(value).ok()),
            _ => None,
        })
        .ok_or_else(|| ApiError::new(400, "account must be a player's account id"))
}

fn sent(told: usize) -> ApiResult {
    Ok(json!({"status": "ok", "told": told}))
}

/// `POST /api/matches/<id>/chat {text, channel?}`: a system line to the
/// match's chat, or to one team's (`team1`, `team2`).
pub(super) fn say(lobby: Option<&Lobby>, id: MatchId, data: &Map<String, Json>) -> ApiResult {
    let lobby = lobby_of(lobby)?;
    let text = text_of(data)?;
    let audience = match data.get("channel").and_then(Json::as_str) {
        None | Some("match") => Audience::Match,
        Some("team1") => Audience::Team(0),
        Some("team2") => Audience::Team(1),
        Some(_) => return Err(ApiError::new(400, "channel must be match, team1 or team2")),
    };
    running(lobby, id)?;
    sent(host::say(lobby, id, audience, &text, None)?)
}

/// `POST /api/broadcast {text, scope?}`: a system line to everyone online
/// (`all`), the menu (`lobby`) or the matches (`matches`).
pub(super) fn broadcast(lobby: Option<&Lobby>, data: &Map<String, Json>) -> ApiResult {
    let lobby = lobby_of(lobby)?;
    let text = text_of(data)?;
    let scope = match data.get("scope").and_then(Json::as_str) {
        None | Some("all") => Scope::All,
        Some("lobby") => Scope::Lobby,
        Some("matches") => Scope::Matches,
        Some(_) => return Err(ApiError::new(400, "scope must be all, lobby or matches")),
    };
    sent(host::broadcast(lobby, &text, scope)?)
}

/// `POST /api/matches/<id>/kick {account, reason?}`.
pub(super) fn kick(lobby: Option<&Lobby>, id: MatchId, data: &Map<String, Json>) -> ApiResult {
    let lobby = lobby_of(lobby)?;
    let account = account_of(data)?;
    let reason = data
        .get("reason")
        .and_then(Json::as_str)
        .and_then(host::one_line);
    let found = running(lobby, id)?;
    let Some(player) = found
        .players
        .iter()
        .find(|player| !player.ai && player.account == account)
    else {
        return Err(ApiError::new(404, "That player is not in the match."));
    };
    if !host::kick(lobby, id, account, &player.name, reason.as_deref())? {
        return Err(ApiError::new(409, "That player has left the match."));
    }
    Ok(json!({"status": "ok", "message": format!("{} goes back to the menu.", player.name)}))
}

/// The flow action a request names.
fn flow_action(found: &MatchSnapshot, data: &Map<String, Json>) -> Result<FlowAction, ApiError> {
    let action = data.get("action").and_then(Json::as_str).unwrap_or("");
    let winner = || match data.get("account") {
        Some(_) => {
            let account = account_of(data)?;
            found
                .players
                .iter()
                .find(|player| !player.ai && player.account == account)
                .map(|player| player.team)
                .ok_or_else(|| ApiError::new(404, "That player is not in the match."))
        }
        None => team_of(data.get("team")),
    };
    Ok(match action {
        "set_time" => {
            let seconds = data
                .get("seconds")
                .and_then(Json::as_f64)
                .filter(|seconds| (0.0..=3599.0).contains(seconds))
                .ok_or_else(|| ApiError::new(400, "seconds must be 0 to 3599"))?;
            FlowAction::SetTime(seconds)
        }
        "skip_phase" => FlowAction::SkipPhase,
        "pause" => FlowAction::Pause,
        "unpause" => FlowAction::Unpause,
        "declare_winner" => FlowAction::DeclareWinner(winner()?),
        "draw" => FlowAction::Draw,
        "round_winner" => FlowAction::RoundWinner(team_of(data.get("team"))?),
        "round_draw" => FlowAction::RoundDraw,
        "assemble" => FlowAction::Assemble,
        "completion" => FlowAction::Completion(
            data.get("on")
                .and_then(Json::as_bool)
                .ok_or_else(|| ApiError::new(400, "on must be true or false"))?,
        ),
        _ => {
            return Err(ApiError::new(
                400,
                format!("Unknown flow action '{action}'."),
            ));
        }
    })
}

fn run_flow(lobby: &Lobby, found: &MatchSnapshot, action: FlowAction) -> ApiResult {
    let game = lobby
        .game
        .as_deref()
        .ok_or_else(|| ApiError::new(409, "The game server is off."))?;
    match game.run_flow(found.id, action)? {
        Ok(()) => Ok(json!({"status": "ok"})),
        Err(FlowRefusal::NoMatch) => Err(no_match()),
        Err(FlowRefusal::NotAllowed) => Err(ApiError::new(
            409,
            format!(
                "The match doesn't allow {} now.",
                action.control().name().replace('_', " ")
            ),
        )),
        Err(FlowRefusal::Failed(reason)) => Err(ApiError::new(500, reason)),
    }
}

/// `POST /api/matches/<id>/flow {action, …}`: 409 when its `controls`
/// lack the action.
pub(super) fn flow(lobby: Option<&Lobby>, id: MatchId, data: &Map<String, Json>) -> ApiResult {
    let lobby = lobby_of(lobby)?;
    let found = running(lobby, id)?;
    let action = flow_action(&found, data)?;
    run_flow(lobby, &found, action)
}

/// `POST /api/matches/<id>/stop {result, team?}`: a declared winner or
/// draw ends the match as its mode does (the result cards, the results,
/// then home); `none` sends every game home now.
pub(super) fn stop(lobby: Option<&Lobby>, id: MatchId, data: &Map<String, Json>) -> ApiResult {
    let lobby = lobby_of(lobby)?;
    let found = running(lobby, id)?;
    match data.get("result").and_then(Json::as_str) {
        Some("winner") => run_flow(
            lobby,
            &found,
            FlowAction::DeclareWinner(team_of(data.get("team"))?),
        ),
        Some("draw") => run_flow(lobby, &found, FlowAction::Draw),
        Some("none") => {
            let players = host::stop(lobby, id)?;
            Ok(
                json!({"status": "ok", "message": format!("Sent {players} player(s) back to the menu.")}),
            )
        }
        _ => Err(ApiError::new(400, "result must be winner, draw or none")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_routes_take_a_32_digit_id() {
        let id = [0x0123_4567_89ab_cdef, 0x7edc_ba98_7654_3210];
        let text = match_id_text(id);
        assert_eq!(text, "0123456789abcdef7edcba9876543210");
        assert_eq!(
            match_route(&format!("/api/matches/{text}/kick")),
            Some((id, "kick"))
        );
        assert_eq!(match_route("/api/matches/0123/kick"), None);
        assert_eq!(match_route(&format!("/api/matches/{text}")), None);
    }
}
