//! A Workshop script's `settings` block as the custom match the server
//! starts (`--workshop`, `docs/dev/doomfist-parkour-plan.md`): the modes and
//! maps it plays, the team sizes, the hero pool, and each mode's ruleset
//! values by their `01C` keys.
//!
//! A mode or hero setting line is named by its game ruleset schema entry's
//! `m_displayText` (`0C6`; the client's importer matches the line with the
//! names of the mode's schemas, `sub_7FF789A8A0C0`), derived into
//! `game_rulesets_174.json` `settings`. So a General line under `modes`
//! resolves against each played mode's own schemas (`0C5`
//! `m_gameRulesetSchemas`), and a mode's block against that mode's; a
//! General line under `heroes` against the heroes' schemas. The lobby lines
//! are no ruleset values (the lobby parser `sub_7FF789A87C40` writes them
//! into the settings struct): only the team sizes are read. What the server
//! does not apply is kept in [`ScriptMatch::skipped`] to be logged, the
//! Overwatch 2 lines among them (`Tank Role Passive Health Bonus`).

use std::{
    collections::{BTreeMap, HashMap},
    fs, io,
    path::Path,
    sync::Arc,
};

use orpheus_core::guid::parse_guid;
use orpheus_script::{graph::ModeValue, rulesets};
use orpheus_workshop::{Catalog, Logic, text::Setting};
use serde_json::Value as Json;

/// The packed GUID type bits of a map (`002`) and of an `01C` identifier.
const MAP_GUID: u64 = 0x0800_0000_0000_0000;
const IDENTIFIER_GUID: u64 = 0x0d80_0000_0000_0000;

/// The settings the text names, by schema: `game_rulesets_174.json`
/// `settings`, and which schemas each mode and hero lists.
#[derive(Debug, Default)]
pub struct SettingNames {
    schemas: HashMap<u64, Vec<Entry>>,
    modes: HashMap<u64, Vec<u64>>,
    heroes: HashMap<u64, Vec<u64>>,
}

#[derive(Debug)]
struct Entry {
    key: u32,
    name: String,
    kind: Kind,
}

#[derive(Debug)]
enum Kind {
    Bool { on: String, off: String },
    Int { min: i64, max: i64 },
    Float { min: f64, max: f64 },
    Enum(Vec<(String, u32)>),
}

impl SettingNames {
    pub fn load(data: &Path) -> io::Result<Self> {
        Self::from_json(&fs::read(data.join(rulesets::FILE))?)
    }

    pub fn from_json(data: &[u8]) -> io::Result<Self> {
        let file: Json = serde_json::from_slice(data).map_err(invalid)?;
        let lists = |name: &str| -> io::Result<HashMap<u64, Vec<u64>>> {
            object(&file[name])?
                .iter()
                .map(|(owner, schemas)| {
                    let schemas = schemas
                        .as_array()
                        .ok_or_else(|| invalid(format!("{name} {owner}: not a list")))?
                        .iter()
                        .map(guid)
                        .collect::<io::Result<_>>()?;
                    Ok((guid(&Json::String(owner.clone()))?, schemas))
                })
                .collect()
        };
        let mut schemas = HashMap::new();
        for (schema, settings) in object(&file["settings"])? {
            let entries = settings["entries"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(entry)
                .collect();
            schemas.insert(guid(&Json::String(schema.clone()))?, entries);
        }
        Ok(Self {
            schemas,
            modes: lists("modes")?,
            heroes: lists("heroes")?,
        })
    }

    /// The entry named `name` (as the importer compares, ignoring case) in
    /// the schemas `owners` list, first schema first.
    fn find<'a>(
        &'a self,
        schemas: impl IntoIterator<Item = &'a u64>,
        name: &str,
    ) -> Option<&'a Entry> {
        schemas
            .into_iter()
            .filter_map(|schema| self.schemas.get(schema))
            .flatten()
            .find(|entry| entry.name.eq_ignore_ascii_case(name))
    }

    fn mode_entry(&self, mode: u64, name: &str) -> Option<&Entry> {
        self.find(self.modes.get(&mode).into_iter().flatten(), name)
    }

    /// A General hero line: the first hero schema that has the name (the
    /// heroes share `Abilities` `0A3:7`, `Combat Modifiers` `0A3:11` and
    /// `Weapons` `0A3:1B`).
    fn hero_entry(&self, name: &str) -> Option<&Entry> {
        let mut schemas: Vec<&u64> = self.heroes.values().flatten().collect();
        schemas.sort_unstable();
        schemas.dedup();
        self.find(schemas, name)
    }
}

fn entry(json: &Json) -> Option<Entry> {
    let text = |field: &str| json[field].as_str().map(str::to_owned);
    let kind = match json["type"].as_str()? {
        "bool" => Kind::Bool {
            on: text("true")?,
            off: text("false")?,
        },
        "int" => Kind::Int {
            min: json["min"].as_i64()?,
            max: json["max"].as_i64()?,
        },
        "float" => Kind::Float {
            min: json["min"].as_f64()?,
            max: json["max"].as_f64()?,
        },
        "enum" => Kind::Enum(
            json["choices"]
                .as_array()?
                .iter()
                .filter_map(|choice| {
                    Some((
                        choice["name"].as_str()?.to_owned(),
                        u32::try_from(choice["identifier"].as_u64()?).ok()?,
                    ))
                })
                .collect(),
        ),
        _ => return None,
    };
    Some(Entry {
        key: u32::try_from(json["key"].as_u64()?).ok()?,
        // An entry without a name (a hero's per-ability lines) is never
        // matched by a line.
        name: text("name")?,
        kind,
    })
}

impl Entry {
    /// The value a line gives, as the client reads the text: a bool by
    /// its words or On/Off (what the writer prints; the archive's scripts
    /// have both), a number with `%` as a fraction (`Respawn Time Scalar:
    /// 0%` is 0.0 of `0..1`), clamped to the entry's range; an enum by its
    /// choice's name.
    fn value(&self, text: &str) -> Option<ModeValue> {
        let number = || -> Option<f64> {
            match text.strip_suffix('%') {
                Some(percent) => percent
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .map(|value| value / 100.0),
                None => text.parse().ok(),
            }
        };
        Some(match &self.kind {
            Kind::Bool { on, off } => {
                let is = |words: &[&str]| words.iter().any(|word| text.eq_ignore_ascii_case(word));
                if is(&[on, "On", "Yes"]) {
                    ModeValue::Bool(true)
                } else if is(&[off, "Off", "No"]) {
                    ModeValue::Bool(false)
                } else {
                    return None;
                }
            }
            Kind::Int { min, max } => {
                ModeValue::Integer((number()?.round() as i64).clamp(*min, *max))
            }
            Kind::Float { min, max } => ModeValue::Float(number()?.clamp(*min, *max)),
            Kind::Enum(choices) => {
                let (_, identifier) = choices
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case(text))?;
                ModeValue::Asset(IDENTIFIER_GUID | u64::from(*identifier))
            }
        })
    }
}

/// A mode the script plays: its maps and ruleset values.
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptGame {
    pub mode: u64,
    /// Its `enabled maps` in order; `None`: every map of the mode, but the
    /// `disabled maps` in [`ScriptGame::without`].
    pub maps: Option<Vec<u64>>,
    pub without: Vec<u64>,
    /// The ruleset values of its games by key: the modes' General lines,
    /// the mode's own block over them, and the heroes' General lines.
    pub values: BTreeMap<u32, ModeValue>,
}

/// The match a script's settings describe.
#[derive(Clone, Debug, Default)]
pub struct ScriptMatch {
    /// The modes in the order the text names them.
    pub games: Vec<ScriptGame>,
    /// `Max Team 1 Players` and `Max Team 2 Players`.
    pub team_sizes: [Option<u8>; 2],
    /// The heroes' General `enabled heroes`; `None`: every hero.
    pub heroes: Option<Vec<u64>>,
    /// The script's rules, for the Workshop engine.
    pub logic: Arc<Logic>,
    /// The lines the match does not apply, each with why.
    pub skipped: Vec<String>,
}

impl ScriptMatch {
    pub fn parse(text: &str, catalog: &Catalog, names: &SettingNames) -> io::Result<Self> {
        let script = orpheus_workshop::parse(text, catalog)
            .map_err(|error| invalid(format!("Workshop text: {error}")))?;
        let mut found = Self {
            logic: Arc::new(script.logic),
            ..Self::default()
        };
        let mut mode_lines: Vec<(&str, &str)> = Vec::new();
        let mut hero_lines: Vec<(&str, &str)> = Vec::new();
        let mut blocks: Vec<(u64, &[Setting])> = Vec::new();
        for setting in &script.settings {
            let Setting::Block(name, block) = setting else {
                continue;
            };
            match name.as_str() {
                "lobby" => found.lobby(block),
                "modes" => {
                    for setting in block {
                        match setting {
                            Setting::Block(name, block) if name == "General" => {
                                mode_lines.extend(pairs(block));
                            }
                            Setting::Block(name, block) => match catalog.game_mode(mode_name(name))
                            {
                                Some(mode) if !name.starts_with("disabled ") => {
                                    blocks.push((mode, block));
                                }
                                Some(_) => {}
                                None => found.skip(format!("modes: {name}: no such game mode")),
                            },
                            other => found.skip(format!("modes: {other:?}")),
                        }
                    }
                }
                "heroes" => found.heroes(block, catalog, &mut hero_lines),
                "main" | "workshop" | "extensions" => {}
                other => found.skip(format!("{other}: not read")),
            }
        }
        let mut hero_values = BTreeMap::new();
        for (name, text) in hero_lines {
            match names
                .hero_entry(name)
                .and_then(|entry| Some((entry.key, entry.value(text)?)))
            {
                Some((key, value)) => {
                    hero_values.insert(key, value);
                }
                None => found.skip(format!("heroes: {name}: {text}: no 1.74 hero setting")),
            }
        }
        for (mode, block) in blocks {
            let mut game = ScriptGame {
                mode,
                maps: None,
                without: Vec::new(),
                values: BTreeMap::new(),
            };
            let mode_name = catalog.game_mode_name(mode).unwrap_or("?").to_owned();
            let own = pairs(block);
            for (name, text) in mode_lines.iter().chain(&own) {
                match names
                    .mode_entry(mode, name)
                    .and_then(|entry| Some((entry.key, entry.value(text)?)))
                {
                    Some((key, value)) => {
                        game.values.insert(key, value);
                    }
                    None => found.skip(format!(
                        "modes: {mode_name}: {name}: {text}: no 1.74 setting of the mode"
                    )),
                }
            }
            game.values
                .extend(hero_values.iter().map(|(&key, &value)| (key, value)));
            for setting in block {
                if let Setting::Block(list, maps) = setting {
                    let maps = found.maps(maps, catalog);
                    match list.as_str() {
                        "enabled maps" => game.maps = Some(maps),
                        "disabled maps" => game.without = maps,
                        other => found.skip(format!("modes: {mode_name}: {other}: not read")),
                    }
                }
            }
            if game.maps.as_ref().is_none_or(|maps| !maps.is_empty()) {
                found.games.push(game);
            }
        }
        Ok(found)
    }

    fn skip(&mut self, why: String) {
        self.skipped.push(why);
    }

    fn lobby(&mut self, block: &[Setting]) {
        for (name, text) in pairs(block) {
            let team = match name {
                "Max Team 1 Players" => 0,
                "Max Team 2 Players" => 1,
                _ => {
                    self.skip(format!("lobby: {name}: {text}: not applied"));
                    continue;
                }
            };
            match text.parse() {
                Ok(size) => self.team_sizes[team] = Some(size),
                Err(_) => self.skip(format!("lobby: {name}: {text}: not a number")),
            }
        }
    }

    fn heroes<'a>(
        &mut self,
        block: &'a [Setting],
        catalog: &Catalog,
        lines: &mut Vec<(&'a str, &'a str)>,
    ) {
        for setting in block {
            match setting {
                Setting::Block(name, block) if name == "General" => {
                    lines.extend(pairs(block));
                    for setting in block {
                        match setting {
                            Setting::Block(list, heroes) if list == "enabled heroes" => {
                                self.heroes = Some(self.hero_list(heroes, catalog));
                            }
                            Setting::Block(list, heroes) if list == "disabled heroes" => {
                                let disabled = self.hero_list(heroes, catalog);
                                self.heroes = Some(
                                    catalog
                                        .heroes()
                                        .map(|(_, hero)| hero)
                                        .filter(|hero| !disabled.contains(hero))
                                        .collect(),
                                );
                            }
                            Setting::Block(hero, _) => {
                                self.skip(format!(
                                    "heroes: General: {hero}: hero settings not applied"
                                ));
                            }
                            _ => {}
                        }
                    }
                }
                Setting::Block(name, _) => {
                    self.skip(format!("heroes: {name}: team settings not applied"))
                }
                other => self.skip(format!("heroes: {other:?}")),
            }
        }
    }

    fn hero_list(&mut self, block: &[Setting], catalog: &Catalog) -> Vec<u64> {
        let mut heroes = Vec::new();
        for setting in block {
            match setting {
                Setting::Line(name) => match catalog.hero(name) {
                    Some(hero) => heroes.push(hero),
                    None => self.skip(format!("heroes: {name}: no such hero")),
                },
                other => self.skip(format!("heroes: {other:?}")),
            }
        }
        heroes
    }

    /// A map list's maps as `002` GUIDs. An Overwatch 2 line has the map's
    /// `01C` identifier after its name (`Hanamura 972777519512068154`),
    /// which the 1.74 importer refuses (`Workshop.md` §6); the name is read.
    fn maps(&mut self, block: &[Setting], catalog: &Catalog) -> Vec<u64> {
        let mut maps = Vec::new();
        for setting in block {
            let Setting::Line(line) = setting else {
                self.skip(format!("maps: {setting:?}"));
                continue;
            };
            let name = setting.ow2_map_id().map_or(line.as_str(), |(name, _)| name);
            match catalog.map(name) {
                Some(index) => maps.push(MAP_GUID | u64::from(index)),
                None => self.skip(format!("maps: {line}: no such map")),
            }
        }
        maps
    }
}

/// A mode block's mode name (`disabled Assault` is Assault, off).
fn mode_name(block: &str) -> &str {
    block.strip_prefix("disabled ").unwrap_or(block)
}

/// The `key: value` lines of a block.
fn pairs(block: &[Setting]) -> Vec<(&str, &str)> {
    block
        .iter()
        .filter_map(|setting| match setting {
            Setting::Pair(name, value) => Some((name.as_str(), value.as_str())),
            _ => None,
        })
        .collect()
}

fn object(json: &Json) -> io::Result<&serde_json::Map<String, Json>> {
    json.as_object()
        .ok_or_else(|| invalid(format!("{}: not an object", rulesets::FILE)))
}

fn guid(json: &Json) -> io::Result<u64> {
    json.as_str()
        .and_then(parse_guid)
        .ok_or_else(|| invalid(format!("{}: bad GUID {json}", rulesets::FILE)))
}

fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// C32S2's settings block (`dfp-archive`, Hanamura Doomfist parkour),
    /// OW2-era lines included.
    const C32S2: &str = r#"settings
{
	lobby
	{
		Max Spectators: 12
		Max Team 1 Players: 12
		Max Team 2 Players: 0
		Return To Lobby: Never
	}
	modes
	{
		Assault
		{
			enabled maps
			{
				Hanamura 972777519512068154
			}
		}
		Control
		{
			enabled maps
			{
			}
		}
		General
		{
			Allow Hero Switching: Off
			Enemy Health Bars: Off
			Game Mode Start: Immediately
			Hero Limit: Off
			Kill Cam: Off
			Kill Feed: Off
			Respawn Time Scalar: 0%
			Spawn Health Packs: Disabled
			Tank Role Passive Health Bonus: Disabled
		}
	}
	heroes
	{
		General
		{
			Ultimate Generation - Combat: 0%
			Ultimate Generation - Passive: 0%
			enabled heroes
			{
				Doomfist
			}
		}
	}
}
"#;

    /// Every C32S2 setting the 1.74 schemas name reaches its `01C` key with
    /// the value the text gives (the keys the graphs read: `0C8E` #9531 and
    /// #9523, `0046`/`0CBB` #9542, `01CB` #9541, `0DEA` #9540, `0D24`
    /// #9539, the mode graphs #9672, `0C90` #9548, `0B8B` #13022/#12918);
    /// Assault on Hanamura only (Control's empty list plays nothing),
    /// teams of 12 and 0, Doomfist alone; the OW2 line is skipped.
    #[test]
    #[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
    fn c32s2_settings_map_to_hanamura_assault_and_their_ruleset_keys() {
        let data = orpheus_testkit::runtime_dir().join("data");
        let catalog = Catalog::load(&data).unwrap();
        let names = SettingNames::load(&data).unwrap();
        let script = ScriptMatch::parse(C32S2, &catalog, &names).unwrap();
        let assault = catalog.game_mode("Assault").unwrap();
        let [game] = script.games.as_slice() else {
            panic!("{:?}", script.games)
        };
        assert_eq!(game.mode, assault);
        assert_eq!(game.maps, Some(vec![0x0800_0000_0000_0165]));
        let identifier = |index: u64| ModeValue::Asset(IDENTIFIER_GUID | index);
        assert_eq!(
            game.values,
            BTreeMap::from([
                (9523, ModeValue::Bool(false)),
                (9531, ModeValue::Float(0.0)),
                (9539, ModeValue::Bool(false)),
                (9540, ModeValue::Bool(false)),
                (9541, ModeValue::Bool(false)),
                (9542, identifier(9663)),
                (9548, identifier(9549)),
                (9672, identifier(9673)),
                (12918, ModeValue::Float(0.0)),
                (13022, ModeValue::Float(0.0)),
            ])
        );
        assert_eq!(script.team_sizes, [Some(12), Some(0)]);
        assert_eq!(script.heroes, Some(vec![catalog.hero("Doomfist").unwrap()]));
        assert!(
            script
                .skipped
                .iter()
                .any(|line| line.contains("Tank Role Passive Health Bonus")),
            "{:?}",
            script.skipped
        );
    }
}
