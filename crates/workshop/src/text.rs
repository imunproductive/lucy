//! The Workshop text (Settings > Workshop > Paste; the client's parser is
//! `sub_7FF789A7E200`, its writer `sub_7FF789A7C1F0`; Workshop.md §3.2).
//!
//! The grammar follows what the client's writer produces:
//! - `settings { … }` (kept as a tree of lines, pairs and blocks; the
//!   server's custom game settings read it), `variables { global: N: name …
//!   player: N: name … }`, `subroutines { N: name … }`, then the rules:
//!   `disabled? rule("name") { event { … } conditions { … } actions { … } }`.
//! - The event block: the event's name, then the team (`All`, `Team 1`,
//!   `Team 2`) and the player (`All`, `Slot N` or a hero), or for a
//!   subroutine rule the subroutine's name (`sub_7FF789A7B480`).
//! - Each condition and action may be preceded by a `"comment"` and
//!   `disabled`. A condition is `left op right`.
//! - Values are written by name, `Name(arguments)` when the value has
//!   parameters (`sub_7FF789CA5520`), except: Number (a bare number), Team
//!   (`All Teams`, `Team 1`, `Team 2`), True and False, Global Variable
//!   (`Global.name`), Player Variable (`player.name`), Value In Array
//!   (`array[index]`), and the operators, by precedence from loosest:
//!   `? :` (If-Then-Else), `||`, `&&`, the comparisons, `+ -`, `* / %`,
//!   `^`, the prefix `!`.
//! - Set/Modify Global/Player Variable [At Index] are assignments
//!   (`Global.x = v;`, `p.x[i] += v;`, `sub_7FF789CA7070`; the compound
//!   operators are Add through Raise To Power, enum `4A28`).
//!
//! Names match case-insensitively and the longest name wins, as the
//! client's settings parser matches names (`sub_7FF789A8A0C0`). The older
//! function forms (`Compare(a, ==, b)`, `Global Variable(x)`,
//! `Set Global Variable(x, v)`) parse as the values and actions they name.
//!
//! OW2-era lines: the archive's scripts were exported by Overwatch 2 and
//! carry lines the 1.74 client does not know, such as `Tank Role Passive
//! Health Bonus: Disabled` in a mode block and a map's OW2 identifier after
//! its name (`Hanamura 972777519512068154`, `01C:A03A`). The client's
//! settings parser matches each setting line against its schemas' names and
//! fails the import on a line no name matches (`sub_7FF789A8A0C0` error
//! `07C:100AE`, then `sub_7FF789A891C0` error `07C:10099`), and a map list
//! entry must be a map name (`sub_7FF789A88940` error `07C:D412`) [static
//! reading; not tried in a game]. This parser keeps the settings as text;
//! [`Setting::ow2_map_id`] tells the map lines with an identifier apart.

use std::{error, fmt};

use crate::{
    catalog::{Catalog, Id, Param, ParamKind},
    logic::{
        Action, Arg, Compare, Condition, Event, Logic, Rule, Value, WorkshopSetting, actions,
        default_arg, events, ids,
    },
};

/// The enum of the Button value (`74D2`) and of the modify operators.
const BUTTON_ENUM: Id = 0x4A39;
const MODIFY_ADD: Id = 0x4A29;
/// The Color value's enum (`50A6`).
const COLOR_ENUM: Id = 0x50A6;

/// Former hero names the archive's scripts still use, and the 1.74 name.
/// The 1.74 client's own importer knows only the new name; the server's
/// parser takes both.
const HERO_ALIASES: &[(&str, &str)] = &[("McCree", "Cassidy")];

/// One line of the `settings` block.
#[derive(Clone, Debug, PartialEq)]
pub enum Setting {
    /// A bare line (a map or hero name).
    Line(String),
    /// `key: value`.
    Pair(String, String),
    Block(String, Vec<Setting>),
}

impl Setting {
    /// A map line with an Overwatch 2 identifier after the name
    /// (`Hanamura 972777519512068154`): the name and the identifier.
    pub fn ow2_map_id(&self) -> Option<(&str, u64)> {
        let Self::Line(line) = self else { return None };
        let (name, id) = line.rsplit_once(' ')?;
        let id: u64 = id.parse().ok()?;
        (id >> 48 == 0x0D80).then_some((name, id))
    }
}

/// A parsed script: its settings and its rules.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Script {
    pub settings: Vec<Setting>,
    pub logic: Logic,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl error::Error for ParseError {}

type Result<T, E = ParseError> = std::result::Result<T, E>;

/// Parse a Workshop text into its settings and normalised rules.
pub fn parse(text: &str, catalog: &Catalog) -> Result<Script> {
    let mut parser = Parser::new(text, catalog);
    let mut script = parser.script()?;
    script.logic.normalise(catalog);
    Ok(script)
}

/// The characters the writer leaves out of names (`sub_7FF789CA11E0`'s
/// table, used by `sub_7FF789CA4490` and the rule writer): "Visible To,
/// Position, and Radius" is written "Visible To Position and Radius".
const UNWRITTEN: &[char] = &['(', ')', '{', '|', '}', '"', ';', '/', '\\', ','];

/// Names of one kind as the text writes them, longest first.
struct Names<T> {
    names: Vec<(String, T)>,
}

impl<T: Copy> Names<T> {
    fn new(names: impl IntoIterator<Item = (String, T)>) -> Self {
        let mut names: Vec<(String, T)> = names
            .into_iter()
            .map(|(name, item)| (name.to_lowercase().replace(UNWRITTEN, ""), item))
            .filter(|(name, _)| !name.trim().is_empty())
            .collect();
        names.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
        Self { names }
    }

    /// The longest name at the start of `rest` that ends at a word
    /// boundary, and its length in bytes.
    fn longest(&self, rest: &str) -> Option<(usize, T)> {
        let lower: String = rest
            .chars()
            .take(LONGEST_NAME)
            .flat_map(char::to_lowercase)
            .collect();
        self.names.iter().find_map(|(name, item)| {
            if !lower.starts_with(name.as_str()) {
                return None;
            }
            let chars = name.chars().count();
            let length = rest
                .char_indices()
                .nth(chars)
                .map_or(rest.len(), |(at, _)| at);
            let boundary = rest[length..]
                .chars()
                .next()
                .is_none_or(|next| !(next.is_alphanumeric() || next == '_'))
                || !name.ends_with(|c: char| c.is_alphanumeric());
            boundary.then_some((length, *item))
        })
    }
}

/// Longer than any name the catalog has.
const LONGEST_NAME: usize = 80;

struct Parser<'a> {
    text: &'a str,
    at: usize,
    catalog: &'a Catalog,
    values: Names<Id>,
    actions: Names<u16>,
    heroes: Names<u64>,
    maps: Names<u32>,
    game_modes: Names<u64>,
    buttons: Names<Id>,
    teams: Names<u8>,
    /// The event block's teams (its `All` is read first).
    event_teams: Names<u8>,
    globals: Vec<String>,
    players: Vec<String>,
    subroutines: Vec<String>,
}

/// Binary operators by precedence level (the writer's levels,
/// `sub_7FF789CA5520`): 2 `||`, 3 `&&`, 4 comparisons, 6 `+ -`, 7 `* / %`,
/// 8 `^`.
const LEVELS: [&[(&str, Op)]; 6] = [
    &[("||", Op::Value(ids::OR))],
    &[("&&", Op::Value(ids::AND))],
    &[
        ("==", Op::Compare(Compare::Eq)),
        ("!=", Op::Compare(Compare::Ne)),
        ("<=", Op::Compare(Compare::Le)),
        (">=", Op::Compare(Compare::Ge)),
        ("<", Op::Compare(Compare::Lt)),
        (">", Op::Compare(Compare::Gt)),
    ],
    &[("+", Op::Value(ids::ADD)), ("-", Op::Value(ids::SUBTRACT))],
    &[
        ("*", Op::Value(ids::MULTIPLY)),
        ("/", Op::Value(ids::DIVIDE)),
        ("%", Op::Value(ids::MODULO)),
    ],
    &[("^", Op::Value(ids::RAISE_TO_POWER))],
];

#[derive(Clone, Copy)]
enum Op {
    Value(Id),
    Compare(Compare),
}

impl<'a> Parser<'a> {
    fn new(text: &'a str, catalog: &'a Catalog) -> Self {
        let button_names = catalog
            .enum_def(BUTTON_ENUM)
            .map(|def| def.values.clone())
            .unwrap_or_default();
        Self {
            text,
            at: 0,
            catalog,
            values: Names::new(
                catalog
                    .values
                    .iter()
                    .map(|value| (value.name.clone(), value.id)),
            ),
            actions: Names::new(
                catalog
                    .actions
                    .iter()
                    .enumerate()
                    .map(|(index, action)| (action.name.clone(), index as u16)),
            ),
            heroes: Names::new(
                catalog
                    .heroes()
                    .map(|(name, guid)| (name.to_owned(), guid))
                    .chain(HERO_ALIASES.iter().filter_map(|&(alias, name)| {
                        Some((alias.to_owned(), catalog.hero(name)?))
                    })),
            ),
            maps: Names::new(catalog.maps().map(|(name, index)| (name.to_owned(), index))),
            game_modes: Names::new(
                catalog
                    .game_modes()
                    .map(|(name, guid)| (name.to_owned(), guid)),
            ),
            buttons: Names::new(button_names.into_iter().map(|(id, name)| (name, id))),
            teams: Names::new([
                ("All Teams".to_owned(), 0),
                ("Team 1".to_owned(), 1),
                ("Team 2".to_owned(), 2),
            ]),
            event_teams: Names::new([("Team 1".to_owned(), 1), ("Team 2".to_owned(), 2)]),
            globals: Vec::new(),
            players: Vec::new(),
            subroutines: Vec::new(),
        }
    }

    // ---- characters ----

    fn rest(&self) -> &'a str {
        &self.text[self.at..]
    }

    fn error<T>(&self, message: impl Into<String>) -> Result<T> {
        let line = self.text[..self.at].matches('\n').count() + 1;
        let near: String = self.rest().chars().take(40).collect();
        Err(ParseError {
            line,
            message: format!("{} at {near:?}", message.into()),
        })
    }

    fn skip_space(&mut self) {
        let trimmed = self.rest().trim_start();
        self.at = self.text.len() - trimmed.len();
    }

    fn peek(&mut self) -> Option<char> {
        self.skip_space();
        self.rest().chars().next()
    }

    fn eat(&mut self, token: &str) -> bool {
        self.skip_space();
        if self.rest().starts_with(token) {
            self.at += token.len();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, token: &str) -> Result<()> {
        if self.eat(token) {
            Ok(())
        } else {
            self.error(format!("expected {token:?}"))
        }
    }

    /// A keyword: case-insensitive, at a word boundary.
    fn eat_word(&mut self, word: &str) -> bool {
        self.skip_space();
        let rest = self.rest();
        let matches = rest
            .get(..word.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(word))
            && rest[word.len()..]
                .chars()
                .next()
                .is_none_or(|next| !(next.is_alphanumeric() || next == '_'));
        if matches {
            self.at += word.len();
        }
        matches
    }

    /// The longest of the parser's own names of one kind.
    fn field_name<T: Copy>(&mut self, names: fn(&Self) -> &Names<T>) -> Option<T> {
        self.skip_space();
        let (length, item) = names(self).longest(self.rest())?;
        self.at += length;
        Some(item)
    }

    fn name<T: Copy>(&mut self, names: &Names<T>) -> Option<T> {
        self.skip_space();
        let (length, item) = names.longest(self.rest())?;
        self.at += length;
        Some(item)
    }

    /// An identifier: a variable or subroutine name.
    fn identifier(&mut self) -> Result<&'a str> {
        self.skip_space();
        let rest = self.rest();
        let length = rest
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        if length == 0 {
            return self.error("expected a name");
        }
        self.at += length;
        Ok(&rest[..length])
    }

    fn string(&mut self) -> Result<String> {
        self.expect("\"")?;
        let mut text = String::new();
        let mut chars = self.rest().char_indices();
        while let Some((offset, c)) = chars.next() {
            match c {
                '"' => {
                    self.at += offset + 1;
                    return Ok(text);
                }
                '\\' => match chars.next() {
                    Some((_, 'n')) => text.push('\n'),
                    Some((_, 'r')) => text.push('\r'),
                    Some((_, 't')) => text.push('\t'),
                    Some((_, other)) => text.push(other),
                    None => break,
                },
                c => text.push(c),
            }
        }
        self.error("unterminated string")
    }

    fn number(&mut self) -> Option<f64> {
        self.skip_space();
        let rest = self.rest();
        let mut end = usize::from(rest.starts_with('-'));
        let digits = |text: &str| {
            text.find(|c: char| !c.is_ascii_digit())
                .unwrap_or(text.len())
        };
        let whole = digits(&rest[end..]);
        if whole == 0 {
            return None;
        }
        end += whole;
        if rest[end..].starts_with('.') && digits(&rest[end + 1..]) > 0 {
            end += 1 + digits(&rest[end + 1..]);
        }
        let value = rest[..end].parse().ok()?;
        self.at += end;
        Some(value)
    }

    // ---- script ----

    fn script(&mut self) -> Result<Script> {
        let mut script = Script::default();
        loop {
            self.skip_space();
            if self.rest().is_empty() {
                break;
            }
            if self.eat_word("settings") {
                self.expect("{")?;
                script.settings = self.settings_block()?;
                script.logic.workshop_settings = workshop_settings(&script.settings);
            } else if self.eat_word("variables") {
                self.variables()?;
            } else if self.eat_word("subroutines") {
                self.expect("{")?;
                self.subroutines = self.numbered_names()?;
            } else {
                let rule = self.rule()?;
                script.logic.rules.push(rule);
            }
        }
        script.logic.global_names = self.globals.clone();
        script.logic.player_names = self.players.clone();
        script.logic.subroutine_names = self.subroutines.clone();
        Ok(script)
    }

    /// The lines of a settings block up to its `}`.
    fn settings_block(&mut self) -> Result<Vec<Setting>> {
        let mut settings = Vec::new();
        loop {
            self.skip_space();
            if self.rest().is_empty() {
                return self.error("unterminated settings block");
            }
            if self.eat("}") {
                return Ok(settings);
            }
            let rest = self.rest();
            let end = rest.find('\n').unwrap_or(rest.len());
            let line = rest[..end].trim().to_owned();
            self.at += end;
            if self.eat("{") {
                let block = self.settings_block()?;
                settings.push(Setting::Block(line, block));
            } else if line.starts_with('"') || line.ends_with('"') {
                // Description: "…" may hold ": ".
                let (key, value) = line.split_once(':').unwrap_or((&line, ""));
                settings.push(Setting::Pair(key.trim().into(), value.trim().into()));
            } else if let Some((key, value)) = line.split_once(": ") {
                settings.push(Setting::Pair(key.trim().into(), value.trim().into()));
            } else {
                settings.push(Setting::Line(line));
            }
        }
    }

    fn variables(&mut self) -> Result<()> {
        self.expect("{")?;
        loop {
            if self.eat("}") {
                return Ok(());
            }
            let global = if self.eat_word("global") {
                true
            } else if self.eat_word("player") {
                false
            } else {
                return self.error("expected global: or player:");
            };
            self.expect(":")?;
            let mut names = Vec::new();
            while let Some(index) = self.number() {
                self.expect(":")?;
                let name = self.identifier()?.to_owned();
                let index = index as usize;
                if names.len() <= index {
                    names.resize(index + 1, String::new());
                }
                names[index] = name;
            }
            if global {
                self.globals = names;
            } else {
                self.players = names;
            }
        }
    }

    fn numbered_names(&mut self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        loop {
            if self.eat("}") {
                return Ok(names);
            }
            let Some(index) = self.number() else {
                return self.error("expected a number");
            };
            self.expect(":")?;
            let name = self.identifier()?.to_owned();
            let index = index as usize;
            if names.len() <= index {
                names.resize(index + 1, String::new());
            }
            names[index] = name;
        }
    }

    fn rule(&mut self) -> Result<Rule> {
        let disabled = self.eat_word("disabled");
        if !self.eat_word("rule") {
            return self.error("expected a rule");
        }
        self.expect("(")?;
        let name = self.string()?;
        self.expect(")")?;
        self.expect("{")?;
        if !self.eat_word("event") {
            return self.error("expected the event block");
        }
        let event = self.event()?;
        let mut rule = Rule {
            name,
            disabled,
            event,
            conditions: Vec::new(),
            actions: Vec::new(),
        };
        if self.eat_word("conditions") {
            self.expect("{")?;
            while !self.eat("}") {
                rule.conditions.push(self.condition()?);
            }
        }
        if self.eat_word("actions") {
            self.expect("{")?;
            while !self.eat("}") {
                rule.actions.push(self.statement()?);
            }
        }
        self.expect("}")?;
        Ok(rule)
    }

    fn event(&mut self) -> Result<Event> {
        self.expect("{")?;
        let rest = self.rest().trim_start();
        let end = rest.find(';').unwrap_or(0);
        let Some(kind) = self.catalog.event_named(rest[..end].trim()) else {
            return self.error("unknown event");
        };
        self.skip_space();
        self.at += end;
        self.expect(";")?;
        let mut event = Event {
            kind,
            ..Event::global()
        };
        if kind == events::SUBROUTINE {
            let name = self.identifier()?;
            event.slot = self.subroutine_index(name)?;
            self.expect(";")?;
        } else if !self.eat("}") {
            event.team = if self.eat_word("All") {
                0
            } else if let Some(team) = self.field_name(|parser| &parser.event_teams) {
                team
            } else {
                return self.error("expected the event's team");
            };
            self.expect(";")?;
            if self.eat_word("All") {
            } else if self.eat_word("Slot") {
                let Some(slot) = self.number() else {
                    return self.error("expected a slot number");
                };
                event.slot = slot as i32;
            } else if let Some(hero) = self.field_name(|parser| &parser.heroes) {
                event.hero = hero;
            } else {
                return self.error("expected the event's player");
            }
            self.expect(";")?;
        } else {
            return Ok(event);
        }
        self.expect("}")?;
        Ok(event)
    }

    /// A comment and `disabled` before a condition or an action.
    fn preamble(&mut self) -> Result<(String, bool)> {
        let comment = if self.peek() == Some('"') {
            self.string()?
        } else {
            String::new()
        };
        Ok((comment, self.eat_word("disabled")))
    }

    fn condition(&mut self) -> Result<Condition> {
        let (comment, disabled) = self.preamble()?;
        let value = self.expr()?;
        self.expect(";")?;
        let (left, op, right) = match value {
            Value {
                id: ids::COMPARE,
                mut args,
            } if args.len() == 3 => {
                let right = args.pop().expect("three arguments");
                let Some(Arg::Compare(op)) = args.pop() else {
                    return self.error("a comparison without its operator");
                };
                let left = args.pop().expect("three arguments");
                (left, op, right)
            }
            value => (
                Arg::Value(value),
                Compare::Eq,
                Arg::Value(Value::new(ids::TRUE, Vec::new())),
            ),
        };
        Ok(Condition {
            left,
            op,
            right,
            invert: false,
            disabled,
            comment,
        })
    }

    fn statement(&mut self) -> Result<Action> {
        let (comment, disabled) = self.preamble()?;
        let start = self.at;
        if let Some(index) = self.field_name(|parser| &parser.actions) {
            if matches!(self.peek(), Some('(' | ';')) {
                let params = &self.catalog.actions[usize::from(index)].params;
                let args = if self.eat("(") {
                    self.args(params)?
                } else {
                    Vec::new()
                };
                self.expect(";")?;
                return Ok(Action {
                    index,
                    args,
                    disabled,
                    comment,
                });
            }
            self.at = start;
        }
        let target = self.unary()?;
        let op = ["+=", "-=", "*=", "/=", "%=", "^="]
            .iter()
            .position(|op| self.eat(op))
            .map(|position| MODIFY_ADD + position as Id);
        if op.is_none() && !self.eat("=") {
            return self.error("expected an action or an assignment");
        }
        let value = Arg::Value(self.expr()?);
        self.expect(";")?;
        let (index, mut args) = match (target, op) {
            (
                Value {
                    id: ids::GLOBAL_VARIABLE,
                    args,
                },
                None,
            ) => (actions::SET_GLOBAL_VARIABLE, args),
            (
                Value {
                    id: ids::GLOBAL_VARIABLE,
                    args,
                },
                Some(_),
            ) => (actions::MODIFY_GLOBAL_VARIABLE, args),
            (
                Value {
                    id: ids::PLAYER_VARIABLE,
                    args,
                },
                None,
            ) => (actions::SET_PLAYER_VARIABLE, args),
            (
                Value {
                    id: ids::PLAYER_VARIABLE,
                    args,
                },
                Some(_),
            ) => (actions::MODIFY_PLAYER_VARIABLE, args),
            (
                Value {
                    id: ids::VALUE_IN_ARRAY,
                    mut args,
                },
                op,
            ) if args.len() == 2 => {
                let index = args.pop().expect("two arguments");
                let Some(Arg::Value(array)) = args.pop() else {
                    return self.error("an indexed assignment to a non-variable");
                };
                let (action, mut args) = match (array.id, op) {
                    (ids::GLOBAL_VARIABLE, None) => {
                        (actions::SET_GLOBAL_VARIABLE_AT_INDEX, array.args)
                    }
                    (ids::GLOBAL_VARIABLE, Some(_)) => {
                        (actions::MODIFY_GLOBAL_VARIABLE_AT_INDEX, array.args)
                    }
                    (ids::PLAYER_VARIABLE, None) => {
                        (actions::SET_PLAYER_VARIABLE_AT_INDEX, array.args)
                    }
                    (ids::PLAYER_VARIABLE, Some(_)) => {
                        (actions::MODIFY_PLAYER_VARIABLE_AT_INDEX, array.args)
                    }
                    _ => return self.error("an indexed assignment to a non-variable"),
                };
                args.push(index);
                (action, args)
            }
            _ => return self.error("an assignment to a non-variable"),
        };
        if let Some(op) = op {
            args.push(Arg::Enum(op));
        }
        args.push(value);
        Ok(Action {
            index,
            args,
            disabled,
            comment,
        })
    }

    // ---- values ----

    fn expr(&mut self) -> Result<Value> {
        let condition = self.binary(0)?;
        if self.eat("?") {
            let then = self.expr()?;
            self.expect(":")?;
            let otherwise = self.expr()?;
            return Ok(Value::new(
                ids::IF_THEN_ELSE,
                vec![
                    Arg::Value(condition),
                    Arg::Value(then),
                    Arg::Value(otherwise),
                ],
            ));
        }
        Ok(condition)
    }

    fn binary(&mut self, level: usize) -> Result<Value> {
        let Some(ops) = LEVELS.get(level) else {
            return self.unary();
        };
        let mut left = self.binary(level + 1)?;
        'operators: loop {
            self.skip_space();
            for &(symbol, op) in ops.iter() {
                // The two-character operators come first in their level;
                // `+=` and the like end the expression (assignments).
                let rest = self.rest();
                if rest.starts_with(symbol) && !(symbol.len() == 1 && rest[1..].starts_with('=')) {
                    self.at += symbol.len();
                    let right = self.binary(level + 1)?;
                    left = match op {
                        Op::Value(id) => Value::new(id, vec![Arg::Value(left), Arg::Value(right)]),
                        Op::Compare(compare) => Value::new(
                            ids::COMPARE,
                            vec![Arg::Value(left), Arg::Compare(compare), Arg::Value(right)],
                        ),
                    };
                    continue 'operators;
                }
            }
            return Ok(left);
        }
    }

    fn unary(&mut self) -> Result<Value> {
        self.skip_space();
        if self.rest().starts_with('!') && !self.rest().starts_with("!=") {
            self.at += 1;
            let operand = self.unary()?;
            return Ok(Value::new(ids::NOT, vec![Arg::Value(operand)]));
        }
        let mut value = self.primary()?;
        loop {
            if self.eat("[") {
                let index = self.expr()?;
                self.expect("]")?;
                value = Value::new(
                    ids::VALUE_IN_ARRAY,
                    vec![Arg::Value(value), Arg::Value(index)],
                );
            } else if self.peek() == Some('.') {
                self.at += 1;
                let name = self.identifier()?;
                let variable = self.variable_index(name, false)?;
                value = Value::new(
                    ids::PLAYER_VARIABLE,
                    vec![Arg::Value(value), Arg::Variable(variable)],
                );
            } else {
                return Ok(value);
            }
        }
    }

    fn primary(&mut self) -> Result<Value> {
        if self.eat("(") {
            let value = self.expr()?;
            self.expect(")")?;
            return Ok(value);
        }
        if let Some(number) = self.number() {
            return Ok(Value::number(number as f32));
        }
        let start = self.at;
        if self.eat_word("Global") && self.eat(".") {
            let name = self.identifier()?;
            let variable = self.variable_index(name, true)?;
            return Ok(Value::new(
                ids::GLOBAL_VARIABLE,
                vec![Arg::Variable(variable)],
            ));
        }
        self.at = start;
        if let Some(team) = self.field_name(|parser| &parser.teams) {
            return Ok(Value::new(ids::TEAM, vec![Arg::Team(team)]));
        }
        let Some(id) = self.field_name(|parser| &parser.values) else {
            return self.error("expected a value");
        };
        let params = &self
            .catalog
            .value(id)
            .expect("named values are in the catalog")
            .params;
        let args = if self.eat("(") {
            self.args(params)?
        } else {
            Vec::new()
        };
        Ok(Value::new(id, args))
    }

    /// The arguments of a value or an action after its `(`, through `)`.
    /// The Array value's one parameter takes every argument; hidden
    /// parameters are not written and take their defaults.
    fn args(&mut self, params: &[Param]) -> Result<Vec<Arg>> {
        if matches!(params, [param] if param.kind == ParamKind::Array) {
            let mut items = Vec::new();
            if !self.eat(")") {
                loop {
                    items.push(self.expr()?);
                    if self.eat(")") {
                        break;
                    }
                    self.expect(",")?;
                }
            }
            return Ok(vec![Arg::Array(items)]);
        }
        let mut args = Vec::new();
        let mut closed = self.eat(")");
        for param in params {
            if closed {
                break;
            }
            if param.hidden {
                args.push(default_arg(self.catalog, param));
                continue;
            }
            args.push(self.arg(param)?);
            if self.eat(")") {
                closed = true;
            } else {
                self.expect(",")?;
            }
        }
        if !closed {
            return self.error("too many arguments");
        }
        Ok(args)
    }

    /// A value slot. A bare Color or Button constant in a slot whose
    /// default is the Color or Button value stands for that value: the
    /// older text form the normaliser rewrites (`sub_7FF789C9F790`,
    /// `sub_7FF789C9FA10`).
    fn slot(&mut self, param: &Param) -> Result<Value> {
        let start = self.at;
        let constant = match param.default_value {
            Some(ids::COLOR) => {
                let names = self.enum_names(COLOR_ENUM);
                self.name(&names)
                    .map(|id| Value::new(ids::COLOR, vec![Arg::Enum(id)]))
            }
            Some(ids::BUTTON) => self
                .field_name(|parser| &parser.buttons)
                .map(|id| Value::new(ids::BUTTON, vec![Arg::Button(id)])),
            _ => None,
        };
        if let Some(value) = constant {
            if matches!(self.peek(), Some(',' | ')')) {
                return Ok(value);
            }
            self.at = start;
        }
        self.expr()
    }

    fn enum_names(&self, id: Id) -> Names<Id> {
        Names::new(
            self.catalog
                .enum_def(id)
                .map(|def| def.values.clone())
                .unwrap_or_default()
                .into_iter()
                .map(|(id, name)| (name, id)),
        )
    }

    fn arg(&mut self, param: &Param) -> Result<Arg> {
        Ok(match param.kind {
            ParamKind::Value | ParamKind::Array => Arg::Value(self.slot(param)?),
            ParamKind::Team => match self.field_name(|parser| &parser.teams) {
                Some(team) => Arg::Team(team),
                None => return self.error("expected a team"),
            },
            ParamKind::Hero => match self.field_name(|parser| &parser.heroes) {
                Some(hero) => Arg::Hero(hero),
                None => return self.error("expected a hero"),
            },
            ParamKind::Bool => {
                if self.eat_word("True") {
                    Arg::Bool(true)
                } else if self.eat_word("False") {
                    Arg::Bool(false)
                } else {
                    return self.error("expected True or False");
                }
            }
            ParamKind::Int => match self.number() {
                Some(number) => Arg::Int(number as i64),
                None => return self.error("expected an integer"),
            },
            ParamKind::Number => match self.number() {
                Some(number) => Arg::Number(number as f32),
                None => return self.error("expected a number"),
            },
            ParamKind::Flags => match self.number() {
                Some(number) => Arg::Flags(number as u64),
                None => return self.error("expected a number"),
            },
            ParamKind::Variable => {
                let name = self.identifier()?;
                Arg::Variable(self.variable_index(name, param.global)?)
            }
            ParamKind::Subroutine => {
                let name = self.identifier()?;
                Arg::Subroutine(self.subroutine_index(name)? as u8)
            }
            ParamKind::CustomString => Arg::CustomString(self.string()?),
            ParamKind::PredefinedString => {
                let text = self.string()?;
                match self.catalog.predefined_string(&text) {
                    Some(index) => Arg::PredefinedString(index),
                    None => return self.error(format!("no predefined string {text:?}")),
                }
            }
            ParamKind::Enum => {
                let names = self.enum_names(param.enum_id.unwrap_or(0));
                match self.name(&names) {
                    Some(id) => Arg::Enum(id),
                    None => return self.error("expected an enum value"),
                }
            }
            ParamKind::Compare => {
                let Some(op) = ["==", "!=", "<=", ">=", "<", ">"]
                    .iter()
                    .position(|op| self.eat(op))
                else {
                    return self.error("expected a comparison operator");
                };
                Arg::Compare(
                    [
                        Compare::Eq,
                        Compare::Ne,
                        Compare::Le,
                        Compare::Ge,
                        Compare::Lt,
                        Compare::Gt,
                    ][op],
                )
            }
            ParamKind::Map => match self.field_name(|parser| &parser.maps) {
                Some(map) => Arg::Map(map),
                None => return self.error("expected a map"),
            },
            ParamKind::GameMode => match self.field_name(|parser| &parser.game_modes) {
                Some(mode) => Arg::GameMode(mode),
                None => return self.error("expected a game mode"),
            },
            ParamKind::Button => match self.field_name(|parser| &parser.buttons) {
                Some(button) => Arg::Button(button),
                None => return self.error("expected a button"),
            },
            ParamKind::WaitBehavior => {
                let names = Names::new([
                    ("Ignore Condition".to_owned(), 0),
                    ("Abort When False".to_owned(), 1),
                    ("Restart When True".to_owned(), 2),
                ]);
                match self.name(&names) {
                    Some(behavior) => Arg::WaitBehavior(behavior),
                    None => return self.error("expected a wait behavior"),
                }
            }
            ParamKind::EventFilter => return self.error("an event filter in a value"),
        })
    }

    /// A variable by name: the declared names, else the default letter
    /// names (`A`…`Z`, `AA`…, the base-26 names of `sub_7FF789941650`).
    fn variable_index(&self, name: &str, global: bool) -> Result<u8> {
        let names = if global { &self.globals } else { &self.players };
        if let Some(index) = names.iter().position(|declared| declared == name) {
            return u8::try_from(index).or_else(|_| self.error("variable index over 255"));
        }
        match letter_index(name) {
            Some(index) if index < 128 => Ok(index as u8),
            _ => self.error(format!("unknown variable {name}")),
        }
    }

    fn subroutine_index(&self, name: &str) -> Result<i32> {
        if let Some(index) = self
            .subroutines
            .iter()
            .position(|declared| declared == name)
        {
            return Ok(index as i32);
        }
        match name
            .strip_prefix("Sub")
            .and_then(|digits| digits.parse().ok())
        {
            Some(index) => Ok(index),
            None => self.error(format!("unknown subroutine {name}")),
        }
    }
}

/// The index of a default variable name: `A` 0 … `Z` 25, `AA` 26 …
fn letter_index(name: &str) -> Option<usize> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_uppercase()) {
        return None;
    }
    let mut index = 0usize;
    for c in name.bytes() {
        index = index * 26 + usize::from(c - b'A') + 1;
    }
    Some(index - 1)
}

/// The `workshop` block's settings: `name: value`.
fn workshop_settings(settings: &[Setting]) -> Vec<WorkshopSetting> {
    let mut found = Vec::new();
    collect_workshop(settings, false, &mut found);
    found
}

fn collect_workshop(settings: &[Setting], inside: bool, found: &mut Vec<WorkshopSetting>) {
    for setting in settings {
        match setting {
            Setting::Block(name, block) => collect_workshop(
                block,
                inside || name.eq_ignore_ascii_case("workshop"),
                found,
            ),
            Setting::Pair(name, value) if inside => found.push(WorkshopSetting {
                name: name.clone(),
                value: value.clone(),
            }),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_variable_names_count_in_base_26() {
        assert_eq!(letter_index("A"), Some(0));
        assert_eq!(letter_index("Z"), Some(25));
        assert_eq!(letter_index("AA"), Some(26));
        assert_eq!(letter_index("DX"), Some(127));
        assert_eq!(letter_index("Ab"), None);
    }

    #[test]
    fn ow2_map_lines_are_told_apart() {
        assert_eq!(
            Setting::Line("Hanamura 972777519512068154".into()).ow2_map_id(),
            Some(("Hanamura", 972777519512068154))
        );
        assert_eq!(Setting::Line("Hanamura".into()).ow2_map_id(), None);
    }
}
