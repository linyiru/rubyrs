# Backtraces are recorded lazily at raise and built on first read
# (#383). Every read and copy path must see the raise-site frames.
# Only file:line is printed: rubyrs's method labels are a separate
# known divergence.
def lines(bt) = bt&.map { |f| f.split(":in ").first }

def a; raise ArgumentError, "x"; end
def b; a; end
def caught = begin; b; rescue => e; e; end

e = caught
p lines(e.backtrace)
p e.backtrace.equal?(e.backtrace)

# A re-raise keeps the original backtrace.
begin
  begin; b; rescue => e1; raise e1; end
rescue => e2
  p e2.equal?(e1), lines(e2.backtrace).first
end

# set_backtrace after the raise wins; nil clears it, and a re-raise
# then records a fresh one.
e3 = caught
e3.set_backtrace(["custom:1"])
p e3.backtrace
e3.set_backtrace(nil)
p e3.backtrace
begin; raise e3; rescue => e4; p lines(e4.backtrace).first; end

# Copies carry the backtrace.
e5 = caught
p e5.clone.backtrace == e5.backtrace, e5.dup.backtrace == e5.backtrace
p e5.exception("w").backtrace == e5.backtrace
e6 = caught
p lines(e6.clone.backtrace)
p e6.full_message(highlight: false).include?("x (ArgumentError)")

# A user set_backtrace override observes the raise.
class Bt < StandardError
  def set_backtrace(bt); @seen = bt.size; super; end
  attr_reader :seen
end
begin; raise Bt; rescue Bt => e7; p e7.seen == e7.backtrace.size, lines(e7.backtrace).first; end
class Bt2 < StandardError
  def set_backtrace(bt); @seen = true; end
  attr_reader :seen
end
begin; raise Bt2; rescue Bt2 => e8; p e8.seen, e8.backtrace; end

# The cause keeps its own backtrace.
begin
  begin; raise "inner"; rescue; raise TypeError, "outer"; end
rescue => e9
  p lines(e9.cause.backtrace).first, lines(e9.backtrace).first
end

# Explicit backtrace argument; never-raised exception.
begin; raise ArgumentError, "m", ["a:1", "b:2"]; rescue => e10; p e10.backtrace; end
begin; raise ArgumentError, "m", []; rescue => e11; p e11.backtrace; end
p RuntimeError.new("x").backtrace

# Many pending backtraces survive collections.
es = 3000.times.map { |i| begin; raise "e#{i}"; rescue => x; x; end }
GC.start
p es.map { |x| lines(x.backtrace) }.uniq.size, es.last.message

# Raised by the runtime, frozen, and through a block.
begin; nil.foo; rescue => e12; p lines(e12.backtrace).first; end
e13 = caught.freeze
p lines(e13.backtrace).first
begin; [1].each { |_| raise "blk" }; rescue => e14; p lines(e14.backtrace).first; end
p Marshal.load(Marshal.dump(caught)).backtrace.size == caught.backtrace.size

# rescue filters resolved through a lexical scope.
module M
  class Err < StandardError; end
  class C
    def r(k) = begin; raise(k == 0 ? Err : ArgumentError); rescue Err; :err; rescue ArgumentError; :arg; end
  end
end
p 3.times.map { |k| M::C.new.r(k % 2) }

# A repeated raise of an overriding class still calls the override,
# and a top-level method cannot shadow the backtrace reader.
p 3.times.map { begin; raise Bt; rescue Bt => x; x.seen == x.backtrace.size; end }
def __rubyrs_exc_backtrace = :shadowed
p lines(caught.backtrace).first
