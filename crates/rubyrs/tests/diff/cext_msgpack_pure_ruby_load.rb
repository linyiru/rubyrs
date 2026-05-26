# Four msgpack-ruby `lib/msgpack/*.rb` helpers — buffer.rb,
# packer.rb, unpacker.rb, factory.rb — now load cleanly into
# rubyrs after this cycle's gap fills (nested-module dual-
# write from PR #89, bare-`new`/`method_defined?` retry,
# `Class#undef_method` no-op stub, RationalNode → Float).
#
# These are the "Ruby side of the cext" — class shells that
# `require 'msgpack/msgpack'` (the C ext) opens and fills
# in with Buffer / Packer / Unpacker / Factory implementation.
# rubyrs doesn't run the cext here; the fixture asserts that
# the pure-Ruby halves parse + define their classes + carry
# the expected method tables. Functional packing/unpacking
# round-trips still need the cext path (separate scope).
#
# Documented gaps NOT in scope:
#   - `Class#undef_method` is a no-op stub in rubyrs (matches
#     `Class#private`/`#public` with args), so `dup` / `clone`
#     are NOT actually removed; calling them would inherit
#     Object's. CRuby raises NoMethodError. The fixture stays
#     off that path.
#   - `Class#method_defined?` doesn't strip private methods in
#     rubyrs (covered separately in class_method_defined.rb).
#   - `to_msgpack` registered via core_ext.rb stays unverified
#     (the load happens but the actual pack call requires the
#     cext-side Packer).
#   - `Class#name` returns the bare local name in rubyrs vs
#     the fully-qualified `MessagePack::Buffer`-style path in
#     CRuby — the class object carries `Buffer`, the prefixed
#     `MessagePack::Buffer` constant is a separate lookup-time
#     alias from PR #89's dual-write, not stamped back on the
#     class. The fixture probes class identity via constant
#     resolution + `method_defined?` (both work) rather than
#     `name` so the assertion stays portable.

require_relative "../../examples/msgpack-cext/vendor-rb/msgpack/buffer.rb"
require_relative "../../examples/msgpack-cext/vendor-rb/msgpack/packer.rb"
require_relative "../../examples/msgpack-cext/vendor-rb/msgpack/unpacker.rb"
require_relative "../../examples/msgpack-cext/vendor-rb/msgpack/factory.rb"

# All four expose their public classes under MessagePack —
# probe via identity rather than `Class#name` (see header note
# on the unqualified-name divergence).
puts MessagePack::Buffer.equal?(MessagePack::Buffer)
puts MessagePack::Packer.equal?(MessagePack::Packer)
puts MessagePack::Unpacker.equal?(MessagePack::Unpacker)
puts MessagePack::Factory.equal?(MessagePack::Factory)
puts MessagePack::Buffer.is_a?(Class)
puts MessagePack::Packer.is_a?(Class)
puts MessagePack::Unpacker.is_a?(Class)
puts MessagePack::Factory.is_a?(Class)

# Pure-Ruby method names from each file land in the class's
# method table (the cext fills in the others at runtime).
puts MessagePack::Packer.method_defined?(:register_type)
puts MessagePack::Packer.method_defined?(:registered_types)
puts MessagePack::Packer.method_defined?(:type_registered?)
puts MessagePack::Unpacker.method_defined?(:register_type)
puts MessagePack::Unpacker.method_defined?(:registered_types)
puts MessagePack::Unpacker.method_defined?(:type_registered?)
puts MessagePack::Factory.method_defined?(:load)
puts MessagePack::Factory.method_defined?(:dump)
puts MessagePack::Factory.method_defined?(:pool)

# Factory has a Pool sub-class (defined inside Factory's body).
puts MessagePack::Factory::Pool.is_a?(Class)

# Pool.new takes (size) + a block; without the cext the block
# can't usefully open a Packer, but the class itself exists
# and its initializer signature is reachable.
puts MessagePack::Factory::Pool.method_defined?(:with)
puts MessagePack::Factory::Pool.method_defined?(:load)
puts MessagePack::Factory::Pool.method_defined?(:dump)
puts MessagePack::Factory::Pool.method_defined?(:unpacker)
puts MessagePack::Factory::Pool.method_defined?(:packer)
