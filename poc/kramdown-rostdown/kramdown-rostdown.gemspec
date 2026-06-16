require_relative "lib/kramdown/rostdown/version"

Gem::Specification.new do |s|
  s.name        = "kramdown-rostdown"
  s.version     = Kramdown::Rostdown::VERSION
  s.summary     = "Rust-accelerated, byte-identical drop-in accelerator for kramdown (PoC)"
  s.description = <<~DESC
    Routes Kramdown::Document#to_html through the Rust `rostdown` renderer
    when the options + source fall inside its byte-identical subset, and
    falls back to pure-Ruby kramdown otherwise. Zero code change at the
    call site: just `require "kramdown-rostdown"`. Proof-of-concept ported
    from rubyrs' in-VM _kramdown_native accelerator.
  DESC
  s.authors  = ["Lawrence Lin"]
  s.license  = "MIT"
  s.homepage = "https://github.com/linyiru/rubyrs"

  s.required_ruby_version = ">= 3.0"
  s.files = Dir["lib/**/*.rb", "ext/**/*.{rs,toml}", "bin/*.rb", "README.md"]
  s.require_paths = ["lib"]

  # PoC binding: load the prebuilt cdylib via the `ffi` gem. A shippable
  # build would compile through rb-sys + rake-compiler instead (see README).
  s.add_dependency "ffi", "~> 1.15"
  s.add_dependency "kramdown", ">= 2.0"
  s.add_dependency "kramdown-parser-gfm", ">= 1.0"
  s.add_dependency "rouge", ">= 3.0"

  s.metadata["rubygems_mfa_required"] = "true"
end
