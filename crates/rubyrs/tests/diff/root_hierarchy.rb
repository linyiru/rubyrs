# Universal ancestor hierarchy: BasicObject → Object (Kernel mixed
# in). Real classes now appear in the chain instead of the previous
# isolated-Object stub. Locks the parity for the new structure
# introduced when preamble/object.rb was rewritten to use the full
# `class BasicObject; end; module Kernel; end; class Object <
# BasicObject; include Kernel; end` form.
#
# Knock-on effect: `Module#superclass` now raises NoMethodError
# (CRuby parity), and reflection-heavy code that walks
# `obj.class.ancestors` sees the same shape CRuby produces.

# --- Object's full ancestor chain ---
puts Object.ancestors.inspect                  # [Object, Kernel, BasicObject]
puts Object.superclass                         # BasicObject
puts BasicObject.superclass.inspect            # nil (root)

# --- Kernel is a Module included in Object ---
puts Kernel.is_a?(Module)                      # true
puts Kernel.is_a?(Class)                       # false
puts Kernel.ancestors.inspect                  # [Kernel]
puts Object.include?(Kernel)                   # true

# --- User classes inherit the chain transitively ---
class UserA
end
puts UserA.ancestors.inspect                   # [UserA, Object, Kernel, BasicObject]
puts UserA.new.is_a?(Object)                   # true
puts UserA.new.is_a?(Kernel)                   # true (Kernel is in chain)
puts UserA.new.is_a?(BasicObject)              # true (root)

# --- Class with explicit parent — chain extends ---
class UserB < UserA
end
puts UserB.ancestors.inspect                   # [UserB, UserA, Object, Kernel, BasicObject]
puts UserB.new.is_a?(UserA)                    # true
puts UserB.new.is_a?(Object)                   # true

# --- Module#superclass raises NoMethodError ---
module SomeModule
end
begin
  SomeModule.superclass
  puts "no raise (BAD)"
rescue NoMethodError => e
  puts "Module#superclass raises NoMethodError"
  # The error message includes the module name and the lowercase
  # word "module" (CRuby's exact format). We assert via `include?`
  # since the quote-style around the method name differs slightly
  # between rubyrs (backtick) and CRuby (straight quote) — a
  # pre-existing pretty-printing divergence not specific to this PR.
  puts e.message.include?("module SomeModule")    # true
end

# --- BasicObject.ancestors is just [BasicObject] ---
puts BasicObject.ancestors.inspect             # [BasicObject]

# --- Module#superclass user override wins (CRuby parity) ---
# Defining `def M.superclass` overrides the default raise.
# respond_to? also reflects the override.
module Overridden
  def self.superclass
    "user-defined"
  end
end
puts Overridden.superclass                     # user-defined
puts Overridden.respond_to?(:superclass)       # true

# --- respond_to?(:superclass) parity: Modules report false ---
# CRuby raises NoMethodError on M.superclass, so respond_to?
# returns false too. Feature-detection patterns like
# `cls.respond_to?(:superclass) && cls.superclass` rely on
# this truthiness alignment.
puts SomeModule.respond_to?(:superclass)       # false
puts Object.respond_to?(:superclass)           # true

# --- BasicObject can't be re-rooted ---
# CRuby raises TypeError on `class BasicObject < Anything`
# to prevent the cycle Object < BasicObject < Object.
begin
  eval "class BasicObject < Object; end"
  puts "no raise (BAD)"
rescue TypeError => e
  puts "BasicObject reparent: #{e.message}"
end

# --- Class < BasicObject explicit form bypasses Object ---
# (User code can opt out of Kernel/Object by inheriting from
# BasicObject directly — e.g. DSL receivers that want maximal
# method_missing coverage)
class MinimalReceiver < BasicObject
end
puts MinimalReceiver.ancestors.inspect         # [MinimalReceiver, BasicObject]
