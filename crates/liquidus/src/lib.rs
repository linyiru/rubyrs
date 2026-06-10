//! liquidus — a Liquid-compatible template engine.
//!
//! Targets byte-identical output with Ruby liquid 4.x + Jekyll's
//! filters for an explicitly-bounded subset; templates using anything
//! outside the subset return [`Error::Declined`] at compile time so
//! embedders can fall back to the pure-Ruby gem (right-or-declined,
//! never silently wrong).
//!
//! Pre-alpha: API reservation release. The compile/render pipeline is
//! under active development in the rubyrs workspace.

/// A value supplied to the renderer. Mirrors the Liquid data model
/// (nil/bool/number/string plus arrays and string-keyed maps).
#[derive(Debug, Clone, PartialEq)]
pub enum LValue {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Array(Vec<LValue>),
    Map(Vec<(String, LValue)>),
}

/// Supplies variable values during a render. A template's required
/// variable paths are known statically (see [`Template::variables`]),
/// so embedders can batch-resolve them per render.
pub trait ValueSource {
    /// Resolve a dotted variable path (e.g. `"page.title"`).
    fn get(&mut self, path: &str) -> LValue;
}

/// Why liquidus refused a template.
#[derive(Debug)]
pub enum Error {
    /// The template uses a construct outside the implemented subset;
    /// the payload names it.
    Declined(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Declined(what) => write!(f, "declined: {what}"),
        }
    }
}

impl std::error::Error for Error {}

/// A compiled template: constant segments plus typed variable slots.
#[derive(Debug)]
pub struct Template {
    _private: (),
}

impl Template {
    /// The dotted variable paths this template reads. Stable across
    /// renders — compile once, batch-resolve per page.
    pub fn variables(&self) -> &[String] {
        &[]
    }
}

/// Compile Liquid `source` into a [`Template`].
///
/// Pre-alpha: every template currently declines while the engine is
/// developed (the right-or-declined contract from day one).
pub fn compile(_source: &str) -> Result<Template, Error> {
    Err(Error::Declined("liquidus pre-alpha: engine under development"))
}
