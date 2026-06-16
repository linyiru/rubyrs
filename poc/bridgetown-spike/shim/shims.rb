# Bridgetown spike discovery shims. Each shim names the VM/stdlib wall
# it bridges. These stand in for gems/stdlib that rubyrs does not yet
# vendor; the point is to discover the NEXT wall, not to be faithful.

# WALL: `bundler/shared_helpers` (and bundler itself) is not vendored.
# Bridgetown only touches Bundler at boot for plugin discovery; the
# require-time surface is just the namespace + a couple predicates.
module Bundler
  module SharedHelpers
    def self.in_bundle?
      false
    end
  end

  def self.bundler_major_version
    2
  end

  def self.with_unbundled_env
    yield if block_given?
  end

  def self.setup(*); end
  def self.require(*); end
  def self.reset!; end
end

# WALL: `Gem::Deprecate` is part of the unvendored rubygems runtime.
# addressable's idna/pure.rb does `extend Gem::Deprecate` then
# `deprecate :foo, :bar, 2023, 1` to wrap methods with a deprecation
# notice. For the spike a no-op `deprecate` (leave the method as-is) is
# faithful enough — the deprecation warning is cosmetic.
module Gem
  module Deprecate
    def deprecate(*); self; end
    def rubygems_deprecate(*); self; end
    def rubygems_deprecate_command(*); self; end
    def skip_during; yield if block_given?; end
  end

  # WALL: `Gem.find_files_from_load_path(glob)` is RubyGems runtime
  # surface (globs every $LOAD_PATH entry). bridgetown-core's
  # `Localizable#locale` uses it to discover `*.yml` locale files. No
  # gem locales on the spike load path, so `[]` is faithful for boot.
  def self.find_files_from_load_path(glob)
    $LOAD_PATH.flat_map { |dir| Dir.glob(File.join(dir, glob)) rescue [] }
  end
end

# WALL: `pp` (pretty-print stdlib) isn't vendored. faraday's logging
# formatter requires it for `Object#pretty_inspect`. A plain inspect (+ the
# trailing newline pp adds) is faithful enough for the load + the log path.
module Kernel
  def pretty_inspect
    "#{inspect}\n"
  end
end

# Satisfy `require "bundler/shared_helpers"` / `require "bundler"` with the
# shim above instead of the unvendored gem.
module Kernel
  alias_method :__bt_orig_require, :require unless private_method_defined?(:__bt_orig_require) || method_defined?(:__bt_orig_require)
  def require(name)
    # WALL: the real rubygems.rb / bundler are unvendored and pull in a
    # large C-backed surface. rubyrs already exposes a minimal `Gem`
    # namespace; treat these as already-loaded. `pp` is stubbed above.
    return true if name == "bundler/shared_helpers" || name == "bundler" || name == "rubygems" || name == "pp"
    __bt_orig_require(name)
  end
end
