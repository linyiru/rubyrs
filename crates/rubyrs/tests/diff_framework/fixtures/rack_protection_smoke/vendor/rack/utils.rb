# Minimal `rack/utils` for the rack_protection_smoke fixture.
# The real `rack` gem ships these helpers; for the parity oracle
# CRuby uses the real ones, and on rubyrs we either get them via
# sinatra_lite's `Rack::Utils` shim OR define them lazily here.
# The three middlewares we vendor (FrameOptions / XSSHeader /
# PathTraversal) only reach `Rack::Utils.secure_compare`
# transitively from base.rb's `secure_compare` helper — which
# itself is never invoked on the code paths the fixture
# exercises — so this shim is structurally complete with the
# module declaration alone.

module Rack
  module Utils
    # CRuby ships this; rubyrs's sinatra_lite already provides
    # `valid_path?` / `unescape_path` / `clean_path_info`. Adding
    # `secure_compare` here would be belt-and-suspenders for the
    # base.rb helper that doesn't fire on our routes; left as a
    # comment so a future fixture that DOES use CSRF tokens has
    # a clear hook to land it on.
    # def self.secure_compare(a, b); a == b; end
  end
end
