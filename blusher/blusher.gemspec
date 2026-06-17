# frozen_string_literal: true

require_relative "lib/blusher/version"

Gem::Specification.new do |spec|
  spec.name        = "blusher"
  spec.version     = Blusher::VERSION
  spec.summary     = "A faster, Rust-backed, byte-for-byte-compatible drop-in backend for rouge."
  spec.description = <<~DESC
    blusher routes Ruby's rouge lexing — and, for the common HTML path, its
    formatting — through the Rust `carmine` engine, which executes rule tables
    extracted from rouge's own lexers. For an unadorned Rouge::Formatters::HTML
    pipeline it fuses lex+format in Rust and returns one String, crossing the
    Ruby boundary once instead of per-token: ~1.7x faster on a mixed corpus
    (2.5x+ on large files), byte-identical, with transparent fallback to rouge
    for callback lexers and other formatters. Verified against rouge's full
    lexer spec suite (757/757). The engine's raw 4.6x is realized Rust-to-Rust.
  DESC
  spec.authors  = ["momiji-rs"]
  spec.license  = "MIT"
  spec.homepage = "https://github.com/momiji-rs/blusher"
  spec.metadata = {
    "source_code_uri" => "https://github.com/momiji-rs/blusher",
    "changelog_uri"   => "https://github.com/momiji-rs/blusher/blob/main/CHANGELOG.md",
  }

  spec.required_ruby_version = ">= 3.0"
  spec.required_rubygems_version = ">= 3.3.11" # cargo-builder support in rubygems
  spec.files = Dir[
    "lib/**/*.rb",
    "lib/blusher/tables/*.json",
    "ext/blusher/**/*.{rs,rb,toml}",
    "README.md", "CHANGELOG.md", "LICENSE*"
  ]
  spec.require_paths = ["lib"]

  # The native engine is an rb-sys/magnus extension, compiled at install
  # (or shipped precompiled per platform via .github/workflows/release.yml).
  # `Blusher::Native` also accepts a Fiddle-loaded carmine-ffi cdylib as a
  # dependency-light fallback (see lib/blusher/native.rb).
  spec.extensions = ["ext/blusher/extconf.rb"]
  spec.add_dependency "rouge", "~> 5.0"
  spec.add_dependency "rb_sys", "~> 0.9"
end
