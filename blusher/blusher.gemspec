# frozen_string_literal: true

require_relative "lib/blusher/version"

Gem::Specification.new do |spec|
  spec.name        = "blusher"
  spec.version     = Blusher::VERSION
  spec.summary     = "A fast, rouge-compatible syntax highlighter — Rust-backed, drop-in for rouge."
  spec.description = <<~DESC
    blusher routes Ruby's rouge lexers through the Rust `carmine` engine,
    which executes rule tables extracted from rouge's own lexers. Where carmine
    produces a byte-identical token stream it accelerates lexing (~4.6× faster);
    everywhere else it transparently falls back to rouge — zero code change,
    zero divergence (verified against rouge's full lexer spec suite, 757/757).
  DESC
  spec.authors  = ["momiji-rs"]
  spec.license  = "MIT"
  spec.homepage = "https://github.com/momiji-rs/blusher"
  spec.metadata = {
    "source_code_uri" => "https://github.com/momiji-rs/blusher",
    "changelog_uri"   => "https://github.com/momiji-rs/blusher/blob/main/CHANGELOG.md",
  }

  spec.required_ruby_version = ">= 3.0"
  spec.files = Dir["lib/**/*.rb", "lib/blusher/tables/*.json", "ext/*.{dylib,so}", "README.md", "CHANGELOG.md"]
  spec.require_paths = ["lib"]

  # Bootstrap: the native engine is the `carmine-ffi` cdylib, loaded via
  # Fiddle (`rake compile` stages it under ext/, or ship precompiled per
  # platform). The release path replaces this with an rb-sys/magnus extension
  # (which would set `spec.extensions = ["ext/blusher/extconf.rb"]`).
  spec.add_dependency "rouge", "~> 5.0"
end
