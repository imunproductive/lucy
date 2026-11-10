//! The dashboard's game launches (`start_game` and `reconnect` in
//! `ow174/dashboard/service.py`).

use serde_json::{Map, Value as Json, json};

use super::{Api, ApiError, ApiResult};

impl Api<'_> {
    /// Start a second retail game on this PC that plays the account, to test
    /// two players.
    pub fn start_game(&self, data: &Map<String, Json>) -> ApiResult {
        let Some(games) = &self.platform.games else {
            return Err(ApiError::new(
                409,
                "Client launching is unavailable in this server mode",
            ));
        };
        let account = self.account(data.get("name").and_then(Json::as_str))?;
        let online = self
            .online
            .lock()
            .expect("lobby accounts")
            .contains_key(&account.account_lo);
        if online || games.second_accounts().contains(&account.name) {
            return Err(ApiError::new(
                409,
                format!("{} is already playing", account.name),
            ));
        }
        games.start(&account.name);
        Ok(json!({
            "status": "ok",
            "message": format!("A second game is starting as {}.", account.name),
        }))
    }

    /// Drop the game played on this PC so it logs in again as the
    /// dashboard's account.
    pub fn reconnect(&self) -> Json {
        self.platform.reconnect_own_game();
        json!({"status": "ok"})
    }
}
