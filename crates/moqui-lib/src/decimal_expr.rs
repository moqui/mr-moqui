/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! The decimal expression language of payroll formulas (IronStone People design PE-09; docs/dev/JS-Framework-Services.md
//! "Expressions"): a declared grammar over named decimal inputs with an allowlist of functions, computed in exact
//! decimal. The sandbox holds by construction: an expression is parsed into the tree below, which has numbers, input
//! names, operators and the listed functions and nothing else, and is evaluated over a map of decimals. There is no
//! script engine, context, entity, service or I/O behind it to reach.
//!
//! ```text
//! expression := or
//! or         := and ("or" and)*
//! and        := not ("and" not)*
//! not        := "not" not | comparison
//! comparison := additive (("<" | "<=" | ">" | ">=" | "==" | "!=") additive)?
//! additive   := term (("+" | "-") term)*
//! term       := unary (("*" | "/") unary)*
//! unary      := "-" unary | primary
//! primary    := number | name | function "(" expression ("," expression)* ")" | "(" expression ")"
//! number     := digits ("." digits)?
//! name       := [A-Za-z_][A-Za-z0-9_]*
//! function   := if | min | max | abs | round | floor | ceil
//! ```
//!
//! Two types: an amount (a decimal) and a condition. Arithmetic and comparisons take amounts; `and`, `or`, `not` and
//! the first argument of `if` take conditions; the whole expression is an amount. `+`, `-` and `*` are exact, `/` keeps
//! the full precision of the decimal type, and nothing is rounded but by `round`, `floor` and `ceil`.

use std::collections::{BTreeMap, BTreeSet};

use rust_decimal::{Decimal, RoundingStrategy};

use crate::object::{decimal_add, decimal_multiply, decimal_subtract};

/// The longest expression, in characters.
pub const MAX_LENGTH: usize = 2000;
/// The deepest nesting of parentheses, calls, `-` and `not`, so no input can exhaust the stack.
pub const MAX_DEPTH: usize = 32;
/// The most significant digits of a number, the decimal type's precision.
const MAX_DIGITS: usize = 28;
/// The most decimal places `round`, `floor` and `ceil` take.
const MAX_PLACES: u32 = 28;

/// Why an expression was refused: a reason code callers compare, a plain message and, where it applies, the 1-based
/// character position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprError {
    pub code: &'static str,
    pub message: String,
    pub position: Option<usize>,
}

impl ExprError {
    fn at(code: &'static str, message: impl Into<String>, position: usize) -> Self {
        Self {
            code,
            message: message.into(),
            position: Some(position),
        }
    }

    fn plain(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            position: None,
        }
    }
}

impl std::fmt::Display for ExprError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.position {
            Some(position) => write!(formatter, "{} ({} at {position})", self.message, self.code),
            None => write!(formatter, "{} ({})", self.message, self.code),
        }
    }
}

/// The allowlist of functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Function {
    If,
    Min,
    Max,
    Abs,
    Round,
    Floor,
    Ceil,
}

impl Function {
    fn named(name: &str) -> Option<Self> {
        Some(match name {
            "if" => Self::If,
            "min" => Self::Min,
            "max" => Self::Max,
            "abs" => Self::Abs,
            "round" => Self::Round,
            "floor" => Self::Floor,
            "ceil" => Self::Ceil,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Self::If => "if",
            Self::Min => "min",
            Self::Max => "max",
            Self::Abs => "abs",
            Self::Round => "round",
            Self::Floor => "floor",
            Self::Ceil => "ceil",
        }
    }

    /// The arguments it takes: (fewest, most).
    fn arity(self) -> (usize, usize) {
        match self {
            Self::If => (3, 3),
            Self::Min | Self::Max => (2, usize::MAX),
            Self::Abs => (1, 1),
            Self::Round | Self::Floor | Self::Ceil => (2, 2),
        }
    }

    fn rounding(self) -> Option<RoundingStrategy> {
        match self {
            Self::Round => Some(RoundingStrategy::MidpointAwayFromZero),
            Self::Floor => Some(RoundingStrategy::ToNegativeInfinity),
            Self::Ceil => Some(RoundingStrategy::ToPositiveInfinity),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Add,
    Subtract,
    Multiply,
    Divide,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Compare {
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
    Equal,
    NotEqual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Type {
    Amount,
    Condition,
}

/// A parsed expression. A run of one precedence level (`a + b - c`, `p and q and r`) is one chain, so evaluation
/// loops rather than recursing once per operator.
#[derive(Debug)]
enum Node {
    Number(Decimal),
    Name {
        name: String,
        position: usize,
    },
    Negate {
        operand: Box<Node>,
        position: usize,
    },
    Not {
        operand: Box<Node>,
        position: usize,
    },
    Chain {
        first: Box<Node>,
        rest: Vec<(Op, usize, Node)>,
    },
    Compare {
        compare: Compare,
        left: Box<Node>,
        right: Box<Node>,
        position: usize,
    },
    Call {
        function: Function,
        arguments: Vec<Node>,
        position: usize,
    },
}

impl Node {
    fn position(&self) -> usize {
        match self {
            Self::Number(_) => 0,
            Self::Name { position, .. }
            | Self::Negate { position, .. }
            | Self::Not { position, .. }
            | Self::Compare { position, .. }
            | Self::Call { position, .. } => *position,
            Self::Chain { first, .. } => first.position(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(Decimal),
    Name(String),
    Symbol(&'static str),
    End,
}

/// A token and its 1-based character position.
type Positioned = (Token, usize);

fn tokenize(text: &str) -> Result<Vec<Positioned>, ExprError> {
    let chars: Vec<(usize, char)> = text.chars().enumerate().map(|(index, c)| (index + 1, c)).collect();
    let mut tokens = Vec::new();
    let mut index = 0;
    while let Some(&(position, c)) = chars.get(index) {
        if c.is_whitespace() {
            index += 1;
        } else if c.is_ascii_digit() {
            let start = index;
            while chars.get(index).is_some_and(|(_, c)| c.is_ascii_digit()) {
                index += 1;
            }
            if chars.get(index).is_some_and(|(_, c)| *c == '.') {
                index += 1;
                let fraction = index;
                while chars.get(index).is_some_and(|(_, c)| c.is_ascii_digit()) {
                    index += 1;
                }
                if index == fraction {
                    return Err(ExprError::at(
                        "NUMBER_INVALID",
                        "A number has digits after its point",
                        position,
                    ));
                }
            }
            if chars
                .get(index)
                .is_some_and(|(_, c)| c.is_alphanumeric() || *c == '_' || *c == '.')
            {
                return Err(ExprError::at(
                    "NUMBER_INVALID",
                    "A number is digits with an optional point and more digits",
                    position,
                ));
            }
            let literal: String = chars[start..index].iter().map(|(_, c)| *c).collect();
            tokens.push((Token::Number(number(&literal, position)?), position));
        } else if c.is_ascii_alphabetic() || c == '_' {
            let start = index;
            while chars
                .get(index)
                .is_some_and(|(_, c)| c.is_ascii_alphanumeric() || *c == '_')
            {
                index += 1;
            }
            tokens.push((
                Token::Name(chars[start..index].iter().map(|(_, c)| *c).collect()),
                position,
            ));
        } else {
            let next = chars.get(index + 1).map(|(_, c)| *c);
            let (symbol, width) = match (c, next) {
                ('<', Some('=')) => ("<=", 2),
                ('>', Some('=')) => (">=", 2),
                ('=', Some('=')) => ("==", 2),
                ('!', Some('=')) => ("!=", 2),
                ('<', _) => ("<", 1),
                ('>', _) => (">", 1),
                ('+', _) => ("+", 1),
                ('-', _) => ("-", 1),
                ('*', _) => ("*", 1),
                ('/', _) => ("/", 1),
                ('(', _) => ("(", 1),
                (')', _) => (")", 1),
                (',', _) => (",", 1),
                ('.', Some(digit)) if digit.is_ascii_digit() => {
                    return Err(ExprError::at(
                        "NUMBER_INVALID",
                        "A number starts with a digit",
                        position,
                    ));
                }
                ('"' | '\'' | '`', _) => {
                    return Err(ExprError::at("SYNTAX_ERROR", "Text is not part of a formula", position));
                }
                _ => {
                    return Err(ExprError::at(
                        "SYNTAX_ERROR",
                        format!("\"{c}\" is not part of a formula"),
                        position,
                    ));
                }
            };
            tokens.push((Token::Symbol(symbol), position));
            index += width;
        }
    }
    tokens.push((Token::End, chars.len() + 1));
    Ok(tokens)
}

fn number(literal: &str, position: usize) -> Result<Decimal, ExprError> {
    let digits = literal
        .chars()
        .filter(char::is_ascii_digit)
        .skip_while(|c| *c == '0')
        .count();
    if digits > MAX_DIGITS {
        return Err(ExprError::at(
            "NUMBER_INVALID",
            format!("A number has at most {MAX_DIGITS} digits"),
            position,
        ));
    }
    Decimal::from_str_exact(literal)
        .map_err(|_| ExprError::at("NUMBER_INVALID", format!("{literal} is not a number"), position))
}

const RESERVED: [&str; 3] = ["and", "or", "not"];

struct Parser {
    tokens: Vec<Positioned>,
    at: usize,
    depth: usize,
}

impl Parser {
    fn peek(&self) -> &Positioned {
        // tokenize always ends with Token::End, which is never consumed
        &self.tokens[self.at.min(self.tokens.len() - 1)]
    }

    fn advance(&mut self) -> Positioned {
        let token = self.peek().clone();
        if token.0 != Token::End {
            self.at += 1;
        }
        token
    }

    fn is_symbol(&self, symbol: &str) -> bool {
        matches!(&self.peek().0, Token::Symbol(found) if *found == symbol)
    }

    fn is_word(&self, word: &str) -> bool {
        matches!(&self.peek().0, Token::Name(found) if found == word)
    }

    fn enter(&mut self, position: usize) -> Result<(), ExprError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(ExprError::at(
                "EXPRESSION_TOO_DEEP",
                format!("A formula nests at most {MAX_DEPTH} levels"),
                position,
            ));
        }
        Ok(())
    }

    fn unexpected(&self) -> ExprError {
        let (token, position) = self.peek();
        let message = match token {
            Token::End => "The formula ends too soon".to_string(),
            Token::Number(value) => format!("{value} is not expected here"),
            Token::Name(name) => format!("\"{name}\" is not expected here"),
            Token::Symbol(symbol) => format!("\"{symbol}\" is not expected here"),
        };
        ExprError::at("SYNTAX_ERROR", message, *position)
    }

    fn expect_symbol(&mut self, symbol: &str) -> Result<(), ExprError> {
        if !self.is_symbol(symbol) {
            return Err(self.unexpected());
        }
        self.advance();
        Ok(())
    }

    fn expression(&mut self) -> Result<Node, ExprError> {
        let position = self.peek().1;
        self.enter(position)?;
        let node = self.chain(Level::Or)?;
        self.depth -= 1;
        Ok(node)
    }

    fn chain(&mut self, level: Level) -> Result<Node, ExprError> {
        let first = self.operand(level)?;
        let mut rest = Vec::new();
        loop {
            let (token, position) = self.peek().clone();
            let op = match (level, &token) {
                (Level::Or, Token::Name(word)) if word == "or" => Op::Or,
                (Level::And, Token::Name(word)) if word == "and" => Op::And,
                (Level::Additive, Token::Symbol("+")) => Op::Add,
                (Level::Additive, Token::Symbol("-")) => Op::Subtract,
                (Level::Term, Token::Symbol("*")) => Op::Multiply,
                (Level::Term, Token::Symbol("/")) => Op::Divide,
                _ => break,
            };
            self.advance();
            rest.push((op, position, self.operand(level)?));
        }
        Ok(if rest.is_empty() {
            first
        } else {
            Node::Chain {
                first: Box::new(first),
                rest,
            }
        })
    }

    fn operand(&mut self, level: Level) -> Result<Node, ExprError> {
        match level {
            Level::Or => self.chain(Level::And),
            Level::And => self.negation(),
            Level::Additive => self.chain(Level::Term),
            Level::Term => self.unary(),
        }
    }

    fn negation(&mut self) -> Result<Node, ExprError> {
        if self.is_word("not") {
            let (_, position) = self.advance();
            self.enter(position)?;
            let operand = self.negation()?;
            self.depth -= 1;
            return Ok(Node::Not {
                operand: Box::new(operand),
                position,
            });
        }
        self.comparison()
    }

    fn comparison(&mut self) -> Result<Node, ExprError> {
        let left = self.chain(Level::Additive)?;
        let (token, position) = self.peek().clone();
        let compare = match token {
            Token::Symbol("<") => Compare::Less,
            Token::Symbol("<=") => Compare::LessOrEqual,
            Token::Symbol(">") => Compare::Greater,
            Token::Symbol(">=") => Compare::GreaterOrEqual,
            Token::Symbol("==") => Compare::Equal,
            Token::Symbol("!=") => Compare::NotEqual,
            _ => return Ok(left),
        };
        self.advance();
        let right = self.chain(Level::Additive)?;
        Ok(Node::Compare {
            compare,
            left: Box::new(left),
            right: Box::new(right),
            position,
        })
    }

    fn unary(&mut self) -> Result<Node, ExprError> {
        if self.is_symbol("-") {
            let (_, position) = self.advance();
            self.enter(position)?;
            let operand = self.unary()?;
            self.depth -= 1;
            return Ok(Node::Negate {
                operand: Box::new(operand),
                position,
            });
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Node, ExprError> {
        let (token, position) = self.peek().clone();
        match token {
            Token::Number(value) => {
                self.advance();
                Ok(Node::Number(value))
            }
            Token::Symbol("(") => {
                self.advance();
                let inner = self.expression()?;
                self.expect_symbol(")")?;
                Ok(inner)
            }
            Token::Name(name) if RESERVED.contains(&name.as_str()) => Err(self.unexpected()),
            Token::Name(name) => {
                self.advance();
                if !self.is_symbol("(") {
                    return Ok(Node::Name { name, position });
                }
                let Some(function) = Function::named(&name) else {
                    return Err(ExprError::at(
                        "UNKNOWN_FUNCTION",
                        format!("\"{name}\" is not a formula function"),
                        position,
                    ));
                };
                self.advance();
                let mut arguments = vec![self.expression()?];
                while self.is_symbol(",") {
                    self.advance();
                    arguments.push(self.expression()?);
                }
                self.expect_symbol(")")?;
                let (fewest, most) = function.arity();
                if arguments.len() < fewest || arguments.len() > most {
                    let wanted = match (fewest, most) {
                        (fewest, most) if fewest == most => format!("{fewest}"),
                        (fewest, _) => format!("{fewest} or more"),
                    };
                    return Err(ExprError::at(
                        "ARGUMENT_COUNT",
                        format!("{} takes {wanted} arguments", function.name()),
                        position,
                    ));
                }
                Ok(Node::Call {
                    function,
                    arguments,
                    position,
                })
            }
            _ => Err(self.unexpected()),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Level {
    Or,
    And,
    Additive,
    Term,
}

/// A parsed and type-checked expression.
#[derive(Debug)]
pub struct Expression {
    root: Node,
    names: Vec<String>,
    functions: Vec<String>,
}

/// Parse `text`. With `allowed_names`, a name outside it is refused (`UNKNOWN_NAME`).
pub fn parse(text: &str, allowed_names: Option<&[String]>) -> Result<Expression, ExprError> {
    if text.trim().is_empty() {
        return Err(ExprError::plain("EXPRESSION_REQUIRED", "Enter a formula"));
    }
    if text.chars().count() > MAX_LENGTH {
        return Err(ExprError::plain(
            "EXPRESSION_TOO_LONG",
            format!("A formula has at most {MAX_LENGTH} characters"),
        ));
    }
    let mut parser = Parser {
        tokens: tokenize(text)?,
        at: 0,
        depth: 0,
    };
    let root = parser.expression()?;
    if parser.peek().0 != Token::End {
        return Err(parser.unexpected());
    }
    let mut names = BTreeSet::new();
    let mut functions = BTreeSet::new();
    collect(&root, &mut names, &mut functions, allowed_names)?;
    if type_of(&root)? != Type::Amount {
        return Err(ExprError::at(
            "TYPE_MISMATCH",
            "A formula gives an amount, not a condition",
            root.position().max(1),
        ));
    }
    Ok(Expression {
        root,
        names: names.into_iter().collect(),
        functions: functions.into_iter().map(str::to_string).collect(),
    })
}

fn collect(
    node: &Node,
    names: &mut BTreeSet<String>,
    functions: &mut BTreeSet<&'static str>,
    allowed_names: Option<&[String]>,
) -> Result<(), ExprError> {
    match node {
        Node::Number(_) => {}
        Node::Name { name, position } => {
            if allowed_names.is_some_and(|allowed| !allowed.iter().any(|known| known == name)) {
                return Err(ExprError::at(
                    "UNKNOWN_NAME",
                    format!("\"{name}\" is not an input of this formula"),
                    *position,
                ));
            }
            names.insert(name.clone());
        }
        Node::Negate { operand, .. } | Node::Not { operand, .. } => {
            collect(operand, names, functions, allowed_names)?;
        }
        Node::Chain { first, rest } => {
            collect(first, names, functions, allowed_names)?;
            for (_, _, operand) in rest {
                collect(operand, names, functions, allowed_names)?;
            }
        }
        Node::Compare { left, right, .. } => {
            collect(left, names, functions, allowed_names)?;
            collect(right, names, functions, allowed_names)?;
        }
        Node::Call {
            function, arguments, ..
        } => {
            functions.insert(function.name());
            for argument in arguments {
                collect(argument, names, functions, allowed_names)?;
            }
        }
    }
    Ok(())
}

fn mismatch(wanted: Type, position: usize) -> ExprError {
    let message = match wanted {
        Type::Amount => "An amount is needed here, not a condition",
        Type::Condition => "A condition is needed here, such as a comparison, not an amount",
    };
    ExprError::at("TYPE_MISMATCH", message, position.max(1))
}

fn require(node: &Node, wanted: Type, position: usize) -> Result<(), ExprError> {
    if type_of(node)? != wanted {
        let at = match node.position() {
            0 => position,
            own => own,
        };
        return Err(mismatch(wanted, at));
    }
    Ok(())
}

fn type_of(node: &Node) -> Result<Type, ExprError> {
    Ok(match node {
        Node::Number(_) | Node::Name { .. } => Type::Amount,
        Node::Negate { operand, position } => {
            require(operand, Type::Amount, *position)?;
            Type::Amount
        }
        Node::Not { operand, position } => {
            require(operand, Type::Condition, *position)?;
            Type::Condition
        }
        Node::Chain { first, rest } => {
            let wanted = match rest.first() {
                Some((Op::And | Op::Or, _, _)) => Type::Condition,
                _ => Type::Amount,
            };
            let first_position = rest.first().map_or(1, |(_, position, _)| *position);
            require(first, wanted, first_position)?;
            for (_, position, operand) in rest {
                require(operand, wanted, *position)?;
            }
            wanted
        }
        Node::Compare {
            left, right, position, ..
        } => {
            require(left, Type::Amount, *position)?;
            require(right, Type::Amount, *position)?;
            Type::Condition
        }
        Node::Call {
            function,
            arguments,
            position,
        } => {
            for (index, argument) in arguments.iter().enumerate() {
                let wanted = if *function == Function::If && index == 0 {
                    Type::Condition
                } else {
                    Type::Amount
                };
                require(argument, wanted, *position)?;
            }
            if function.rounding().is_some() {
                if let Some(Node::Number(places)) = arguments.get(1) {
                    places_of(*places, *position)?;
                }
            }
            Type::Amount
        }
    })
}

fn places_of(places: Decimal, position: usize) -> Result<u32, ExprError> {
    let whole = places.fract().is_zero() && places >= Decimal::ZERO;
    match u32::try_from(places.trunc().mantissa()) {
        Ok(places) if whole && places <= MAX_PLACES => Ok(places),
        _ => Err(ExprError::at(
            "ROUNDING_PLACES_INVALID",
            format!("Rounding takes a whole number of places from 0 to {MAX_PLACES}"),
            position,
        )),
    }
}

impl Expression {
    /// The input names the expression reads, sorted, each once.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// The functions the expression calls, sorted, each once.
    pub fn functions(&self) -> &[String] {
        &self.functions
    }

    /// The names the expression reads that `inputs` lacks.
    pub fn missing_names(&self, inputs: &BTreeMap<String, Decimal>) -> Vec<String> {
        self.names
            .iter()
            .filter(|name| !inputs.contains_key(*name))
            .cloned()
            .collect()
    }

    /// The expression's amount over `inputs`. Every name it reads must be there (`MISSING_INPUT`).
    pub fn evaluate(&self, inputs: &BTreeMap<String, Decimal>) -> Result<Decimal, ExprError> {
        let missing = self.missing_names(inputs);
        if !missing.is_empty() {
            return Err(ExprError::plain(
                "MISSING_INPUT",
                format!("The formula needs {}", missing.join(", ")),
            ));
        }
        Evaluator { inputs }.amount(&self.root)
    }
}

/// Parse and evaluate in one step.
pub fn evaluate(text: &str, inputs: &BTreeMap<String, Decimal>) -> Result<Decimal, ExprError> {
    parse(text, None)?.evaluate(inputs)
}

struct Evaluator<'a> {
    inputs: &'a BTreeMap<String, Decimal>,
}

fn overflow(position: usize) -> ExprError {
    ExprError::at("OVERFLOW", "The amount is too large", position.max(1))
}

impl Evaluator<'_> {
    fn amount(&self, node: &Node) -> Result<Decimal, ExprError> {
        match node {
            Node::Number(value) => Ok(*value),
            Node::Name { name, position } => self
                .inputs
                .get(name)
                .copied()
                .ok_or_else(|| ExprError::at("MISSING_INPUT", format!("The formula needs {name}"), *position)),
            Node::Negate { operand, .. } => Ok(-self.amount(operand)?),
            Node::Chain { first, rest } => {
                let mut total = self.amount(first)?;
                for (op, position, operand) in rest {
                    let value = self.amount(operand)?;
                    total = match op {
                        Op::Add => decimal_add(total, value).map_err(|_| overflow(*position))?,
                        Op::Subtract => decimal_subtract(total, value).map_err(|_| overflow(*position))?,
                        Op::Multiply => decimal_multiply(total, value).map_err(|_| overflow(*position))?,
                        Op::Divide => {
                            if value.is_zero() {
                                return Err(ExprError::at("DIVIDE_BY_ZERO", "Division by zero", *position));
                            }
                            total.checked_div(value).ok_or_else(|| overflow(*position))?
                        }
                        Op::And | Op::Or => return Err(mismatch(Type::Amount, *position)),
                    };
                }
                Ok(total)
            }
            Node::Call {
                function,
                arguments,
                position,
            } => self.call(*function, arguments, *position),
            Node::Not { position, .. } | Node::Compare { position, .. } => Err(mismatch(Type::Amount, *position)),
        }
    }

    fn condition(&self, node: &Node) -> Result<bool, ExprError> {
        match node {
            Node::Not { operand, .. } => Ok(!self.condition(operand)?),
            Node::Compare {
                compare, left, right, ..
            } => {
                let (left, right) = (self.amount(left)?, self.amount(right)?);
                Ok(match compare {
                    Compare::Less => left < right,
                    Compare::LessOrEqual => left <= right,
                    Compare::Greater => left > right,
                    Compare::GreaterOrEqual => left >= right,
                    Compare::Equal => left == right,
                    Compare::NotEqual => left != right,
                })
            }
            // and and or stop at the first operand that decides, so `b != 0 and a / b > 1` never divides by zero
            Node::Chain { first, rest } => {
                let mut result = self.condition(first)?;
                for (op, position, operand) in rest {
                    result = match op {
                        Op::And => result && self.condition(operand)?,
                        Op::Or => result || self.condition(operand)?,
                        _ => return Err(mismatch(Type::Condition, *position)),
                    };
                }
                Ok(result)
            }
            other => Err(mismatch(Type::Condition, other.position())),
        }
    }

    fn call(&self, function: Function, arguments: &[Node], position: usize) -> Result<Decimal, ExprError> {
        let argument = |index: usize| {
            arguments.get(index).ok_or_else(|| {
                ExprError::at(
                    "ARGUMENT_COUNT",
                    format!("{} is missing an argument", function.name()),
                    position,
                )
            })
        };
        match function {
            // only the branch taken is evaluated, so `if(b == 0, 0, a / b)` never divides by zero
            Function::If => {
                if self.condition(argument(0)?)? {
                    self.amount(argument(1)?)
                } else {
                    self.amount(argument(2)?)
                }
            }
            Function::Min | Function::Max => {
                let mut best = self.amount(argument(0)?)?;
                for node in &arguments[1..] {
                    let value = self.amount(node)?;
                    let better = if function == Function::Min {
                        value < best
                    } else {
                        value > best
                    };
                    if better {
                        best = value;
                    }
                }
                Ok(best)
            }
            Function::Abs => Ok(self.amount(argument(0)?)?.abs()),
            Function::Round | Function::Floor | Function::Ceil => {
                let value = self.amount(argument(0)?)?;
                let places = places_of(self.amount(argument(1)?)?, position)?;
                let strategy = function.rounding().unwrap_or(RoundingStrategy::MidpointAwayFromZero);
                let mut rounded = value.round_dp_with_strategy(places, strategy);
                // a rounded amount shows its places (round(5, 2) is 5.00), as a currency amount does
                if rounded.scale() < places {
                    rounded.rescale(places);
                }
                Ok(rounded)
            }
        }
    }
}

/// A decimal from an input's text: digits with an optional sign and point, exactly (no rounding of extra digits).
pub fn decimal_input(text: &str) -> Option<Decimal> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if !text.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+')) {
        return None;
    }
    Decimal::from_str_exact(text).ok()
}

#[cfg(test)]
#[path = "decimal_expr_tests.rs"]
mod tests;
