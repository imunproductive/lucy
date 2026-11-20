//! Custom games: the lobby a host creates (24000 kind 1), its settings
//! (24008-24013) and Start (24018). The protocol is in
//! `orpheus_lobby::custom_game`; the research in
//! `refs/reports/CustomGames.md`.

use std::{
    collections::BTreeMap,
    io,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use orpheus_content::maps::GameMap;
use orpheus_lobby::{
    accounts::Account,
    custom_game::{self, CustomMatch, CustomSettings, RulesetValue, Upload},
    transport::LobbyMessage,
};
use orpheus_script::graph::ModeValue;
use orpheus_wire::jam::codec::Fields;

use super::{Ctx, Lobby, Matchmaker, Parties, Party, Ticket, rules};

/// The ruleset ids of custom games' matches: `0C0` GUIDs no asset has
/// (the shipped `0C0` indexes are below 0x100), one per match.
const MATCH_RULESETS: u64 = 0x0fd0_8000_0000_0000;
static NEXT_RULESET: AtomicU64 = AtomicU64::new(1);

fn mode_value(value: RulesetValue) -> ModeValue {
    match value {
        RulesetValue::Bool(value) => ModeValue::Bool(value),
        RulesetValue::Integer(value) => ModeValue::Integer(value),
        RulesetValue::Float(value) => ModeValue::Float(f64::from(value)),
        RulesetValue::Asset(value) => ModeValue::Asset(value),
    }
}

/// A host's custom game lobby.
pub(super) struct CustomGame {
    lobby: custom_game::Lobby,
    upload: Upload,
    /// The settings of his last complete upload.
    settings: Option<CustomSettings>,
    /// The host got 23304: later uploads are answered with 23312.
    entered: bool,
}

impl Matchmaker {
    /// 24000 kind 1: the host's new lobby. The client sends its default
    /// settings right after (`0x7FF7895DD340`); the lobby answers when they
    /// are complete. A lobby the host had is replaced.
    pub(crate) fn create_custom_game(
        &self,
        lobby: &Lobby,
        account: &Arc<Account>,
        name: String,
        host_card: Fields,
    ) -> io::Result<()> {
        let party = lobby.party_of(account)?;
        let id = super::match_id()?;
        tracing::info!("[CG] {} creates custom game {name:?}", account.name);
        self.lock().custom.insert(
            account.account_lo,
            CustomGame {
                lobby: custom_game::Lobby {
                    id,
                    name,
                    host_card,
                    host_group: party.id,
                },
                upload: Upload::default(),
                settings: None,
                entered: false,
            },
        );
        Ok(())
    }

    /// 24008-24013: a part of the host's settings upload. Once it is
    /// complete, what the host must get: the lobby itself the first time,
    /// the settings echoed after.
    pub(crate) fn custom_game_settings(
        &self,
        account: &Arc<Account>,
        id: u32,
        fields: Fields,
    ) -> io::Result<Vec<LobbyMessage>> {
        let mut state = self.lock();
        let Some(game) = state.custom.get_mut(&account.account_lo) else {
            tracing::info!(
                "[CG] Settings {id} from {} outside a custom game",
                account.name
            );
            return Ok(Vec::new());
        };
        let Some(settings) = game.upload.push(id, fields)? else {
            return Ok(Vec::new());
        };
        tracing::info!(
            "[CG] {} uploaded settings: maps {:X?}, modes {:X?}, {} chunks, Workshop {}",
            account.name,
            settings.maps(),
            settings.modes(),
            settings.chunks.len(),
            if settings.workshop().is_empty() {
                "empty"
            } else {
                "present"
            }
        );
        tracing::debug!("[CG] settings: {settings:?}");
        let messages = if game.entered {
            settings.update()
        } else {
            game.entered = true;
            game.lobby.enter(&settings)
        };
        game.settings = Some(settings);
        Ok(messages)
    }

    /// 24018: Start (3) puts the host in a match on the settings' first
    /// map that the catalog plays. Other commands are only logged.
    pub(crate) fn custom_game_command(
        &self,
        lobby: &Lobby,
        account: &Arc<Account>,
        command: Option<u64>,
    ) -> io::Result<()> {
        if command != Some(custom_game::START) {
            tracing::info!("[CG] Lobby command {command:?} from {}", account.name);
            return Ok(());
        }
        let party = lobby.party_of(account)?;
        self.with(lobby, |ctx| ctx.start_custom_game(account, &party));
        Ok(())
    }
}

impl Ctx<'_> {
    fn start_custom_game(&mut self, account: &Arc<Account>, party: &Party) {
        let Some(game) = self.state.custom.get(&account.account_lo) else {
            tracing::info!("[CG] Start from {} outside a custom game", account.name);
            return;
        };
        let Some(settings) = game.settings.clone() else {
            tracing::warn!("[CG] Start before the settings came");
            return;
        };
        let Some(mut map) = self.custom_game_map(&settings) else {
            tracing::warn!(
                "[CG] No map of the settings ({:X?}) plays in a known mode",
                settings.maps()
            );
            return;
        };
        let ticket = Ticket {
            party: party.clone(),
            key: Fields::new(),
            card: 0,
            members: Vec::new(),
        };
        let Some(entry) = self.entry(account, &ticket, 0, 0) else {
            return;
        };
        let workshop = match settings.workshop_block(&self.lobby.schemas) {
            Ok(workshop) => workshop,
            Err(error) => {
                tracing::warn!("[CG] The Workshop does not encode: {error}");
                return;
            }
        };
        // The match's own ruleset: the settings' values of its mode and
        // map, read before the schema defaults (`GraphStore::ruleset_value`)
        // and over the mode entity's variables.
        let values: BTreeMap<u32, ModeValue> = settings
            .ruleset_values(map.map_guid, map.mode_guid)
            .into_iter()
            .map(|(key, value)| (key, mode_value(value)))
            .collect();
        map.ruleset = MATCH_RULESETS | NEXT_RULESET.fetch_add(1, Ordering::Relaxed);
        let lobby = self.lobby;
        let graphs = lobby.assets.graphs();
        graphs.add_match_ruleset(map.ruleset, map.mode_guid, values.clone());
        tracing::info!(
            "[CG] {} starts {} ({}), teams {:?}, ruleset values {values:?}",
            account.name,
            map.name,
            map.mode_name,
            map.team_sizes
        );
        let ruleset = map.ruleset;
        if !self.start(map, vec![entry], false, 0, &[], None) {
            graphs.remove_match_ruleset(ruleset);
            return;
        }
        let id = self.state.rooms.last().expect("the started room").id;
        let custom = Arc::new(CustomMatch { settings, workshop });
        if let Some(game) = self.lobby.game.as_deref()
            && let Err(error) = game.attach_custom_game(id, custom)
        {
            tracing::warn!("[CG] The game service did not take the settings: {error}");
        }
    }

    /// The match a custom game plays: the first of its maps that the
    /// catalog plays in a mode the queue rules know, preferring the modes
    /// its settings name; no 0C0 ruleset (the mode's own values); the
    /// team sizes of its settings when they have them, the host's place
    /// kept.
    fn custom_game_map(&self, settings: &CustomSettings) -> Option<GameMap> {
        let preferred = settings.modes();
        let mut rulesets: Vec<_> = self
            .mm
            .rules
            .values()
            .flat_map(|rules| rules.rulesets.iter())
            .collect();
        rulesets.sort_by_key(|ruleset| {
            (
                preferred
                    .iter()
                    .position(|&mode| mode == ruleset.mode)
                    .unwrap_or(usize::MAX),
                ruleset.mode,
                ruleset.ruleset,
            )
        });
        rulesets.dedup_by_key(|ruleset| ruleset.mode);
        for map in settings.maps() {
            let maps = [(map, Vec::new())];
            for ruleset in &rulesets {
                let found =
                    rules::map_choices(&self.mm.maps, &maps, std::slice::from_ref(*ruleset));
                if let Some((mut found, _)) = found.into_iter().next() {
                    found.ruleset = 0;
                    if let Some(sizes) = settings.team_sizes(map, found.mode_guid) {
                        found.team_sizes = sizes;
                    }
                    if found.team_sizes.first().is_none_or(|&size| size == 0) {
                        // The host plays in team 0.
                        found.team_sizes.resize(found.team_sizes.len().max(1), 0);
                        found.team_sizes[0] = 1;
                    }
                    return Some(found);
                }
            }
        }
        None
    }
}
