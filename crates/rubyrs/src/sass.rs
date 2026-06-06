//! SCSS / Sass → CSS compilation backend (the `sass` battery).
//!
//! rubyrs cannot run the real `sass-embedded` gem (it drives a native
//! dart-sass binary over google-protobuf via an Open3 subprocess —
//! all outside the Tier-1 sandbox). Instead, the Sass *capability* is
//! provided Rust-side, the same way regex / bignum / the HTTP server
//! are: a Rust crate behind a Cargo feature, surfaced to Ruby through
//! a host primitive (`RubyrsSass.compile`, wired in vm/dispatch.rs;
//! consumed by the vendored jekyll-sass-converter shim).
//!
//! ## Swappable backend seam
//!
//! Compilation goes through the [`SassBackend`] trait and the
//! [`compile`] resolver. The default backend (under `feature = "sass"`)
//! is [`GrassBackend`], wrapping the pure-Rust `grass` crate
//! (~90% dart-sass spec coverage; `@import`-based, which is what Jekyll
//! themes use). A future native `rubyrs-sass` implementation can slot
//! in behind the same trait — change only `active_backend()` here; the
//! host primitive and the Ruby/Jekyll side stay untouched.

/// A SCSS/Sass → CSS compiler. Implementors take SCSS source and
/// return rendered CSS, or an error message (surfaced to Ruby as a
/// compilation error).
pub(crate) trait SassBackend {
    fn compile(&self, scss: &str) -> Result<String, String>;
}

/// `grass`-backed implementation (the current default backend).
#[cfg(feature = "sass")]
pub(crate) struct GrassBackend;

#[cfg(feature = "sass")]
impl SassBackend for GrassBackend {
    fn compile(&self, scss: &str) -> Result<String, String> {
        grass::from_string(scss.to_string(), &grass::Options::default())
            .map_err(|e| e.to_string())
    }
}

/// The active backend, or `None` when the `sass` battery is disabled.
/// This is the single point a native `rubyrs-sass` backend would hook
/// into (add an arm / swap the constructor).
pub(crate) fn active_backend() -> Option<Box<dyn SassBackend>> {
    #[cfg(feature = "sass")]
    {
        Some(Box::new(GrassBackend))
    }
    #[cfg(not(feature = "sass"))]
    {
        None
    }
}

/// Compile SCSS/Sass source to CSS via the active backend. Returns a
/// `feature-absent` error when no backend is built in.
pub(crate) fn compile(scss: &str) -> Result<String, String> {
    match active_backend() {
        Some(backend) => backend.compile(scss),
        None => Err(
            "rubyrs: SCSS/Sass compilation is unavailable — build with \
             `--features sass` (the grass-backed battery). Plain, \
             non-SCSS sites build without it."
                .to_string(),
        ),
    }
}

#[cfg(all(test, feature = "sass"))]
mod tests {
    use super::*;

    #[test]
    fn grass_backend_compiles_core_scss() {
        // Variables, unit math, nesting, `&`, a color function, and a
        // mixin — the surface a Jekyll theme's SCSS exercises.
        let scss = "\
$primary: #0066cc;
$pad: 8px;
@mixin card($bg) { background: $bg; border-radius: 4px; }
.btn {
  color: $primary;
  padding: $pad ($pad * 2);
  &:hover { color: darken($primary, 10%); }
}
.card { @include card(#fafafa); }
";
        let css = compile(scss).expect("grass should compile");
        assert!(css.contains("color: #0066cc;"), "var substitution: {css}");
        assert!(css.contains("padding: 8px 16px;"), "unit math: {css}");
        assert!(css.contains(".btn:hover"), "nesting + &: {css}");
        assert!(css.contains("color: #004d99;"), "darken(): {css}");
        assert!(css.contains("border-radius: 4px;"), "mixin: {css}");
    }

    #[test]
    fn grass_backend_reports_errors() {
        // Malformed SCSS surfaces as an Err (the converter shim turns
        // this into a Jekyll SyntaxError).
        assert!(compile(".a { color: ; }").is_err());
    }
}
