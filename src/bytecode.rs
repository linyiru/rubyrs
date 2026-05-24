use std::rc::Rc;

use crate::error::Span;
use crate::value::Value;

// ---------- Bytecode ----------

#[derive(Debug, Clone, Copy)]
pub(crate) enum Op {
    LoadConstInt(i64),
    LoadConstStr(u32),   // proto.strings idx
    LoadSymbol(u32),     // proto.strings idx
    LoadNil,
    LoadTrue,
    LoadFalse,
    LoadSelf,
    LoadLocal(u16),
    StoreLocal(u16),     // pops
    Dup,
    Pop,
    LoadIvar(u32),       // proto.strings idx
    StoreIvar(u32),      // pops
    LoadConst(u32),      // class name idx
    Jump(i32),
    JumpIfFalse(i32),    // pops cond
    Call(u32, u8),       // name idx, argc; receiver on stack BELOW args
    CallNoRecv(u32, u8), // implicit self / builtin / toplevel
    DefMethod(u32, u32), // name idx, proto idx
    DefClass(u32, u32),  // name idx, body proto idx
    NewArray(u16),
    NewHash(u16),
    CreateBlock(u32, u16, u16), // proto_idx, param_start, n_params
    CallBlock(u32, u8),         // name, argc; expects [recv, block, ...args]
    CallNoRecvBlock(u32, u8),   // name, argc; expects [block, ...args]
    Yield(u8),
    BinOp(BinOpKind),
    PushRescue(i32, u16, u8), // handler relative offset, slot to bind exception (u16), 1 if bind else 0
    PopRescue,
    Raise,
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
    pub(crate) strings: Vec<String>,
    /// Parallel to `code`: op_spans[i] is the source span where code[i] was emitted.
    /// Used by Trap formatting in P0-B-2.
    #[allow(dead_code)]
    pub(crate) op_spans: Vec<Span>,
    /// Source filename — used when formatting a Trap backtrace in P0-B-2.
    #[allow(dead_code)]
    pub(crate) filename: Rc<str>,
}
