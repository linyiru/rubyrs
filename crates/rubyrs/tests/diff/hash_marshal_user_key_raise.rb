# A user `hash` override that RAISES during Marshal.load must surface
# the USER's exception, catchable with begin/rescue (CRuby dispatches
# key.hash while rebuilding the table; the raise propagates like any
# other). Pre-fix the Trap was collapsed into an uncatchable generic
# TypeError that aborted the process even inside a rescue. The VM must
# stay fully usable afterwards. (Adversarial-verifier probes
# 21_marshal_raise_min / 25_marshal_catch, 2026-07.)

class MK
  def initialize(v) = @v = v
  def hash = ($boom ? raise(ArgumentError, "kaboom") : @v.hash)
  def eql?(o) = o.is_a?(MK)
end

d = Marshal.dump({ MK.new(1) => :a })
$boom = true
begin
  Marshal.load(d)
  puts "loaded"
rescue ArgumentError => e
  puts "caught ArgumentError: #{e.message}"
rescue => e
  puts "caught #{e.class}: #{e.message}"
end

# a raise from eql? during the load-side dedup probe is catchable too
class EK
  def initialize(v) = @v = v
  def hash = @v.hash
  def eql?(o) = ($ebomb ? raise(RuntimeError, "eqlboom") : o.is_a?(EK) && o.instance_variable_get(:@v) == @v)
end
d2 = Marshal.dump({ EK.new(1) => :a, EK.new(1.0) => :b })
$ebomb = true
begin
  Marshal.load(d2)
  puts "loaded2"
rescue => e
  puts "caught #{e.class}: #{e.message}"
end
$ebomb = false

# VM healthy after both raises
$boom = false
puts "vm alive: #{{ MK.new(2) => 1 }.size} #{{ EK.new(3) => 1 }.size}"
h = { MK.new(4) => :x }
puts "roundtrip: #{Marshal.load(Marshal.dump(h)).size}"
