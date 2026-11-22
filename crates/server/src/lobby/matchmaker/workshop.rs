//! `--workshop`: a Workshop script every player goes into straight from the
//! menu (`docs/dev/doomfist-parkour-plan.md`, no lobby and no Workshop
//! menus). The script's settings pick the mode and map, the team sizes and
//! the match's ruleset values (`crate::workshop_match`); its rules go to
//! the game service for the Workshop engine.

use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use lucy_lobby::{accounts::Account, session::PreparedLogin};

use super::{Ctx, Lobby, Matchmaker, Parties, Party};
use crate::workshop_match::{ScriptMatch, SettingNames};

/// A script file and what its settings describe.
#[derive(Debug)]
pub(crate) struct WorkshopScript {
    pub(crate) path: PathBuf,
    pub(crate) script: ScriptMatch,
}

impl Matchmaker {
    /// The Workshop catalog of the runtime data.
    pub(super) fn catalog(&self) -> io::Result<Arc<lucy_workshop::Catalog>> {
        if let Some(catalog) = self.catalog.get() {
            return Ok(Arc::clone(catalog));
        }
        let catalog = Arc::new(lucy_workshop::Catalog::load(&self.data)?);
        Ok(Arc::clone(self.catalog.get_or_init(|| catalog)))
    }

    /// Read the script at `path` (or none) for every player who reaches the
    /// menu from now on. What its settings say that the match does not
    /// apply is logged.
    pub(crate) fn set_workshop(&self, path: Option<&Path>) -> io::Result<()> {
        let script = match path {
            Some(path) => {
                let text = fs::read_to_string(path).map_err(|error| {
                    io::Error::new(error.kind(), format!("{}: {error}", path.display()))
                })?;
                let names = SettingNames::load(&self.data)?;
                let script =
                    ScriptMatch::parse(&text, &*self.catalog()?, &names).map_err(|error| {
                        io::Error::new(error.kind(), format!("{}: {error}", path.display()))
                    })?;
                tracing::info!(
                    "[WS] {}: {} rules, modes {:X?}, teams {:?}, heroes {:X?}",
                    path.display(),
                    script.logic.rules.len(),
                    script
                        .games
                        .iter()
                        .map(|game| game.mode)
                        .collect::<Vec<_>>(),
                    script.team_sizes,
                    script.heroes
                );
                for skipped in &script.skipped {
                    tracing::info!("[WS] Not applied: {skipped}");
                }
                Some(Arc::new(WorkshopScript {
                    path: path.to_owned(),
                    script,
                }))
            }
            None => None,
        };
        self.lock().workshop = script;
        Ok(())
    }

    /// The script players go into from the menu, if any.
    pub(crate) fn workshop(&self) -> Option<PathBuf> {
        self.lock()
            .workshop
            .as_ref()
            .map(|script| script.path.clone())
    }

    /// A session logged in: with a Workshop script it goes into the match
    /// once its menu is up ([`Matchmaker::workshop_menu_ready`]).
    pub(crate) fn workshop_login(&self, login: &PreparedLogin) {
        let mut state = self.lock();
        if state.workshop.is_some() {
            state
                .workshop_due
                .insert(login.account.account_lo, (login.identity.session, None));
        }
    }

    /// The session's menu has its initial data (the client's 60002): its
    /// Workshop match is due. A match handed off before it can leave the
    /// client on the map without hero select (game check 2026-11-20: with
    /// 60002 0.5 s before the match, hero select; 0.2 s after it, none).
    pub(crate) fn workshop_menu_ready(&self, lobby: &Lobby, login: &PreparedLogin) {
        let mut state = self.lock();
        if let Some((session, due)) = state.workshop_due.get_mut(&login.account.account_lo)
            && *session == login.identity.session
        {
            due.get_or_insert(lobby.clock.now());
        }
    }

    /// The session's Workshop match when it is due: started, or tried
    /// again at its next tick while the game clock refuses matches.
    pub(super) fn workshop_tick(
        &self,
        lobby: &Lobby,
        account: &Arc<Account>,
        session: [u64; 2],
    ) -> io::Result<()> {
        let now = lobby.clock.now();
        let due = {
            let state = self.lock();
            state
                .workshop_due
                .get(&account.account_lo)
                .is_some_and(|&(of, due)| of == session && due.is_some_and(|due| due <= now))
        };
        if !due {
            return Ok(());
        }
        let party = lobby.party_of(account)?;
        self.with(lobby, |ctx| {
            let Some(script) = ctx.state.workshop.clone() else {
                ctx.state.workshop_due.remove(&account.account_lo);
                return;
            };
            if ctx.start_workshop(account, &party, &script) || !ctx.state.clock_blocked {
                ctx.state.workshop_due.remove(&account.account_lo);
            }
        });
        Ok(())
    }
}

impl Ctx<'_> {
    /// The script's match for the player: the first map of its modes that
    /// the server hosts, with its team sizes and its ruleset values.
    fn start_workshop(
        &mut self,
        account: &Arc<Account>,
        party: &Party,
        script: &WorkshopScript,
    ) -> bool {
        let found = script.script.games.iter().find_map(|game| {
            let maps: Vec<u64> = match &game.maps {
                Some(maps) => maps.clone(),
                None => self
                    .mm
                    .maps
                    .map_catalog()
                    .into_iter()
                    .map(|listing| listing.guid)
                    .filter(|map| !game.without.contains(map))
                    .collect(),
            };
            Some((self.custom_map(&maps, &[game.mode], false)?, game))
        });
        let Some((mut map, game)) = found else {
            tracing::warn!(
                "[WS] {}: no map of its modes plays here",
                script.path.display()
            );
            return false;
        };
        map.hero_pool = script.script.heroes.clone().unwrap_or_default();
        for (team, size) in script.script.team_sizes.iter().enumerate() {
            if let Some(size) = *size {
                map.team_sizes.resize(map.team_sizes.len().max(team + 1), 0);
                map.team_sizes[team] = size;
            }
        }
        self.start_custom(
            account,
            party,
            map,
            game.values.clone(),
            script.script.bodies.clone(),
            Arc::clone(&script.script.logic),
        )
    }
}
