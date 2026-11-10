//! JSON values in file order, and Python's text for them.
//!
//! `tools/ssq.py` reads its inputs with Python's `json`, whose objects keep
//! their keys in file order, and prints values with `repr`, `str` and
//! `json.dumps`. The workspace's `serde_json` sorts object keys, so the
//! queries read their files into [`J`] instead, and the functions here write
//! numbers and strings as Python does.

use std::{fmt, io};

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

/// A Python int: wide enough for every 64-bit GUID and for the hex numbers a
/// query can name.
pub type Int = i128;

/// A JSON value as Python's `json` reads it.
#[derive(Clone, Debug)]
pub enum J {
    Null,
    Bool(bool),
    Int(Int),
    Float(f64),
    Str(String),
    Arr(Vec<J>),
    Obj(Obj),
}

/// A JSON object with its keys in file order (a Python `dict`).
#[derive(Clone, Debug, Default)]
pub struct Obj(Vec<(String, J)>);

impl Obj {
    /// The value of a key, including a JSON null.
    pub fn get(&self, key: &str) -> Option<&J> {
        self.0
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    /// `dict.get(key)` that is not None: a missing key and null are both None.
    pub fn val(&self, key: &str) -> Option<&J> {
        self.get(key).filter(|value| !value.is_null())
    }

    pub fn contains(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &J)> {
        self.0.iter().map(|(key, value)| (key.as_str(), value))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Moves a value out, leaving null behind.
    pub fn take(&mut self, key: &str) -> J {
        self.0
            .iter_mut()
            .find(|(name, _)| name == key)
            .map(|(_, value)| std::mem::replace(value, J::Null))
            .unwrap_or(J::Null)
    }

    pub fn into_entries(self) -> impl Iterator<Item = (String, J)> {
        self.0.into_iter()
    }
}

static NULL: J = J::Null;

impl J {
    pub fn is_null(&self) -> bool {
        matches!(self, J::Null)
    }

    pub fn as_obj(&self) -> Option<&Obj> {
        match self {
            J::Obj(obj) => Some(obj),
            _ => None,
        }
    }

    pub fn as_arr(&self) -> Option<&[J]> {
        match self {
            J::Arr(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            J::Str(text) => Some(text),
            _ => None,
        }
    }

    /// A number as Python's `int()` gives it (a bool is 0 or 1, a float is
    /// truncated); None for anything else.
    pub fn as_int(&self) -> Option<Int> {
        match self {
            J::Int(value) => Some(*value),
            J::Bool(value) => Some(Int::from(*value)),
            J::Float(value) if value.is_finite() => Some(value.trunc() as Int),
            _ => None,
        }
    }

    /// `value.get(key)` of a dict that is not None; None for a non-dict.
    pub fn val(&self, key: &str) -> Option<&J> {
        self.as_obj()?.val(key)
    }

    /// The value of a key, null when the key is missing or this is no dict.
    pub fn at(&self, key: &str) -> &J {
        self.as_obj().and_then(|obj| obj.get(key)).unwrap_or(&NULL)
    }

    /// The class of an object (`value.get("$")`).
    pub fn class(&self) -> Option<&str> {
        self.val("$")?.as_str()
    }

    /// The items of a list, none for anything else (`value or []`).
    pub fn items(&self) -> &[J] {
        self.as_arr().unwrap_or(&[])
    }

    /// Python's truth value.
    pub fn truthy(&self) -> bool {
        match self {
            J::Null => false,
            J::Bool(value) => *value,
            J::Int(value) => *value != 0,
            J::Float(value) => *value != 0.0,
            J::Str(text) => !text.is_empty(),
            J::Arr(items) => !items.is_empty(),
            J::Obj(obj) => obj.len() > 0,
        }
    }
}

/// Reads a JSON document in file order.
pub fn parse(data: &[u8]) -> io::Result<J> {
    let mut decoder = serde_json::Deserializer::from_slice(data);
    let value = J::deserialize(&mut decoder).map_err(invalid)?;
    decoder.end().map_err(invalid)?;
    Ok(value)
}

pub fn invalid(error: impl fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

impl<'de> Deserialize<'de> for J {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        decoder.deserialize_any(JVisitor)
    }
}

struct JVisitor;

impl<'de> Visitor<'de> for JVisitor {
    type Value = J;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<J, E> {
        Ok(J::Bool(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<J, E> {
        Ok(J::Int(Int::from(value)))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<J, E> {
        Ok(J::Int(Int::from(value)))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<J, E> {
        Ok(J::Float(value))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<J, E> {
        Ok(J::Str(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<J, E> {
        Ok(J::Str(value))
    }

    fn visit_unit<E: de::Error>(self) -> Result<J, E> {
        Ok(J::Null)
    }

    fn visit_none<E: de::Error>(self) -> Result<J, E> {
        Ok(J::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<J, A::Error> {
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(J::Arr(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<J, A::Error> {
        let mut entries: Vec<(String, J)> = Vec::with_capacity(map.size_hint().unwrap_or(0));
        while let Some((key, value)) = map.next_entry::<String, J>()? {
            // A repeated key keeps its first place and its last value, as in a dict.
            match entries.iter_mut().find(|(name, _)| *name == key) {
                Some(entry) => entry.1 = value,
                None => entries.push((key, value)),
            }
        }
        Ok(J::Obj(Obj(entries)))
    }
}

/// Python's `repr()` of a value.
pub fn repr(value: &J) -> String {
    match value {
        J::Null => "None".into(),
        J::Bool(true) => "True".into(),
        J::Bool(false) => "False".into(),
        J::Int(value) => value.to_string(),
        J::Float(value) => float_repr(*value),
        J::Str(text) => str_repr(text),
        J::Arr(items) => format!(
            "[{}]",
            items.iter().map(repr).collect::<Vec<_>>().join(", ")
        ),
        J::Obj(obj) => format!(
            "{{{}}}",
            obj.iter()
                .map(|(key, value)| format!("{}: {}", str_repr(key), repr(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Python's `str()` of a value: text as it is, anything else as `repr()`.
pub fn py_str(value: &J) -> String {
    match value {
        J::Str(text) => text.clone(),
        other => repr(other),
    }
}

/// Python's `str()` of an optional value (None for a missing one).
pub fn opt_str(value: Option<&J>) -> String {
    value.map_or_else(|| "None".into(), py_str)
}

/// Python's `repr()` of a str.
pub fn str_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(text.len() + 2);
    out.push(quote);
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            ch if ch == quote => {
                out.push('\\');
                out.push(ch);
            }
            ch if ch.is_control() => {
                let code = u32::from(ch);
                if code < 0x100 {
                    out.push_str(&format!("\\x{code:02x}"));
                } else if code < 0x10000 {
                    out.push_str(&format!("\\u{code:04x}"));
                } else {
                    out.push_str(&format!("\\U{code:08x}"));
                }
            }
            ch => out.push(ch),
        }
    }
    out.push(quote);
    out
}

/// The shortest decimal digits of a float and its decimal exponent
/// (`1.25e-5` is `("125", -5)`).
fn shortest_digits(value: f64) -> (String, i32) {
    let text = format!("{:e}", value.abs());
    let (mantissa, exponent) = text.split_once('e').expect("exponent form");
    let digits: String = mantissa.chars().filter(|ch| *ch != '.').collect();
    (digits, exponent.parse().expect("exponent"))
}

/// Python's `repr()` of a float.
pub fn float_repr(value: f64) -> String {
    if value.is_nan() {
        return "nan".into();
    }
    if value.is_infinite() {
        return if value > 0.0 { "inf" } else { "-inf" }.into();
    }
    let sign = if value.is_sign_negative() { "-" } else { "" };
    let (digits, exponent) = shortest_digits(value);
    let body = if (-4..16).contains(&exponent) {
        fixed(&digits, exponent, true)
    } else {
        scientific(&digits, exponent)
    };
    format!("{sign}{body}")
}

/// Digits with the point placed by the exponent; `point_zero` keeps `.0` on
/// a whole number.
fn fixed(digits: &str, exponent: i32, point_zero: bool) -> String {
    if exponent < 0 {
        let zeros = "0".repeat((-exponent - 1) as usize);
        return format!("0.{zeros}{digits}");
    }
    let whole = exponent as usize + 1;
    if digits.len() <= whole {
        let zeros = "0".repeat(whole - digits.len());
        return if point_zero {
            format!("{digits}{zeros}.0")
        } else {
            format!("{digits}{zeros}")
        };
    }
    format!("{}.{}", &digits[..whole], &digits[whole..])
}

fn scientific(digits: &str, exponent: i32) -> String {
    let mantissa = if digits.len() > 1 {
        format!("{}.{}", &digits[..1], &digits[1..])
    } else {
        digits.to_owned()
    };
    let sign = if exponent < 0 { '-' } else { '+' };
    format!("{mantissa}e{sign}{:02}", exponent.abs())
}

/// Python's `f"{value:.{precision}g}"`.
pub fn format_g(value: f64, precision: usize) -> String {
    if !value.is_finite() {
        return float_repr(value);
    }
    let precision = precision.max(1);
    let sign = if value.is_sign_negative() { "-" } else { "" };
    let text = format!("{:.*e}", precision - 1, value.abs());
    let (mantissa, exponent) = text.split_once('e').expect("exponent form");
    let exponent: i32 = exponent.parse().expect("exponent");
    let digits: String = mantissa.chars().filter(|ch| *ch != '.').collect();
    let digits = digits.trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    let body = if exponent >= -4 && exponent < precision as i32 {
        fixed(digits, exponent, false)
    } else {
        scientific(digits, exponent)
    };
    format!("{sign}{body}")
}

/// A float rounded to float32, as `expr.f32` does.
pub fn f32_round(value: f64) -> f64 {
    f64::from(value as f32)
}

/// `json.dumps(value, ensure_ascii=False)`.
pub fn dumps(value: &J) -> String {
    match value {
        J::Null => "null".into(),
        J::Bool(value) => value.to_string(),
        J::Int(value) => value.to_string(),
        J::Float(value) if value.is_nan() => "NaN".into(),
        J::Float(value) if value.is_infinite() => if *value > 0.0 {
            "Infinity"
        } else {
            "-Infinity"
        }
        .into(),
        J::Float(value) => float_repr(*value),
        J::Str(text) => json_string(text),
        J::Arr(items) => format!(
            "[{}]",
            items.iter().map(dumps).collect::<Vec<_>>().join(", ")
        ),
        J::Obj(obj) => format!(
            "{{{}}}",
            obj.iter()
                .map(|(key, value)| format!("{}: {}", json_string(key), dumps(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ch if u32::from(ch) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(ch))),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_are_written_as_python_writes_them() {
        assert_eq!(float_repr(1.0), "1.0");
        assert_eq!(float_repr(0.5), "0.5");
        assert_eq!(float_repr(-0.0), "-0.0");
        assert_eq!(float_repr(0.30000001192092896), "0.30000001192092896");
        assert_eq!(float_repr(1e-5), "1e-05");
        assert_eq!(float_repr(0.0001), "0.0001");
        assert_eq!(float_repr(1.5e16), "1.5e+16");
        assert_eq!(float_repr(123456789.0), "123456789.0");
        assert_eq!(format_g(0.30000001192092896, 1), "0.3");
        assert_eq!(format_g(1234.5, 2), "1.2e+03");
        assert_eq!(format_g(0.000012345, 3), "1.23e-05");
        assert_eq!(format_g(100.0, 3), "100");
    }

    #[test]
    fn text_is_quoted_as_python_quotes_it() {
        assert_eq!(str_repr("abc"), "'abc'");
        assert_eq!(str_repr("it's"), "\"it's\"");
        assert_eq!(str_repr("a\nb"), "'a\\nb'");
    }

    #[test]
    fn objects_keep_their_key_order() {
        let value = parse(br#"{"b": 1, "a": [true, null, 2.0]}"#).unwrap();
        let keys: Vec<_> = value.as_obj().unwrap().iter().map(|(key, _)| key).collect();
        assert_eq!(keys, ["b", "a"]);
        assert_eq!(repr(value.at("a")), "[True, None, 2.0]");
        assert_eq!(dumps(&value), r#"{"b": 1, "a": [true, null, 2.0]}"#);
    }
}
