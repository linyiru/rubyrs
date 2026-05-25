# Op-assignment for the families NOT covered by op_assign.rb:
# globals, constants, and constant-paths. Each gets `+=`, `||=`,
# and `&&=`. The local/ivar/index forms live in op_assign.rb.

# --- Globals ---
$g = 10
$g += 5
puts $g            # 15
$g *= 2
puts $g            # 30

# `||=` initialises unset global (read-as-nil).
$unset_or ||= "init"
puts $unset_or     # init
$unset_or ||= "skipped"
puts $unset_or     # init  (no overwrite)

# `&&=` does not init unset (nil && _ short-circuits to nil).
$unset_and &&= "skipped"
puts $unset_and.inspect  # nil
$set_and = "live"
$set_and &&= "updated"
puts $set_and      # updated

# --- Constants ---
FOO = 1
FOO += 9 rescue nil   # CRuby warns "already initialized constant"
                      # but still rewrites; rubyrs accepts silently.
puts FOO              # 10

UNSET_CONST ||= "init"
puts UNSET_CONST      # init

CONST_AND = "live"
CONST_AND &&= "updated"
puts CONST_AND        # updated

# --- ConstantPath ---
# rubyrs flattens constants to a "A::B" key; matches CRuby for
# top-level path writes and reads. Path is established with a
# bare `Bag::X = ...` rather than the `module Bag; X = ...; end`
# form which would store under just `X` in our model.
Bag = Class.new
Bag::X = 1
Bag::X += 9 rescue nil
puts Bag::X           # 10

Lazy = Class.new
Lazy::CACHE ||= "init"
puts Lazy::CACHE      # init

Tag = Class.new
Tag::FLAG = "live"
Tag::FLAG &&= "updated"
puts Tag::FLAG        # updated
