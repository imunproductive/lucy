//! A node's configs worked out once ([`ConfigTable`]): the evaluator runs
//! these instead of reading each config's class and fields by name on
//! every evaluation.

use serde_json::Value as Json;

use super::{
    EmptyContext, EvalError, Evaluator, Expression, MAX_EXPRESSION_STEPS, Value, Variable, byte,
    class, dynamic, identifier, jump, malformed, number, operand_count, resource_field, rounded,
    stack_var, table,
};

/// A config in the form the evaluator runs. Only configs whose evaluation
/// [`Evaluator::known_config`] would give the same way every time are
/// compiled; any other is read from its JSON as before.
#[derive(Debug)]
pub(super) enum Config {
    /// A config that reads no context (a literal, an asset): its value.
    Value(Value),
    /// `STUConfigVarDynamic`: a variable.
    Dynamic(Variable),
    /// `STUConfigVarExpression` (`STU_919FD47C`): its bytecode.
    Expression(Program),
}

/// The configs of one node's fields by their address in those fields
/// (the fields of a loaded graph don't change, so each config keeps its
/// address while the node lives), sorted by address.
#[derive(Debug, Default)]
pub(crate) struct ConfigTable {
    by_address: Vec<(usize, Config)>,
}

impl ConfigTable {
    /// Compile every config in `fields` (not `fields` itself).
    pub(crate) fn of(fields: &Json) -> Self {
        let mut by_address = Vec::new();
        collect(fields, &mut by_address);
        by_address.sort_unstable_by_key(|&(address, _)| address);
        Self { by_address }
    }

    pub(super) fn get(&self, config: &Json) -> Option<&Config> {
        let address = address(config);
        self.by_address
            .binary_search_by_key(&address, |&(address, _)| address)
            .ok()
            .map(|index| &self.by_address[index].1)
    }
}

fn address(config: &Json) -> usize {
    std::ptr::from_ref(config) as usize
}

fn collect(value: &Json, out: &mut Vec<(usize, Config)>) {
    let mut visit = |child: &Json| {
        if let Some(config) = compile(child) {
            out.push((address(child), config));
        }
        collect(child, out);
    };
    match value {
        Json::Object(fields) => fields.values().for_each(&mut visit),
        Json::Array(items) => items.iter().for_each(&mut visit),
        _ => (),
    }
}

/// The classes [`Evaluator::known_config`] answers from the config alone
/// (each arm reads only the config's own fields).
const LITERALS: &[&str] = &[
    "STUConfigVarBool",
    "STUConfigVarInt",
    "STUConfigVarFloat",
    "STUConfigVarLogicalButton",
    "STUConfigVarVec3",
    "STUConfigVarAnimCategory",
    "STU_92CA44CC",
    "STU_B70476F3",
    "STU_0E766B33",
    "STU_20DB8AB5",
    "STU_ABB6C23E",
    "STU_038C32E8",
];

fn compile(config: &Json) -> Option<Config> {
    let class = class(config)?;
    match class {
        "STUConfigVarDynamic" => dynamic(config).ok().flatten().map(Config::Dynamic),
        "STU_919FD47C" | "STUConfigVarExpression" => {
            Program::compile(config).ok().map(Config::Expression)
        }
        // STU_92CA44CC and the anim category are literals only in their
        // null form; otherwise they reach the context's natives, and with
        // the empty context they fail here and stay uncompiled.
        class if LITERALS.contains(&class) || resource_field(class).is_some() => {
            Evaluator::new(MAX_EXPRESSION_STEPS, None)
                .known_config(config, &mut EmptyContext, 0)
                .ok()
                .map(Config::Value)
        }
        _ => None,
    }
}

/// An expression's bytecode decoded at every offset, its table operands
/// read; whatever can't be decoded fails only when it runs, with the error
/// reading the JSON would give there.
#[derive(Debug)]
pub(super) struct Program {
    pub(super) steps: Box<[Result<Step, EvalError>]>,
}

#[derive(Debug)]
pub(super) struct Step {
    pub(super) opcode: u8,
    pub(super) args: [u8; 2],
    /// The offset after its operands.
    pub(super) next: usize,
    pub(super) operand: Operand,
}

/// What a step reads of the expression's tables, or the branch target.
#[derive(Debug)]
pub(super) enum Operand {
    None,
    /// Opcodes 1 to 5.
    Jump(Result<usize, EvalError>),
    /// A float constant (6) or an identifier (7).
    Value(Result<Value, EvalError>),
    /// A dynamic variable (8 to 10).
    Variable(Result<Variable, EvalError>),
    /// A parameter's identifier (13).
    Parameter(Result<u32, EvalError>),
}

impl Program {
    pub(super) fn compile(config: &Json) -> Result<Self, EvalError> {
        let Expression {
            code,
            dynamics,
            floats,
        } = Expression::read(config)?;
        Ok(Self {
            steps: (0..code.len())
                .map(|pc| decode(code, dynamics, floats, pc))
                .collect(),
        })
    }
}

fn decode(code: &[Json], dynamics: &[Json], floats: &[Json], pc: usize) -> Result<Step, EvalError> {
    let opcode = byte(&code[pc], pc)?;
    let next = pc + 1 + operand_count(opcode);
    let operands = code
        .get(pc + 1..next)
        .ok_or_else(|| malformed(pc, "truncated opcode"))?;
    let mut args = [0; 2];
    for (index, (arg, value)) in args.iter_mut().zip(operands).enumerate() {
        *arg = byte(value, pc + 1 + index)?;
    }
    let arg = args[0];
    let operand = match opcode {
        1..=5 => Operand::Jump(jump(pc, arg, code.len())),
        6 => Operand::Value(table(floats, arg, pc).and_then(|value| {
            if value.is_null() {
                return Err(malformed(pc, "expected a float constant"));
            }
            Ok(Value::Float(rounded(number(value)?)))
        })),
        7 => Operand::Value(
            table(dynamics, arg, pc)
                .and_then(identifier)
                .map(|id| Value::Integer(i64::from(id))),
        ),
        8 | 9 => Operand::Variable(stack_var(dynamics, arg, pc)),
        10 => Operand::Variable(stack_var(dynamics, args[1], pc)),
        13 => Operand::Parameter(table(dynamics, arg, pc).and_then(identifier)),
        _ => Operand::None,
    };
    Ok(Step {
        opcode,
        args,
        next,
        operand,
    })
}

impl Step {
    pub(super) fn jump(&self) -> Result<usize, EvalError> {
        match &self.operand {
            Operand::Jump(target) => target.clone(),
            _ => unreachable!("a branch decodes its target"),
        }
    }

    pub(super) fn value(&self) -> Result<Value, EvalError> {
        match &self.operand {
            Operand::Value(value) => value.clone(),
            _ => unreachable!("a constant decodes its value"),
        }
    }

    pub(super) fn variable(&self) -> Result<Variable, EvalError> {
        match &self.operand {
            Operand::Variable(variable) => variable.clone(),
            _ => unreachable!("a variable read decodes its variable"),
        }
    }

    pub(super) fn parameter(&self) -> Result<u32, EvalError> {
        match &self.operand {
            Operand::Parameter(id) => id.clone(),
            _ => unreachable!("a parameter read decodes its identifier"),
        }
    }
}
