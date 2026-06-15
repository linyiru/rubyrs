# `method(:print)` / `:puts` / `:p` — a Method object over a Kernel
# global builtin (no table Method). Calling it routes through the
# builtin, matching CRuby. zeitwerk's logging test does
# `loader.logger = method(:print)`.

mp = method(:p)
r = mp.call([1, 2])            # p prints inspect, returns its arg
p r

method(:puts).call("a", "b")
method(:print).call("x", "y", "\n")

# stored as an object and called later (the logger pattern)
logger = method(:print)
logger.call("logged\n")
logger.("via .() syntax\n")
logger["via [] syntax\n"]

# universal methods still route via the explicit fallback
m_class = method(:class)
p m_class.call                 # main's class
m_frozen = method(:frozen?)
p m_frozen.call

# arity / class of the Method object
p method(:print).class
