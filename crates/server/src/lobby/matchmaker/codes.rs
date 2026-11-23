//! Settings codes and saved settings: Share Code (40802 kind 2 → 40913),
//! Import Code (40809 → 40901 kind 2 or 40912) and the presets
//! (`lucy_lobby::custom_game::links`). A code is the settings a player
//! shared, kept in the state folder, or a Workshop script the server has
//! as a file (`<CODE>.txt` or `<CODE>_<name>.txt` in `workshop_codes/` of
//! the state folder or a `--workshop-codes` folder, as the parkour
//! archive names its scripts): its settings are built from its text.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use lucy_lobby::{
    accounts::Account,
    custom_game::{
        Chunk, ChunkKind, CustomSettings, Upload,
        links::{
            self, LINK_UPLOAD, Saved, SavedRequest, ShareStatus, link, link_failed, normalise,
            shared,
        },
    },
    transport::LobbyMessage,
};
use lucy_script::graph::ModeValue;
use lucy_wire::jam::codec::{
    Fields,
    Value::{Array, Bool, Struct, Unsigned},
};

use super::Matchmaker;
use crate::workshop_match::{ScriptMatch, SettingNames};

/// The state folder's folder of Workshop scripts by code.
pub(super) const CODE_DIR: &str = "workshop_codes";

/// The `075` hero GUID's index, as a team's lists keep heroes.
fn hero_index(hero: u64) -> u64 {
    hero & 0xffff
}

/// A ruleset value as the settings' `{u32 value, u16 key, u8 kind}` pair
/// (`CustomGames.md` §2: kind 1 bool, 2 int, 3 float bits, 4 a `01C`).
fn pair(key: u32, value: ModeValue) -> lucy_wire::jam::codec::Value {
    let (value, kind) = match value {
        ModeValue::Bool(value) => (u32::from(value), 1),
        ModeValue::Integer(value) => (value as i32 as u32, 2),
        ModeValue::Float(value) => ((value as f32).to_bits(), 3),
        ModeValue::Asset(value) => (value as u32, 4),
    };
    Struct(Fields::from([
        (0x0, Unsigned(u64::from(value))),
        (0x4, Unsigned(u64::from(key))),
        (0x6, Unsigned(kind)),
    ]))
}

fn pairs(values: &BTreeMap<u32, ModeValue>) -> lucy_wire::jam::codec::Value {
    Array(
        values
            .iter()
            .map(|(&key, &value)| pair(key, value))
            .collect(),
    )
}

/// The default team size of a custom game's settings.
const TEAM_SIZE: u8 = 6;

/// A script's teams (`CGGameRulesetTeam`): Team 1 and Team 2 with their
/// sizes, heroes and values, and team 5 (every team) with General's.
fn teams(script: &ScriptMatch) -> lucy_wire::jam::codec::Value {
    let team = |number: u64, size: u8, heroes: Option<&Vec<u64>>, scope: Option<u8>| {
        let of_scope = |body: &&BodyValues| body.team == scope;
        let all = script
            .bodies
            .iter()
            .filter(of_scope)
            .filter(|body| body.hero.is_none())
            .fold(BTreeMap::new(), |mut values, body| {
                values.extend(&body.values);
                values
            });
        let per_hero: Vec<_> = script
            .bodies
            .iter()
            .filter(of_scope)
            .filter_map(|body| {
                Some(Struct(Fields::from([
                    (0x0, pairs(&body.values)),
                    (0x18, Unsigned(hero_index(body.hero?))),
                ])))
            })
            .collect();
        let mut fields = Fields::from([
            (0x0, Array(per_hero)),
            (0x18, pairs(&all)),
            (0x60, Unsigned(number)),
            (0x61, Unsigned(u64::from(size))),
        ]);
        if let Some(heroes) = heroes {
            fields.insert(
                0x48,
                Array(
                    heroes
                        .iter()
                        .map(|&hero| Unsigned(hero_index(hero)))
                        .collect(),
                ),
            );
            fields.insert(0x62, Bool(true));
        }
        Struct(fields)
    };
    Array(vec![
        team(
            0,
            script.team_sizes[0].unwrap_or(TEAM_SIZE),
            script.team_heroes[0].as_ref(),
            Some(0),
        ),
        team(
            1,
            script.team_sizes[1].unwrap_or(TEAM_SIZE),
            script.team_heroes[1].as_ref(),
            Some(1),
        ),
        team(5, 0, script.heroes.as_ref(), None),
    ])
}

/// A Workshop script as the settings a client takes: a mode block per
/// game (its values and the teams), a map entry per map it plays
/// (`maps_of` gives a mode's maps when the script names none), the rules
/// as the client uploads them (`lucy_workshop::binary::encode`).
pub(crate) fn script_settings(
    script: &ScriptMatch,
    catalog: &lucy_workshop::Catalog,
    maps_of: impl Fn(u64) -> Vec<u64>,
) -> CustomSettings {
    let teams = teams(script);
    let (workshop, items) = lucy_workshop::binary::encode(&script.logic, catalog);
    let block = |mode: u64, values: Option<&BTreeMap<u32, ModeValue>>| {
        let mut block = Fields::from([
            (0xb0, Unsigned(mode)),
            (0xc0, teams.clone()),
            (0x108, Bool(true)),
        ]);
        if let Some(values) = values {
            block.insert(0xd8, pairs(values));
        }
        block
    };
    let mut chunks = Vec::new();
    for game in &script.games {
        let maps = match &game.maps {
            Some(maps) => maps.clone(),
            None => maps_of(game.mode)
                .into_iter()
                .filter(|map| !game.without.contains(map))
                .collect(),
        };
        for map in maps {
            chunks.push(Chunk {
                kind: ChunkKind::Map,
                fields: Fields::from([(
                    0x78,
                    Struct(Fields::from([
                        (0x0, Struct(block(game.mode, None))),
                        (0x138, Unsigned(map)),
                    ])),
                )]),
            });
        }
    }
    for game in &script.games {
        chunks.push(Chunk {
            kind: ChunkKind::Mode,
            fields: Fields::from([(0x78, Struct(block(game.mode, Some(&game.values))))]),
        });
    }
    chunks.extend(items.into_iter().map(|fields| Chunk {
        kind: ChunkKind::Workshop,
        fields,
    }));
    CustomSettings {
        settings: Fields::from([(0x30, Struct(workshop))]),
        extra: Fields::new(),
        chunks,
    }
}

/// The Workshop script file of `code` in `dirs`.
fn script_file(dirs: &[PathBuf], code: &str) -> Option<PathBuf> {
    let code = normalise(code);
    if code.is_empty() {
        return None;
    }
    dirs.iter()
        .filter_map(|dir| fs::read_dir(dir).ok())
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.extension().is_some_and(|extension| extension == "txt")
                && path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .and_then(|stem| stem.split('_').next())
                    .is_some_and(|prefix| normalise(prefix) == code)
        })
}

impl Matchmaker {
    /// 40802-40807: a part of an upload to the code service. Complete: a
    /// shared code (40913), or a saved preset or last used settings.
    pub(crate) fn link_upload(
        &self,
        account: &Arc<Account>,
        id: u32,
        fields: Fields,
    ) -> io::Result<Vec<LobbyMessage>> {
        let me = account.account_lo;
        let uploaded = self
            .lock()
            .link_uploads
            .entry(me)
            .or_insert_with(|| Upload::new(LINK_UPLOAD))
            .push(id, fields)?;
        let Some(uploaded) = uploaded else {
            return Ok(Vec::new());
        };
        let saved = Saved::of(&uploaded.header)?;
        match saved {
            Saved::Share(again) => {
                let (code, status) =
                    match self.links.share(me, again.as_deref(), &uploaded.settings)? {
                        Ok(code) => (code, ShareStatus::Done),
                        Err(status) => (again.clone().unwrap_or_default(), status),
                    };
                tracing::info!(
                    "[CG] {} shares settings: code {code} ({status:?})",
                    account.name
                );
                Ok(vec![shared(&code, again.is_some(), status)])
            }
            saved => {
                tracing::info!("[CG] {} saves settings: {saved:?}", account.name);
                self.links.save(me, &saved, &uploaded.settings)?;
                Ok(Vec::new())
            }
        }
    }

    /// 40809: a code's settings (40901 kind 2), or 40912.
    pub(crate) fn load_link(&self, code: &str) -> io::Result<Vec<LobbyMessage>> {
        if let Some(settings) = self.links.code(code)? {
            tracing::info!("[CG] Code {code}: shared settings");
            return Ok(link(code, &settings));
        }
        let Some(path) = script_file(&self.code_dirs, code) else {
            tracing::info!("[CG] Code {code}: unknown");
            return Ok(vec![link_failed(code)]);
        };
        match self.script_code(&path) {
            Ok(settings) => {
                tracing::info!("[CG] Code {code}: {}", path.display());
                Ok(link(code, &settings))
            }
            Err(error) => {
                tracing::warn!("[CG] Code {code}: {}: {error}", path.display());
                Ok(vec![link_failed(code)])
            }
        }
    }

    fn script_code(&self, path: &Path) -> io::Result<CustomSettings> {
        let text = fs::read_to_string(path)?;
        let catalog = self.catalog()?;
        let names = SettingNames::load(&self.data)?;
        let script = ScriptMatch::parse(&text, &catalog, &names)?;
        for skipped in &script.skipped {
            tracing::info!("[CG] {}: not applied: {skipped}", path.display());
        }
        Ok(script_settings(&script, &catalog, |mode| {
            self.maps_of(mode)
        }))
    }

    /// The maps the server hosts in `mode`.
    fn maps_of(&self, mode: u64) -> Vec<u64> {
        let rulesets: Vec<_> = self
            .rules
            .values()
            .flat_map(|rules| rules.rulesets.iter())
            .filter(|ruleset| ruleset.mode == mode)
            .take(1)
            .cloned()
            .collect();
        self.maps
            .map_catalog()
            .into_iter()
            .map(|listing| listing.guid)
            .filter(|&map| {
                !super::rules::map_choices(&self.maps, &[(map, Vec::new())], &rulesets).is_empty()
            })
            .collect()
    }

    /// 40800: the saved settings asked for.
    pub(crate) fn saved_settings(
        &self,
        account: &Arc<Account>,
        request: SavedRequest,
    ) -> io::Result<Vec<LobbyMessage>> {
        self.links.saved(account.account_lo, request)
    }

    /// 40801: a preset goes.
    pub(crate) fn delete_preset(&self, account: &Arc<Account>, slot: u32) -> io::Result<()> {
        self.links.delete(account.account_lo, slot)
    }

    /// 40808: a preset's name.
    pub(crate) fn name_preset(
        &self,
        account: &Arc<Account>,
        slot: u32,
        name: String,
    ) -> io::Result<()> {
        self.links.name(account.account_lo, slot, name)
    }

    /// 40900: the account's preset names, at login.
    pub(crate) fn preset_names(&self, account: &Arc<Account>) -> io::Result<LobbyMessage> {
        Ok(links::preset_names(
            &self.links.preset_names(account.account_lo)?,
        ))
    }
}
