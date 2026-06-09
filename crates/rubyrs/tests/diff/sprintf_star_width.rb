# sprintf `*` — argument-driven width and precision; a negative width
# left-justifies, a negative precision is ignored.
p "%*d" % [6, 42]
p "%-*d|" % [6, 42]
p "%*d" % [-6, 42]
p "%*.*f" % [10, 2, 3.14159]
p "%0*d" % [5, 7]
p "%.*f" % [3, 2.5]
p "%.*f" % [-1, 2.5]
p "%*s|" % [8, "hi"]
p "%*x" % [6, 255]
p format("%*d=%*d", 3, 1, 4, 2)
begin; "%*d" % ["x", 5]; rescue => e; p e.class; end
begin; "%*d" % [5]; rescue => e; p e.class; end
