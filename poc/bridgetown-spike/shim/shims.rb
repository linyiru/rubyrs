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

# Satisfy `require "bundler/shared_helpers"` / `require "bundler"` with the
# shim above instead of the unvendored gem.
module Kernel
  alias_method :__bt_orig_require, :require unless private_method_defined?(:__bt_orig_require) || method_defined?(:__bt_orig_require)
  def require(name)
    # WALL: the real rubygems.rb / bundler are unvendored and pull in a
    # large C-backed surface. rubyrs already exposes a minimal `Gem`
    # namespace; treat these as already-loaded.
    return true if name == "bundler/shared_helpers" || name == "bundler" || name == "rubygems"
    __bt_orig_require(name)
  end
end
