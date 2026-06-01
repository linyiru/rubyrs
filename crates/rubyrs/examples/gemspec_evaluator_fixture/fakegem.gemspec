# Realistic gemspec shape, simplified. host_register_* callbacks
# capture each spec field as the script runs. No `$LOAD_PATH.unshift`
# in the fixture — the require MUST resolve via the host-supplied
# `Config::load_paths` seed, not a script-side mutation. That's the
# contract Phase 1 is validating; an inline unshift would mask a
# broken seed.
require "fakegem/version"

class Spec
  def initialize
    @name = nil
    @version = nil
    @deps = []
  end
  def name=(n); @name = n; host_register_name(n); end
  def version=(v); @version = v; host_register_version(v); end
  def add_dependency(name, version)
    @deps << [name, version]
    host_register_dependency(name, version)
  end
end

s = Spec.new
s.name = "fakegem"
s.version = FakeGem::VERSION
s.add_dependency "rack", ">= 3.0"
s.add_dependency "puma", "~> 6.0"
