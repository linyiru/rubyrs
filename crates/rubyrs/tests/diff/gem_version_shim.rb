# `Gem::Version` minimal shim. CRuby auto-loads RubyGems at
# interpreter startup so `Gem::Version` is always available;
# rubyrs has no RubyGems, so the preamble installs a tiny shim
# covering the surface that ecosystem code (Sinatra 4's
# `sinatra/indifferent_hash.rb:189`) uses at class-body
# load time:
#
#   def except(...) ... end if Gem::Version.new(RUBY_VERSION) >= Gem::Version.new("3.0")
#
# Discovery: P3 Sinatra spike — Sinatra previously raised
# `NameError: uninitialized constant ...::Gem::Version` at
# indifferent_hash.rb load.
#
# diff_cruby runs CRuby with `--disable=gems`, so the oracle
# side needs an explicit `require 'rubygems'` to materialise
# Gem; rubyrs treats that require as a no-op (the Gem shim is
# already loaded by the preamble).
require 'rubygems'

# Shape 1: Gem::Version.new(str) produces a comparable object.
v1 = Gem::Version.new("3.0.0")
v2 = Gem::Version.new("2.9.5")
puts "gt=#{v1 > v2}"
puts "lt=#{v2 < v1}"
puts "eq=#{v1 == Gem::Version.new("3.0.0")}"

# Shape 2: Sinatra's actual usage — `>=` against a "3.0"
# threshold. The current RUBY_VERSION (3.x.y) must pass.
puts "ge=#{Gem::Version.new("3.4.1") >= Gem::Version.new("3.0")}"

# Shape 3: padding shorter versions with implicit 0s
# (Rubygems::Version semantics).
puts "pad=#{Gem::Version.new("3.0") == Gem::Version.new("3.0.0")}"

# Shape 4: Comparable mixin produces `between?` and friends.
v = Gem::Version.new("3.2.0")
puts "between=#{v.between?(Gem::Version.new("3.0.0"), Gem::Version.new("4.0.0"))}"

# Shape 5: to_s round-trips the input string.
puts "to_s=#{Gem::Version.new("3.0.0.beta1").to_s}"
