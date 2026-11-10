//! Names and values as text ("names and text" in `tools/ssq.py`, and
//! `expr.source` of `ow174/game/script/expr.py`).

use super::{
    data::{ENTITY, Names, guid},
    json::{self, Int, J, f32_round, format_g},
};

pub const IDENT_TYPE: Int = 0x0D80;
pub const MODE_TYPE: Int = 0x0230;
/// Characters of one field value in `show`.
pub const CUT: usize = 160;
const CONSTANT_TYPES: &[(&str, &str)] = &[
    ("STUConfigVarInt", "int"),
    ("STUConfigVarFloat", "float"),
    ("STUConfigVarBool", "bool"),
];
/// Resource config vars and the field that holds their resource
/// (`expr.RESOURCES`).
const RESOURCES: &[(&str, &str)] = &[
    ("STUConfigVarIdentifier", "m_identifier"),
    ("STUConfigVarGameMessage", "m_gameMessage"),
    ("STUConfigVarEffect", "m_effect"),
    ("STUConfigVarLoadout", "m_loadout"),
    ("STUConfigVarHardPoint", "m_hardPoint"),
    ("STUConfigVarGameMode", "m_gamemode"),
    ("STU_BCD1C634", "m_id"),
    ("STU_8A491388", "m_priority"),
];
/// Config vars `expr.source` writes in its own way (a variable, `owner()`,
/// `ruleset()`, `valueOr()`, ...).
const SOURCE_CLASSES: &[&str] = &[
    "STUConfigVarDynamic",
    "STU_B5A0CAF0",
    "STU_3832D36C",
    "STU_91EFD5B1",
    "STU_3ACE35FB",
    "STU_7197C080",
];
const EXPRESSIONS: &[&str] = &["STU_919FD47C", "STUConfigVarExpression"];
/// Bounds the decompiler on a malformed jump, which would loop forever.
const MAX_DECOMPILE_STEPS: usize = 100_000;
/// Operand bytes per opcode (`expr.OPERANDS`).
pub fn operands(op: Int) -> Int {
    match op {
        1..=9 | 11..=14 | 19..=21 => 1,
        10 => 2,
        _ => 0,
    }
}

pub fn constant_type(cls: &str) -> Option<&'static str> {
    CONSTANT_TYPES
        .iter()
        .find(|(name, _)| *name == cls)
        .map(|(_, kind)| *kind)
}

fn resource_field(cls: &str) -> Option<&'static str> {
    RESOURCES
        .iter()
        .find(|(name, _)| *name == cls)
        .map(|(_, field)| *field)
}

/// The short name of a class: its alias, else `STUStatescript*` without the
/// prefix, else the class.
pub fn alias<'a>(names: &'a Names, cls: &'a str) -> &'a str {
    if let Some(alias) = names.class(cls).and_then(|info| info.alias.as_deref()) {
        return alias;
    }
    match cls.strip_prefix("STUStatescript") {
        Some(rest) if !rest.is_empty() => rest,
        _ => cls,
    }
}

pub fn class_hash<'a>(names: &'a Names, cls: &'a str) -> Option<&'a str> {
    if let Some(hash) = cls.strip_prefix("STU_") {
        return Some(hash);
    }
    names.class(cls).and_then(|info| info.hash.as_deref())
}

/// The asset type of a GUID (`0x01B`, `0x01C`, ...): its 12 type bits
/// reversed, plus one.
pub fn guid_type(value: Int) -> Int {
    Int::from(((value >> 48) as u16 & 0xFFF).reverse_bits() >> 4) + 1
}

pub fn guid_text(value: Int) -> String {
    format!("{:04X}.{:03X}", value & 0xFFFF_FFFF_FFFF, guid_type(value))
}

/// The 12 type bits of an asset type (`0x01B` -> `0x580`).
pub fn type_bits(kind: Int) -> Int {
    Int::from(((kind - 1) as u16 & 0xFFF).reverse_bits() >> 4)
}

pub fn var_text(var: Int, scope: Option<Int>) -> String {
    format!("#{var}{}", if scope == Some(ENTITY) { "e" } else { "" })
}

pub fn var_name(names: &Names, var: Int, scope: Option<Int>) -> String {
    match names.var(var) {
        Some(entry) => format!("{} {}", var_text(var, scope), entry.name),
        None => var_text(var, scope),
    }
}

pub fn scope_text(scope: Option<Int>) -> &'static str {
    if scope == Some(ENTITY) {
        "entity"
    } else {
        "instance"
    }
}

pub fn mode_name(names: &Names, guid: Int) -> String {
    names
        .mode(guid)
        .and_then(|mode| mode.name.clone())
        .unwrap_or_else(|| guid_text(guid))
}

pub fn button_text(names: &Names, value: Option<&J>) -> String {
    let name = match value {
        Some(J::Int(number)) => names.buttons.get(number),
        _ => None,
    };
    let text = json::opt_str(value);
    match name {
        Some(name) => format!("{text} {name}"),
        None => text,
    }
}

pub fn plural(count: usize, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

pub fn cut(text: &str, size: usize) -> String {
    if text.chars().count() <= size {
        return text.to_owned();
    }
    let kept: String = text.chars().take(size - 3).collect();
    format!("{kept}...")
}

/// A field value as text: variables and expressions as the VM writes them
/// (`expr.source`), config vars as `Alias(field=value)`, other objects as
/// `Alias{field=value}`, GUIDs as `index.type`.
pub fn render(names: &Names, value: &J, depth: usize) -> String {
    let obj = match value {
        J::Arr(items) => {
            let parts: Vec<_> = items
                .iter()
                .map(|item| render(names, item, depth + 1))
                .collect();
            return format!("[{}]", parts.join(", "));
        }
        J::Str(_) if guid(value).is_some() => return guid_text(guid(value).expect("a GUID")),
        J::Obj(obj) => obj,
        other => return json::repr(other),
    };
    let Some(cls) = value.class() else {
        return match obj.get("node") {
            Some(node) => format!("node {}", json::py_str(node)),
            None => "editor box".into(),
        };
    };
    if cls == "STUConfigVarLogicalButton" {
        return format!("button({})", button_text(names, obj.val("m_logicalButton")));
    }
    if constant_type(cls).is_some() {
        let number = obj.val("m_value").filter(|number| number.truthy());
        if cls == "STUConfigVarBool" {
            return if number.is_some() { "true" } else { "false" }.into();
        }
        return number.map_or_else(|| "0".into(), json::repr);
    }
    if let Some(field) = resource_field(cls) {
        let name = match cls.strip_prefix("STUConfigVar") {
            Some(_) => cls.replace("STUConfigVar", ""),
            None => alias(names, cls).to_owned(),
        };
        let mut parts = vec![
            obj.val(field)
                .and_then(guid)
                .map_or_else(|| "none".into(), guid_text),
        ];
        parts.extend(
            obj.iter()
                .filter(|(key, item)| {
                    !matches!(*key, "$" | "m_EE729DCB" | "m_resourceKey")
                        && *key != field
                        && !item.is_null()
                })
                .map(|(key, item)| format!("{key}={}", render(names, item, depth + 1))),
        );
        return format!("{name}({})", parts.join(", "));
    }
    if depth > 8 {
        return format!("{}(...)", alias(names, cls));
    }
    let expression = obj.val("m_expression").and_then(J::as_obj).is_some();
    if SOURCE_CLASSES.contains(&cls) || expression {
        return short_floats(&source(names, value, depth, expression));
    }
    let parts: Vec<_> = obj
        .iter()
        .filter(|(key, item)| !matches!(*key, "$" | "links" | "m_EE729DCB") && !item.is_null())
        .map(|(key, item)| format!("{key}={}", render(names, item, depth + 1)))
        .collect();
    if obj.contains("m_EE729DCB") {
        // A config var: evaluated, so written as a call.
        return format!("{}({})", alias(names, cls), parts.join(", "));
    }
    format!("{}{{{}}}", alias(names, cls), parts.join(", "))
}

/// `expr.source` of a config var whose dicts and lists `render` wrote first
/// (`render` passes it a copy in which they are text `expr.source` prints as
/// it is).
fn source(names: &Names, value: &J, depth: usize, expression: bool) -> String {
    // An argument of the config var as `expr.source` prints it.
    let argument = |key: &str| match value.at(key) {
        J::Null => "null".to_owned(),
        item @ (J::Obj(_) | J::Arr(_)) => render(names, item, depth + 1),
        item => json::repr(item),
    };
    let cls = value.class().unwrap_or("?");
    match cls {
        "STUConfigVarDynamic" => return dynamic_text(value),
        "STU_B5A0CAF0" => return "owner()".into(),
        "STU_3832D36C" => return format!("ruleset({})", argument("m_value")),
        "STU_91EFD5B1" => {
            return format!(
                "valueOr({}, {})",
                argument("m_value"),
                argument("m_default")
            );
        }
        "STU_3ACE35FB" => return format!("wouldStackToTop({})", argument("m_E50CF556")),
        "STU_7197C080" => {
            return format!(
                "cooldown({}, {}, {})",
                argument("m_01AFADB6"),
                argument("m_EA70ACC2"),
                argument("m_69A20070")
            );
        }
        _ => {}
    }
    if !EXPRESSIONS.contains(&cls) {
        return format!("{cls}()");
    }
    let data = value.at("m_expression");
    let code: Vec<&J> = data.at("m_opcodes").items().iter().collect();
    let dynamics = data.at("m_dynamicVars").items();
    let floats = data.at("m_D99EF254").items();
    let configs: Vec<String> = if expression {
        value
            .at("m_configVars")
            .items()
            .iter()
            .map(|item| render(names, item, depth + 1))
            .collect()
    } else {
        Vec::new()
    };
    let numbers: Option<Vec<Int>> = code.iter().map(|op| op.as_int()).collect();
    let decompiled = numbers.and_then(|code| {
        Decompiler {
            code: &code,
            dynamics,
            floats,
            configs: &configs,
            steps: 0,
        }
        .run(0, Vec::new(), None)
        .map(|(text, _, _)| text)
    });
    decompiled.unwrap_or_else(|| {
        let parts: Vec<_> = code.iter().map(|op| json::repr(op)).collect();
        format!("expr[{}]", parts.join(", "))
    })
}

/// `#53` (instance variable) or `#31e` (entity variable); `?` for anything
/// that is not a `STUConfigVarDynamic` (`expr._var_text`).
fn dynamic_text(cfg: &J) -> String {
    match dynamic(cfg) {
        Some((scope, var)) => var_text(var, Some(scope)),
        None => "?".into(),
    }
}

/// (scope, variable) of a `STUConfigVarDynamic` (`expr.dynamic`).
pub fn dynamic(cfg: &J) -> Option<(Int, Int)> {
    if cfg.class() != Some("STUConfigVarDynamic") {
        return None;
    }
    let scope = if cfg.at("m_60DB8F99").truthy() {
        ENTITY
    } else {
        super::data::INSTANCE
    };
    let var = cfg
        .val("m_identifier")
        .filter(|value| value.truthy())
        .and_then(J::as_int)
        .unwrap_or(0);
    Some((scope, var))
}

#[derive(PartialEq)]
enum How {
    End,
    Jump,
    Stop,
}

const BINARY: &[(Int, &str)] = &[
    (17, "[]"),
    (22, "++"),
    (23, "+"),
    (24, "-"),
    (25, "*"),
    (26, "/"),
    (27, "%"),
    (33, "<="),
    (34, "<"),
    (35, ">="),
    (36, ">"),
    (37, "=="),
    (38, "!="),
];
const FUNCTIONS: &[(Int, &str)] = &[
    (18, "count"),
    (28, "pow"),
    (29, "-"),
    (30, "!"),
    (39, "sqrt"),
    (40, "rad"),
    (41, "deg"),
    (42, "sinr"),
    (43, "cosr"),
    (44, "tanr"),
    (45, "atan2r"),
    (46, "sin"),
    (47, "cos"),
    (48, "tan"),
    (49, "atan2"),
    (50, "abs"),
    (51, "mag"),
    (52, "magxz"),
    (53, "dist"),
    (54, "distxz"),
    (55, "dot"),
    (56, "cross"),
    (57, "norm"),
    (58, "vec3"),
    (60, "x"),
    (61, "y"),
    (62, "z"),
    (63, "min"),
    (64, "max"),
    (65, "clamp"),
    (66, "floor"),
    (67, "ceil"),
    (68, "round"),
    (69, "random"),
    (70, "randomInt"),
    (71, "lerp"),
];

fn arity(op: Int) -> Int {
    match op {
        28 | 45 | 49 | 53 | 54 | 55 | 56 | 63 | 64 | 69 | 70 => 2,
        58 | 65 | 71 => 3,
        _ => 1,
    }
}

fn lookup(table: &[(Int, &'static str)], op: Int) -> Option<&'static str> {
    table
        .iter()
        .find(|(code, _)| *code == op)
        .map(|(_, text)| *text)
}

/// A list index as Python takes it (a negative one counts from the end).
fn py_index<T>(items: &[T], index: Int) -> Option<&T> {
    let len = items.len() as Int;
    let index = if index < 0 { index + len } else { index };
    usize::try_from(index)
        .ok()
        .and_then(|index| items.get(index))
}

/// Where `stack[len(stack) - count:]` starts.
fn tail_start(len: usize, count: Int) -> usize {
    let len = len as Int;
    let start = len - count;
    let start = if start < 0 {
        (start + len).max(0)
    } else {
        start
    };
    start.min(len) as usize
}

/// `expr._decompile`. None where Python fails with an IndexError, a
/// KeyError or a TypeError (the caller then prints the opcodes).
struct Decompiler<'a> {
    code: &'a [Int],
    dynamics: &'a [J],
    floats: &'a [J],
    configs: &'a [String],
    steps: usize,
}

impl Decompiler<'_> {
    fn run(
        &mut self,
        mut pc: Int,
        mut stack: Vec<String>,
        stop: Option<Int>,
    ) -> Option<(String, Int, How)> {
        let len = self.code.len() as Int;
        let top = |stack: &[String]| stack.last().cloned().unwrap_or_else(|| "?".into());
        while pc < len && Some(pc) != stop {
            // A malformed jump could loop forever; Python would never finish.
            self.steps += 1;
            if self.steps > MAX_DECOMPILE_STEPS {
                return None;
            }
            let op = *py_index(self.code, pc)?;
            let arg = if pc + 1 < len {
                *py_index(self.code, pc + 1)?
            } else {
                0
            };
            match op {
                0 => return Some((top(&stack), pc, How::End)),
                1 => return Some((top(&stack), pc + arg, How::Jump)),
                2 | 3 => {
                    let left = stack.pop()?;
                    let (right, _, _) = self.run(pc + 2, Vec::new(), Some(pc + arg))?;
                    let symbol = if op == 2 { "&&" } else { "||" };
                    stack.push(format!("({left} {symbol} {right})"));
                    pc += arg;
                    continue;
                }
                4 | 5 => {
                    let mut cond = stack.pop()?;
                    if op == 5 {
                        cond = format!("!{cond}");
                    }
                    let (then, merge, how) = self.run(pc + 2, Vec::new(), None)?;
                    if how != How::Jump {
                        // The true branch ends the expression: so does the false one.
                        let (other, end, how) = self.run(pc + arg, Vec::new(), None)?;
                        return Some((format!("({cond} ? {then} : {other})"), end, how));
                    }
                    let (other, _, _) = self.run(pc + arg, Vec::new(), Some(merge))?;
                    stack.push(format!("({cond} ? {then} : {other})"));
                    pc = merge;
                    continue;
                }
                6 => {
                    let value = match py_index(self.floats, arg)? {
                        J::Int(value) => *value as f64,
                        J::Float(value) => *value,
                        J::Bool(value) => f64::from(u8::from(*value)),
                        _ => return None,
                    };
                    stack.push(json::float_repr(f32_round(value)));
                }
                7 | 13 => {
                    let dynamic = py_index(self.dynamics, arg)?.as_obj()?;
                    let id = json::opt_str(dynamic.val("m_identifier"));
                    stack.push(if op == 7 {
                        format!("id({id})")
                    } else {
                        format!("param({id})")
                    });
                }
                8 | 9 => stack.push(dynamic_text(py_index(self.dynamics, arg)?)),
                10 => {
                    let number = *py_index(self.code, pc + 2)?;
                    let dynamic = dynamic_text(py_index(self.dynamics, number)?);
                    stack.push(format!("parent{arg}.{dynamic}"));
                }
                14 => stack.push(py_index(self.configs, arg)?.clone()),
                15 | 16 => {
                    let key = stack.pop()?;
                    let object = stack.pop()?;
                    stack.push(format!("{object}.{key}"));
                }
                17 => {
                    let index = stack.pop()?;
                    let object = stack.pop()?;
                    stack.push(format!("{object}[{index}]"));
                }
                19 | 20 => {
                    let count = arg * if op == 19 { 2 } else { 1 };
                    let items = if count != 0 {
                        stack.split_off(tail_start(stack.len(), count))
                    } else {
                        Vec::new()
                    };
                    let name = if op == 19 { "map" } else { "array" };
                    stack.push(format!("{name}({})", items.join(", ")));
                }
                21 => stack.push(format!("parentEntity{arg}")),
                31 | 32 => {
                    let value = stack.pop()?;
                    let name = if op == 32 { "globalDeref" } else { "deref" };
                    stack.push(format!("{name}({value})"));
                }
                op if lookup(BINARY, op).is_some() => {
                    let b = stack.pop()?;
                    let a = stack.pop()?;
                    stack.push(format!("({a} {} {b})", lookup(BINARY, op).expect("binary")));
                }
                op if lookup(FUNCTIONS, op).is_some() => {
                    let args = stack.split_off(tail_start(stack.len(), arity(op)));
                    let name = lookup(FUNCTIONS, op).expect("function");
                    stack.push(if op == 29 || op == 30 {
                        format!("{name}{}", args.first()?)
                    } else {
                        format!("{name}({})", args.join(", "))
                    });
                }
                op => stack.push(format!("op{op}")),
            }
            pc += 1 + operands(op);
        }
        Some((top(&stack), pc, How::Stop))
    }
}

/// int / float / bool / a resource kind for a constant config var; None for
/// a variable, an expression or anything computed.
pub fn value_type(names: &Names, cfg: &J) -> Option<String> {
    let cls = cfg.as_obj().and_then(|_| cfg.class())?;
    if let Some(kind) = constant_type(cls) {
        return Some(kind.into());
    }
    if resource_field(cls).is_some()
        || cfg
            .as_obj()
            .is_some_and(|obj| obj.contains("m_resourceKey"))
    {
        return Some(match cls.strip_prefix("STUConfigVar") {
            Some(_) => cls.replace("STUConfigVar", ""),
            None => alias(names, cls).to_owned(),
        });
    }
    None
}

/// What kind of value a field holds, for the field lists of `class`.
pub fn label(names: &Names, value: &J) -> String {
    match value {
        J::Obj(obj) => match value.class() {
            Some(cls) => alias(names, cls).to_owned(),
            None if obj.contains("$") => json::py_str(obj.get("$").expect("class")),
            None if obj.contains("node") => "node link".into(),
            None => "editor box".into(),
        },
        J::Arr(items) => {
            let mut kinds: Vec<String> = items
                .iter()
                .filter(|item| !item.is_null())
                .map(|item| label(names, item))
                .collect();
            kinds.sort();
            kinds.dedup();
            if kinds.is_empty() {
                "[]".into()
            } else {
                format!("[{}]", kinds.join(", "))
            }
        }
        J::Str(_) => match guid(value) {
            Some(number) => format!("GUID .{:03X}", guid_type(number)),
            None => "text".into(),
        },
        J::Null => "NoneType".into(),
        J::Bool(_) => "bool".into(),
        J::Int(_) => "int".into(),
        J::Float(_) => "float".into(),
    }
}

/// `expr.source` prints float32 constants in full (0.30000001192092896):
/// print the shortest decimal that is the same float32 (0.3). Python finds
/// them with `(?<![\w.])-?\d+\.\d{7,}(?:e[-+]?\d+)?`.
pub fn short_floats(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while at < chars.len() {
        let free = at == 0 || {
            let before = chars[at - 1];
            !(before.is_alphanumeric() || before == '_' || before == '.')
        };
        if let Some(end) = free.then(|| long_float(&chars, at)).flatten() {
            let found: String = chars[at..end].iter().collect();
            out.push_str(&shortest(&found));
            at = end;
        } else {
            out.push(chars[at]);
            at += 1;
        }
    }
    out
}

/// The end of a long float starting at `at`.
fn long_float(chars: &[char], at: usize) -> Option<usize> {
    let digits = |from: usize| {
        chars[from..]
            .iter()
            .take_while(|ch| ch.is_ascii_digit())
            .count()
    };
    let mut end = at;
    if chars.get(end) == Some(&'-') {
        end += 1;
    }
    let whole = digits(end);
    if whole == 0 || chars.get(end + whole) != Some(&'.') {
        return None;
    }
    end += whole + 1;
    let fraction = digits(end);
    if fraction < 7 {
        return None;
    }
    end += fraction;
    if chars.get(end) == Some(&'e') {
        let mut exponent = end + 1;
        if matches!(chars.get(exponent), Some('-' | '+')) {
            exponent += 1;
        }
        let count = digits(exponent);
        if count > 0 {
            end = exponent + count;
        }
    }
    Some(end)
}

fn shortest(text: &str) -> String {
    let Ok(value) = text.parse::<f64>() else {
        return text.to_owned();
    };
    for digits in 1..18 {
        let short = format_g(value, digits);
        if short
            .parse::<f64>()
            .is_ok_and(|number| f32_round(number) == f32_round(value))
        {
            return short;
        }
    }
    text.to_owned()
}
