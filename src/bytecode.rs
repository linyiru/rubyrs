use std::rc::Rc;

use crate::error::Span;
use crate::intern::SymId;
use crate::value::Value;

// ---------- Bytecode ----------

#[derive(Debug, Clone, Copy)]
pub(crate) enum Op {
    LoadConstInt(i64),
    LoadConstStr(SymId),
    LoadSymbol(SymId),
    LoadNil,
    LoadTrue,
    LoadFalse,
    LoadSelf,
    LoadLocal(u16),
    StoreLocal(u16),
    /// Fast path for `name = name + 1`: increment slot in place, push new value.
    /// Falls back to a synthesised `BinOp::Add` if the slot doesn't hold an Int.
    IncLocal(u16),
    /// Same as `IncLocal` but does *not* push the resulting value. Emitted
    /// in statement position where the body discards the value anyway.
    IncLocalNoPush(u16),
    Dup,
    Pop,
    LoadIvar(SymId),
    StoreIvar(SymId),
    /// Fast path for `@name = @name + 1`. Same shape as IncLocal but on
    /// self's ivar table.
    IncIvar(SymId),
    /// Same as `IncIvar` but does *not* push the resulting value.
    IncIvarNoPush(SymId),
    LoadConst(SymId),
    Jump(i32),
    JumpIfFalse(i32),
    /// Args: name SymId, argc, per-call-site inline-cache slot id.
    Call(SymId, u8, u16),
    CallNoRecv(SymId, u8, u16),
    DefMethod(SymId, u32),         // name, proto_idx
    DefClass(SymId, u32),
    NewArray(u16),
    NewHash(u16),
    CreateBlock(u32, u16, u16),    // proto_idx, param_start, n_params
    CallBlock(SymId, u8, u16),
    CallNoRecvBlock(SymId, u8, u16),
    Yield(u8),
    BinOp(BinOpKind),
    /// Fast path for `recv <op> <int_literal>` — fuses the preceding
    /// `LoadConstInt` into the BinOp. Saves one op and one stack
    /// round-trip per such expression. Falls back to generic dispatch
    /// when LHS isn't an `Int`.
    BinOpInt(BinOpKind, i64),
    PushRescue(i32, u16, u8),
    PopRescue,
    /// Like PushRescue but for `ensure` clauses. When an exception is
    /// unwinding and hits a PushEnsure handler, the exception value is
    /// pushed onto the operand stack and control jumps to the handler;
    /// the handler runs the ensure body (which must leave the stack
    /// unchanged) and ends with `Raise` to rethrow.
    PushEnsure(i32),
    PopEnsure,
    Raise,
    /// Signals the current iteration driver (Array#each, #map, etc.) to
    /// stop and use the value on top of the operand stack as the call's
    /// return value. Almost always emitted as `<val>; Break; Return` so
    /// the block frame also pops.
    Break,
    Return,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BinOpKind { Add, Sub, Mul, Div, Mod, Lt, Le, Gt, Ge, Eq, Ne }

impl BinOpKind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            BinOpKind::Add => "+", BinOpKind::Sub => "-", BinOpKind::Mul => "*",
            BinOpKind::Div => "/", BinOpKind::Mod => "%",
            BinOpKind::Lt => "<", BinOpKind::Le => "<=",
            BinOpKind::Gt => ">", BinOpKind::Ge => ">=",
            BinOpKind::Eq => "==", BinOpKind::Ne => "!=",
        }
    }
    pub(crate) fn from_op_name(s: &str) -> Option<Self> {
        Some(match s {
            "+" => BinOpKind::Add, "-" => BinOpKind::Sub, "*" => BinOpKind::Mul,
            "/" => BinOpKind::Div, "%" => BinOpKind::Mod,
            "<" => BinOpKind::Lt, "<=" => BinOpKind::Le,
            ">" => BinOpKind::Gt, ">=" => BinOpKind::Ge,
            "==" => BinOpKind::Eq, "!=" => BinOpKind::Ne,
            _ => return None,
        })
    }
    pub(crate) fn apply_int(self, a: i64, b: i64) -> Value {
        match self {
            BinOpKind::Add => Value::Int(a.wrapping_add(b)),
            BinOpKind::Sub => Value::Int(a.wrapping_sub(b)),
            BinOpKind::Mul => Value::Int(a.wrapping_mul(b)),
            BinOpKind::Div => Value::Int(a.wrapping_div(b)),
            BinOpKind::Mod => Value::Int(a.wrapping_rem(b)),
            BinOpKind::Lt => Value::Bool(a < b),
            BinOpKind::Le => Value::Bool(a <= b),
            BinOpKind::Gt => Value::Bool(a > b),
            BinOpKind::Ge => Value::Bool(a >= b),
            BinOpKind::Eq => Value::Bool(a == b),
            BinOpKind::Ne => Value::Bool(a != b),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Proto {
    pub(crate) name: String,
    pub(crate) params: Vec<String>,
    pub(crate) n_locals: u16,
    pub(crate) code: Vec<Op>,
    /// Parallel to `code`: op_spans[i] is the source span where code[i] was emitted.
    pub(crate) op_spans: Vec<Span>,
    /// Source filename — used by Trap backtrace formatting.
    pub(crate) filename: Rc<str>,
}
