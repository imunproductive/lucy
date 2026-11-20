//! Read expressions: what a hook or the frame sampler reads at each hit.
//!
//! `rcx`, `[rcx+0x18]:u32`, `[[rdi+8]+0x10]:f32x3`, `c4+0x160`. A name is a
//! register or one of the sampler's bases (`c59`, `c4`, `ent`, `base`);
//! `[x]` reads the pointer at `x`. A `:type` suffix reads that type at the
//! address of a top-level `[x]`, or reinterprets the value of anything else;
//! `xN` reads N values in a row. Without a suffix the value is a `u64`.

use serde_json::Value;

/// A value type of a read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    F32,
    F64,
}

impl Kind {
    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "u8" => Self::U8,
            "u16" => Self::U16,
            "u32" => Self::U32,
            "u64" | "ptr" => Self::U64,
            "i8" => Self::I8,
            "i16" => Self::I16,
            "i32" => Self::I32,
            "i64" => Self::I64,
            "f32" => Self::F32,
            "f64" => Self::F64,
            _ => return None,
        })
    }

    pub fn size(self) -> usize {
        match self {
            Self::U8 | Self::I8 => 1,
            Self::U16 | Self::I16 => 2,
            Self::U32 | Self::I32 | Self::F32 => 4,
            Self::U64 | Self::I64 | Self::F64 => 8,
        }
    }

    /// The value of the first `size()` bytes as JSON.
    pub fn json(self, bytes: &[u8]) -> Value {
        let mut raw = [0u8; 8];
        raw[..self.size()].copy_from_slice(&bytes[..self.size()]);
        let word = u64::from_le_bytes(raw);
        match self {
            Self::U8 | Self::U16 | Self::U32 | Self::U64 => word.into(),
            Self::I8 => (word as u8 as i8).into(),
            Self::I16 => (word as u16 as i16).into(),
            Self::I32 => (word as u32 as i32).into(),
            Self::I64 => (word as i64).into(),
            Self::F32 => finite(f64::from(f32::from_bits(word as u32))),
            Self::F64 => finite(f64::from_bits(word)),
        }
    }
}

/// JSON has no NaN or infinity: those become strings.
fn finite(value: f64) -> Value {
    if value.is_finite() {
        value.into()
    } else {
        format!("{value}").into()
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Node {
    Number(u64),
    Name(String),
    Deref(Box<Node>),
    Sum(Box<Node>, Box<Node>),
    Difference(Box<Node>, Box<Node>),
}

/// A parsed read expression.
#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    pub text: String,
    node: Node,
    kind: Option<Kind>,
    count: usize,
}

impl Expr {
    pub fn parse(text: &str) -> Result<Self, String> {
        let trimmed = text.trim();
        let (body, suffix) = match trimmed.rfind(':') {
            Some(at) if !trimmed[at..].contains(']') => (&trimmed[..at], Some(&trimmed[at + 1..])),
            _ => (trimmed, None),
        };
        let (kind, count) = match suffix {
            None => (None, 1),
            Some(suffix) => {
                let (name, count) = match suffix.split_once('x') {
                    Some((name, count)) => (
                        name,
                        count
                            .parse::<usize>()
                            .ok()
                            .filter(|&n| (1..=64).contains(&n))
                            .ok_or_else(|| format!("{text}: bad count {count:?}"))?,
                    ),
                    None => (suffix, 1),
                };
                let kind = Kind::parse(name.trim())
                    .ok_or_else(|| format!("{text}: unknown type {name:?}"))?;
                (Some(kind), count)
            }
        };
        let tokens = tokenize(body)?;
        let mut parser = Parser { tokens, at: 0 };
        let node = parser.sum()?;
        if parser.at != parser.tokens.len() {
            return Err(format!("{text}: unexpected {:?}", parser.tokens[parser.at]));
        }
        Ok(Self {
            text: trimmed.to_owned(),
            node,
            kind,
            count,
        })
    }

    /// The expression's value: `name` resolves names, `read` reads memory
    /// (`None` when unreadable). An unreadable read gives `null`.
    pub fn eval(
        &self,
        name: &dyn Fn(&str) -> Option<u64>,
        read: &dyn Fn(u64, &mut [u8]) -> bool,
    ) -> Value {
        let at = |node: &Node| value(node, name, read);
        match (&self.node, self.kind) {
            (Node::Deref(inner), Some(kind)) => {
                let Some(address) = at(inner) else {
                    return Value::Null;
                };
                let mut bytes = vec![0u8; kind.size() * self.count];
                if !read(address, &mut bytes) {
                    return Value::Null;
                }
                let values: Vec<Value> = bytes
                    .chunks_exact(kind.size())
                    .map(|chunk| kind.json(chunk))
                    .collect();
                if self.count == 1 {
                    values.into_iter().next().unwrap_or(Value::Null)
                } else {
                    values.into()
                }
            }
            (node, kind) => match at(node) {
                Some(word) => kind.unwrap_or(Kind::U64).json(&word.to_le_bytes()),
                None => Value::Null,
            },
        }
    }

    /// The address the expression reads at: the value of a top-level `[x]`'s
    /// `x`, else the value itself.
    pub fn address(
        &self,
        name: &dyn Fn(&str) -> Option<u64>,
        read: &dyn Fn(u64, &mut [u8]) -> bool,
    ) -> Option<u64> {
        match &self.node {
            Node::Deref(inner) if self.kind.is_some() => value(inner, name, read),
            node => value(node, name, read),
        }
    }
}

fn value(
    node: &Node,
    name: &dyn Fn(&str) -> Option<u64>,
    read: &dyn Fn(u64, &mut [u8]) -> bool,
) -> Option<u64> {
    match node {
        Node::Number(number) => Some(*number),
        Node::Name(text) => name(text),
        Node::Deref(inner) => {
            let address = value(inner, name, read)?;
            let mut bytes = [0u8; 8];
            read(address, &mut bytes).then(|| u64::from_le_bytes(bytes))
        }
        Node::Sum(a, b) => Some(value(a, name, read)?.wrapping_add(value(b, name, read)?)),
        Node::Difference(a, b) => Some(value(a, name, read)?.wrapping_sub(value(b, name, read)?)),
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Number(u64),
    Name(String),
    Open,
    Close,
    OpenParen,
    CloseParen,
    Plus,
    Minus,
}

fn tokenize(text: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            ' ' | '\t' => {
                chars.next();
            }
            '[' | ']' | '(' | ')' | '+' | '-' => {
                chars.next();
                tokens.push(match c {
                    '[' => Token::Open,
                    ']' => Token::Close,
                    '(' => Token::OpenParen,
                    ')' => Token::CloseParen,
                    '+' => Token::Plus,
                    _ => Token::Minus,
                });
            }
            c if c.is_ascii_alphanumeric() || c == '_' => {
                let mut word = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_ascii_alphanumeric() || c == '_' {
                        word.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                tokens.push(number(&word).map_or(Token::Name(word.to_lowercase()), Token::Number));
            }
            other => return Err(format!("{text}: unexpected {other:?}")),
        }
    }
    Ok(tokens)
}

/// `0x1F`, `1Fh` or decimal `31`.
fn number(word: &str) -> Option<u64> {
    if let Some(hex) = word.strip_prefix("0x").or_else(|| word.strip_prefix("0X")) {
        return u64::from_str_radix(hex, 16).ok();
    }
    if let Some(hex) = word.strip_suffix(['h', 'H']) {
        if hex.starts_with(|c: char| c.is_ascii_digit()) {
            return u64::from_str_radix(hex, 16).ok();
        }
    }
    if word.starts_with(|c: char| c.is_ascii_digit()) {
        return word.parse().ok();
    }
    None
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
}

impl Parser {
    fn sum(&mut self) -> Result<Node, String> {
        let mut node = self.atom()?;
        loop {
            match self.tokens.get(self.at) {
                Some(Token::Plus) => {
                    self.at += 1;
                    node = Node::Sum(Box::new(node), Box::new(self.atom()?));
                }
                Some(Token::Minus) => {
                    self.at += 1;
                    node = Node::Difference(Box::new(node), Box::new(self.atom()?));
                }
                _ => return Ok(node),
            }
        }
    }

    fn atom(&mut self) -> Result<Node, String> {
        let token = self.tokens.get(self.at).cloned();
        self.at += 1;
        match token {
            Some(Token::Number(number)) => Ok(Node::Number(number)),
            Some(Token::Name(name)) => Ok(Node::Name(name)),
            Some(Token::Open) => {
                let inner = self.sum()?;
                self.expect(Token::Close)?;
                Ok(Node::Deref(Box::new(inner)))
            }
            Some(Token::OpenParen) => {
                let inner = self.sum()?;
                self.expect(Token::CloseParen)?;
                Ok(inner)
            }
            other => Err(format!("expected a value, got {other:?}")),
        }
    }

    fn expect(&mut self, token: Token) -> Result<(), String> {
        if self.tokens.get(self.at) == Some(&token) {
            self.at += 1;
            Ok(())
        } else {
            Err(format!("expected {token:?}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn memory(address: u64, out: &mut [u8]) -> bool {
        // 0x1000: a pointer to 0x2000; 0x2000..: the floats 1.5, -2.0, 3.25.
        let mut image = vec![0u8; 0x100];
        image[..8].copy_from_slice(&0x2000u64.to_le_bytes());
        let mut floats = vec![0u8; 0x100];
        for (index, value) in [1.5f32, -2.0, 3.25].iter().enumerate() {
            floats[0x10 + index * 4..0x14 + index * 4].copy_from_slice(&value.to_le_bytes());
        }
        let (area, start) = match address {
            0x1000..0x1100 => (&image, 0x1000),
            0x2000..0x2100 => (&floats, 0x2000),
            _ => return false,
        };
        let offset = (address - start) as usize;
        let Some(bytes) = area.get(offset..offset + out.len()) else {
            return false;
        };
        out.copy_from_slice(bytes);
        true
    }

    fn eval(text: &str) -> Value {
        let names = |name: &str| (name == "rcx").then_some(0x1000);
        Expr::parse(text).unwrap().eval(&names, &memory)
    }

    #[test]
    fn reads_registers_pointers_and_typed_vectors() {
        assert_eq!(eval("rcx"), json!(0x1000));
        assert_eq!(eval("rcx+0x10 - 8"), json!(0x1008));
        assert_eq!(eval("[rcx]"), json!(0x2000));
        assert_eq!(eval("[[rcx]+0x10]:f32"), json!(1.5));
        assert_eq!(eval("[[rcx]+0x10]:f32x3"), json!([1.5, -2.0, 3.25]));
        assert_eq!(eval("[[rcx] + 14h]:i32"), json!(-0x4000_0000i32));
        assert_eq!(eval("rcx:u8"), json!(0));
    }

    #[test]
    fn unreadable_memory_and_unknown_names_are_null() {
        assert_eq!(eval("[rcx+0x200]:u32"), Value::Null);
        assert_eq!(eval("rdx"), Value::Null);
        assert!(Expr::parse("[rcx").is_err());
        assert!(Expr::parse("rcx:f16").is_err());
    }
}
