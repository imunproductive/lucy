//! The dashboard's Bot plate page: the live nameplate tool
//! (`lucy_sim::bot_plate`), which changes the training bots' health bar
//! and nameplate inputs in every match while it runs.

use serde_json::{Map, Value as Json, json};

use super::{Api, ApiError, ApiResult};
use lucy_sim::bot_plate::{ARRAYS, INPUTS, MESSAGES, PlateChange, PlateView, value_text};

impl Api<'_> {
    /// What the tool shows; with `command` (the chat command's text after
    /// `.plate`), the change first.
    pub fn bot_plate(&self, data: Option<&Map<String, Json>>) -> ApiResult {
        let game = self
            .lobby
            .and_then(|lobby| lobby.game.as_deref())
            .ok_or_else(|| ApiError::new(409, "The game server is off."))?;
        let change = data
            .map(|data| {
                let command = data
                    .get("command")
                    .and_then(Json::as_str)
                    .ok_or_else(|| ApiError::new(400, "Send a command."))?;
                PlateChange::parse(command).map_err(|reason| ApiError::new(400, reason))
            })
            .transpose()?;
        let view = game
            .bot_plate(change)
            .map_err(|error| {
                ApiError::new(503, format!("The game server did not answer: {error}"))
            })?
            .map_err(|reason| ApiError::new(400, reason))?;
        Ok(view_json(&view))
    }
}

fn view_json(view: &PlateView) -> Json {
    let bot = view.bot.as_ref().map(|bot| {
        let parts: Vec<Json> = bot
            .parts
            .iter()
            .map(|part| {
                json!({
                    "name": part.name(),
                    "array": ARRAYS[part.field / lucy_wire::entity::POOLS],
                    "id": part.id,
                    "max": part.max,
                    "now": part.now,
                    "kind": part.kind,
                    "bytes": part.bytes,
                    "tuned": part.tuned,
                })
            })
            .collect();
        let variables: Vec<Json> = bot
            .variables
            .iter()
            .map(|(variable, value, tuned)| {
                let what = INPUTS
                    .iter()
                    .find(|input| input.variable == *variable)
                    .map_or("", |input| input.what);
                json!({
                    "name": variable.name(),
                    "value": value.as_ref().map(value_text),
                    "tuned": tuned,
                    "what": what,
                })
            })
            .collect();
        json!({
            "entity": format!("{:08X}", bot.entity),
            "name": bot.name,
            "parts": parts,
            "variables": variables,
        })
    });
    let messages: Vec<Json> = MESSAGES
        .iter()
        .map(|(id, what)| json!({"id": format!("{id:04X}"), "what": what}))
        .collect();
    json!({
        "status": "ok",
        "commands": view.commands,
        "bot": bot,
        "messages": messages,
        "summary": view.summary(),
    })
}
