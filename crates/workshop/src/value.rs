//! What Workshop values evaluate to, and the conversions between them.
//!
//! Number, vector and boolean conversions follow the Workshop's documented
//! behaviour (the catalog's value descriptions, `054/0162`; OverPy's and
//! the community wiki's notes where the client's data says nothing). The
//! ones the data doesn't settle are marked as assumptions.

use std::fmt;

use crate::catalog::{Catalog, Id};

/// An entity: a player (its controller, Workshop.md §2), a dummy bot or an
/// entity an action created.
pub type EntityId = u32;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// Workshop numbers are kept as `f64`; the logic stores literals as
    /// `f32` [assumption: retail's arithmetic precision is not traced].
    Number(f64),
    Vector([f64; 3]),
    Entity(EntityId),
    /// 0 All Teams, 1 Team 1, 2 Team 2.
    Team(u8),
    Hero(u64),
    /// A Button enum value (`4A39`).
    Button(Id),
    Color(Color),
    /// Any other enum constant (icons, effects, statuses…).
    Enum(Id),
    /// A map header (`09F`) index.
    Map(u32),
    GameMode(u64),
    Text(Text),
    Array(Vec<Value>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Color {
    /// A Color enum value (`50A6`).
    Named(Id),
    /// Custom Color: red, green, blue, alpha (0-255).
    Rgba([f64; 4]),
}

/// A string value. The parts each client renders itself (input bindings,
/// icons) stay structured; the presenter graphs take them
/// (`STU_2566340D`, Workshop.md §8 question 4).
#[derive(Clone, Debug, PartialEq)]
pub enum Text {
    /// Custom String: the template with `{0}`-`{2}` and its values.
    Custom {
        template: String,
        args: Vec<Value>,
    },
    /// String: a predefined string by index and its values.
    Predefined {
        index: u16,
        args: Vec<Value>,
    },
    InputBinding(Id),
    AbilityIcon {
        hero: u64,
        button: Id,
    },
    Icon(Id),
    HeroIcon(u64),
}

impl Value {
    /// Whether the value counts as true: a non-zero number or vector, any
    /// entity, a non-empty array or string [assumption for arrays and
    /// strings].
    pub fn truthy(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Bool(value) => *value,
            Self::Number(value) => *value != 0.0,
            Self::Vector(vector) => vector.iter().any(|&c| c != 0.0),
            Self::Array(items) => !items.is_empty(),
            Self::Text(_) => true,
            _ => true,
        }
    }

    /// The value as a number: booleans are 0 or 1, everything that isn't
    /// a number is 0 [assumption].
    pub fn number(&self) -> f64 {
        match self {
            Self::Number(value) => *value,
            Self::Bool(value) => f64::from(u8::from(*value)),
            _ => 0.0,
        }
    }

    pub fn array(&self) -> Vec<Value> {
        match self {
            Self::Array(items) => items.clone(),
            Self::Null => Vec::new(),
            other => vec![other.clone()],
        }
    }

    pub fn entity(&self) -> Option<EntityId> {
        match self {
            Self::Entity(entity) => Some(*entity),
            Self::Array(items) => items.first()?.entity(),
            _ => None,
        }
    }

    /// The entities of a value: one, or every entity of an array.
    pub fn entities(&self) -> Vec<EntityId> {
        match self {
            Self::Entity(entity) => vec![*entity],
            Self::Array(items) => items.iter().filter_map(Value::entity).collect(),
            _ => Vec::new(),
        }
    }

    /// Workshop equality: numbers, booleans and Null compare as numbers;
    /// vectors by component; arrays element by element; the rest by value.
    pub fn equals(&self, other: &Value) -> bool {
        let numeric = |value: &Value| matches!(value, Self::Null | Self::Bool(_) | Self::Number(_));
        match (self, other) {
            (a, b) if numeric(a) && numeric(b) => a.number() == b.number(),
            (Self::Array(a), Self::Array(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.equals(b))
            }
            (Self::Text(a), Self::Text(b)) => a == b,
            (a, b) => a == b,
        }
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Self::Number(value)
    }
}

impl Text {
    /// The text as the server can render it: custom and predefined strings
    /// with their values substituted; the per-client parts as placeholders
    /// (`<button>`, `<icon>`).
    pub fn render(&self, catalog: &Catalog) -> String {
        match self {
            Self::Custom { template, args } => substitute(template, args, catalog),
            Self::Predefined { index, args } => substitute(
                catalog
                    .strings
                    .get(usize::from(*index))
                    .map_or("", String::as_str),
                args,
                catalog,
            ),
            Self::InputBinding(button) => {
                format!("<{}>", catalog.enum_value_name(*button).unwrap_or("button"))
            }
            Self::AbilityIcon { .. } | Self::Icon(_) | Self::HeroIcon(_) => "<icon>".into(),
        }
    }
}

/// `{0}`, `{1}`, `{2}` replaced by the values; `{{` and `}}` are braces
/// (the Custom String description, `054/0162`).
fn substitute(template: &str, args: &[Value], catalog: &Catalog) -> String {
    let mut out = String::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '{' => {
                let digits: String = chars.by_ref().take_while(|&c| c != '}').collect();
                match digits
                    .parse::<usize>()
                    .ok()
                    .and_then(|index| args.get(index))
                {
                    Some(value) => out.push_str(&display(value, catalog)),
                    None => out.push_str(""),
                }
            }
            c => out.push(c),
        }
    }
    out
}

/// A value as text: numbers with at most two decimals [assumption: the
/// client's formatting is not traced], vectors as `(x, y, z)`.
pub fn display(value: &Value, catalog: &Catalog) -> String {
    match value {
        Value::Null => "0".into(),
        Value::Bool(value) => if *value { "True" } else { "False" }.into(),
        Value::Number(number) => number_text(*number),
        Value::Vector([x, y, z]) => format!(
            "({}, {}, {})",
            number_text(*x),
            number_text(*y),
            number_text(*z)
        ),
        Value::Entity(entity) => format!("Entity {entity}"),
        Value::Team(0) => "All Teams".into(),
        Value::Team(team) => format!("Team {team}"),
        Value::Hero(hero) => catalog.hero_name(*hero).unwrap_or("Hero").into(),
        Value::Button(id) | Value::Enum(id) | Value::Color(Color::Named(id)) => {
            catalog.enum_value_name(*id).unwrap_or("").into()
        }
        Value::Color(Color::Rgba(_)) => "Color".into(),
        Value::Map(map) => catalog.map_name(*map).unwrap_or("Map").into(),
        Value::GameMode(mode) => catalog.game_mode_name(*mode).unwrap_or("Game Mode").into(),
        Value::Text(text) => text.render(catalog),
        Value::Array(items) => items
            .iter()
            .map(|item| display(item, catalog))
            .collect::<Vec<_>>()
            .join(", "),
    }
}

fn number_text(number: f64) -> String {
    let rounded = (number * 100.0).round() / 100.0;
    if rounded.fract() == 0.0 {
        format!("{rounded:.0}")
    } else {
        let text = format!("{rounded:.2}");
        text.trim_end_matches('0').to_owned()
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Named(id) => write!(f, "Color({id:04X})"),
            Self::Rgba([r, g, b, a]) => write!(f, "Custom Color({r}, {g}, {b}, {a})"),
        }
    }
}
