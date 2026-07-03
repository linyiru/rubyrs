# Tier-2 CONST battery (ADR 0037 tail): compiled bodies serve
# LoadConst / LoadConstChain from the interpreter's own inline constant
# caches (framed IC-hit helpers + the lite flat/chain reads) and
# LoadConstStr via the shared fresh-string push. Every serve must be
# invalidation-exact and semantics-identical to the interpreted arms.
N = 300

# 1. Flat + chain reads inside hot method bodies.
TOP = 7
module Outer
  INNER = 5
  module Deep
    DEEPER = 3
    def self.read = INNER + DEEPER + TOP
  end
end
def flat_read = TOP + Outer::INNER
N.times { flat_read; Outer::Deep.read }
p flat_read
p Outer::Deep.read

# 2. Const REMOVED + re-defined after the body is warm native: the
#    generation bump must force a re-resolve (no stale cached Value).
WARM = 1
def warm_read = WARM
N.times { warm_read }
p warm_read
Object.send(:remove_const, :WARM)
WARM = 99
p warm_read

# 3. Const removed after warm with NO replacement: the compiled body must
#    raise CRuby's NameError through the interpreted arm.
GONE = 5
def gone_read = GONE
N.times { gone_read }
Object.send(:remove_const, :GONE)
begin
  gone_read
rescue NameError => e
  puts e.message[/uninitialized constant \w+/]
end

# 4. Ancestor-vs-toplevel resolution from a warm chain read: defining a
#    NEARER constant after warm must win (cache invalidation via
#    const_gen, then phase-order re-resolution).
SHADOW = :toplevel
module Host
  def self.read = SHADOW
end
N.times { Host.read }
p Host.read
module Host
  SHADOW = :lexical
end
p Host.read

# 5. Autoload-pending const read from a compiled body: the FIRST read
#    after warm-up must fire the require (cold cache -> interpreted arm).
path = "/tmp/rubyrs_t2_const_battery_auto.rb"
File.write(path, "AUTO_T2 = :loaded_by_autoload\n")
autoload :AUTO_T2, path
def auto_read
  defined?(AUTO_T2) ? 1 : 0
end
N.times { auto_read } # warm WITHOUT triggering (defined? doesn't require)
def auto_value = AUTO_T2
p auto_value
p auto_value
File.delete(path)

# 6. private_constant read through a warm body must keep raising.
module Priv
  SECRET = 42
  private_constant :SECRET
  def self.inside = SECRET
end
N.times { Priv.inside }
p Priv.inside
begin
  def outside_read = Priv::SECRET
  N.times { }
  outside_read
rescue NameError => e
  puts e.message[/private constant/]
end

# 7. String literals from compiled bodies: fresh (mutations never alias),
#    frozen_string_literal off in this file, and correct inside blocks.
def str_fresh
  s = +"base"
  s << "!"
  s
end
N.times { str_fresh }
p str_fresh
p str_fresh
def str_blocks(arr)
  arr.map { |x| "v" + x.to_s }
end
N.times { str_blocks([1, 2]) }
p str_blocks([1, 2, 3])
