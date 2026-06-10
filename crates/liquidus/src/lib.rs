//! liquidus — a Liquid-compatible template engine.
//!
//! Targets byte-identical output with Ruby liquid 4.x + Jekyll's
//! filters for an explicitly-bounded subset; templates using anything
//! outside the subset return [`Error::Declined`] at compile time, and
//! renders that meet a value the subset can't reproduce exactly
//! decline at render time — embedders fall back to the pure-Ruby gem
//! either way (right-or-declined, never silently wrong).
//!
//! The design exploits the static nature of site templates: a template
//! compiles into constant segments plus typed variable slots, and the
//! value paths it needs are known statically ([`Template::variables`]).
//! Embedders resolve those once per render and pass a [`Values`] map —
//! no per-node dynamic dispatch.

mod filters;
mod parse;
mod render;
mod strftime;

use std::collections::HashMap;

/// A value supplied to the renderer. Mirrors the Liquid data model
/// plus a Time flavour for the date filters (Tier-1 UTC clock with the
/// local/utc FLAVOUR bit that decides zone rendering, matching the
/// rubyrs Time model).
#[derive(Debug, Clone, PartialEq)]
pub enum LValue {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Array(Vec<LValue>),
    Map(Vec<(String, LValue)>),
    Time { sec: i64, local: bool },
}

impl LValue {
    pub(crate) fn field(&self, name: &str) -> Option<&LValue> {
        match self {
            LValue::Map(pairs) => pairs.iter().find(|(k, _)| k == name).map(|(_, v)| v),
            _ => None,
        }
    }
}

/// One statically-known value requirement of a template.
#[derive(Debug, Clone, PartialEq)]
pub struct VarNeed {
    /// Dotted root path, e.g. `"page.title"` or `"site.posts"`.
    pub path: String,
    /// `Some(n)` when the path is only iterated under `limit: n` —
    /// the embedder may supply just the first `n` items.
    pub slice: Option<usize>,
    /// The template asks for this path's `size`. Supply the real
    /// length as [`LValue::Int`] under `path + "#size"` when a slice
    /// would hide it; otherwise the supplied array's length is used.
    pub need_size: bool,
    /// For iterated collections: the loop-variable fields the body
    /// reads (`post.url` → "url"). The embedder only needs to
    /// materialize these per item.
    pub fields: Vec<String>,
}

/// Resolved values for one render, keyed by [`VarNeed::path`] (plus
/// optional `path#size` companions).
#[derive(Debug, Default)]
pub struct Values(pub HashMap<String, LValue>);

/// Why liquidus refused a template or a render.
#[derive(Debug)]
pub enum Error {
    /// Outside the implemented subset; the payload names the construct.
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

/// Site-level constants that Jekyll filters close over (today just
/// `relative_url`'s baseurl).
#[derive(Debug, Clone, Default)]
pub struct SiteConfig {
    /// Jekyll `baseurl` (default "").
    pub baseurl: String,
}

/// A compiled template: constant segments + variable slots + control
/// flow, with includes expanded at compile time.
#[derive(Debug)]
pub struct Template {
    pub(crate) nodes: Vec<parse::Node>,
    pub(crate) needs: Vec<VarNeed>,
    pub(crate) config: SiteConfig,
}

impl Template {
    /// The value paths this template reads — stable across renders.
    pub fn variables(&self) -> &[VarNeed] {
        &self.needs
    }

    /// Render with resolved `values`. Runtime declines (a value shape
    /// the subset can't reproduce byte-exactly) return `Err`.
    pub fn render(&self, values: &Values) -> Result<String, Error> {
        render::render(self, values)
    }
}

/// Compile Liquid `source`. `include` resolves `{% include name %}`
/// bodies at compile time (Jekyll's `_includes/<name>`); returning
/// `None` declines the template.
pub fn compile(
    source: &str,
    config: SiteConfig,
    include: &dyn Fn(&str) -> Option<String>,
) -> Result<Template, Error> {
    let mut needs: Vec<VarNeed> = Vec::new();
    let nodes = parse::parse_template(source, include, &mut needs)?;
    needs.sort_by(|a, b| a.path.cmp(&b.path));
    needs.dedup_by(|a, b| {
        if a.path == b.path {
            // Merge duplicates: the widest slice wins (None =
            // unrestricted), size needs accumulate. dedup_by keeps
            // `b` (the earlier element) and drops `a`.
            b.slice = match (a.slice, b.slice) {
                (Some(x), Some(y)) => Some(x.max(y)),
                _ => None,
            };
            b.need_size |= a.need_size;
            b.fields.append(&mut a.fields);
            b.fields.sort();
            b.fields.dedup();
            true
        } else {
            false
        }
    });
    Ok(Template {
        nodes,
        needs,
        config,
    })
}
