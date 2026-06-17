# sprintf/format positional argument references: `%N$` selects the
# N-th (1-based) argument for that spec instead of the next sequential
# one, and may be reused.
p "%2$s %1$s" % ["a", "b"]
p "%1$s %1$s %2$d" % ["x", 5]
p "%1$s-%2$s-%1$s" % ["a", "b"]
p "%2$05.2f" % [1, 3.14159]
p format("%1$#x %2$+d", 255, 7)
p "%3$s %1$s %2$s" % ["a", "b", "c"]

# 0$ and out-of-range references raise ArgumentError.
def rescued
  yield
rescue => e
  e.class
end
p(rescued { "%0$s" % ["x"] })
p(rescued { "%3$s" % ["x"] })

# Plain (non-positional) specs still work alongside literal '%' and
# width/precision — no '$' means it's a width, not a positional ref.
p "%d%%" % [50]
p "%-5d|%5d" % [3, 4]
p "%05.2f" % [3.14159]
