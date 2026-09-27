# Zero-arg yield / call into every block shape: the plain ones take the
# `invoke_block0` fast path, the rest must fall back to the general binder.
def y0; yield; end
def y0_twice; [yield, yield]; end

# Plain block, no params.
p y0 { 1 }
# Params bind nil on every invocation (param slots are not body-reset).
p y0_twice { |a| a }
p y0_twice { |a| r = a; a = 9; r }
p y0 { |a, b| [a, b] }
p y0 { |a, (b, c)| [a, b, c] }
# Optional param: its default runs.
p y0 { |a = 5| a }
p y0_twice { |a = [], b = a.size| [a, b] }
# Rest / kw / kw-rest / block param: general path.
p y0 { |*r| r }
p y0 { |a, *r| [a, r] }
p y0 { |k: 3| k }
p y0 { |**o| o }
p y0 { |&b| b }
# Body locals are fresh each invocation.
p y0_twice { x = (x || 0) + 1; x }
# Outer locals are shared and written through.
n = 0
3.times { y0 { n += 1 } }
p n
# Closure capture per invocation.
procs = []
2.times { |i| y0 { procs << -> { i } } }
p procs.map(&:call)

# Lambdas: a 0-param lambda is served; declared params keep strict arity.
def with_lam(l); yield_it = l; yield_it.call; end
p with_lam(-> { :ok })
p with_lam(->(a = 7) { a })
begin
  with_lam(->(a) { a })
rescue ArgumentError => e
  p e.message
end
p y0(&-> { :lam })
begin
  y0(&->(a) { a })
rescue ArgumentError => e
  p e.message
end

# Proc#call with a block argument (the pending caller-block channel).
pr = proc { |&b| b ? b.call : :none }
p pr.call { :given }
p pr.call
pr2 = proc { :plain }
p pr2.call { :ignored }
p pr2.call

# break / next / return / raise / ensure out of a zero-arg yield.
p y0 { break :broke }
p y0 { next :nexted }
def ret_from_block; y0 { return :returned }; :not_reached; end
p ret_from_block
def ensure_in_yield
  log = []
  r = y0 do
    begin
      raise "boom"
    rescue => e
      log << e.message
      :rescued
    ensure
      log << :ensure
    end
  end
  [r, log]
end
p ensure_in_yield
def rescue_around_yield
  y0 { raise ArgumentError, "bad" }
rescue ArgumentError => e
  [:caught, e.message]
end
p rescue_around_yield

# loop drives its block with zero args.
i = 0
r = loop do
  i += 1
  break i * 10 if i == 4
end
p r

# self and instance variables inside the block.
class Holder
  def initialize; @v = 42; end
  def run; y0 { [self.class, @v] }; end
  def y0; yield; end
end
p Holder.new.run

# block_given? and nested yield through a block.
def nested; [1].map { yield }; end
p nested { :inner }
def maybe; block_given? ? yield : :no_block; end
p maybe
p maybe { :blk }
