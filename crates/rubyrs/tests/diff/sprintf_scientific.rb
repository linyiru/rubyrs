# sprintf `%e` / `%E` (scientific, C-style `e+NN` exponent) and
# `%g` / `%G` (general: pick %e or %f by magnitude, strip trailing zeros).

# %e / %E
p("%e" % 12345.678)
p("%.2e" % 12345.678)
p("%E" % 0.00012)
p("%+.2e" % 3.14)
p("%e" % -1.5)
p("%e" % 0.0)
p("%.0e" % 9.9)        # rounds up across the exponent

# width / padding / zero-pad with %e
p("%10.2e" % 3.14)
p("%-10.2e|" % 3.14)
p("%010.2e" % 3.14)    # 0-flag zero-pads inside the value

# %g / %G
p("%g" % 12345.678)    # 6 sig figs → 12345.7
p("%g" % 0.0001234)
p("%g" % 100000.0)     # → 100000 (%f form)
p("%g" % 1000000.0)    # → 1e+06 (%e form)
p("%g" % 0.0)          # → 0
p("%g" % 0.5)
p("%.3g" % 12345.678)  # 3 sig figs → 1.23e+04
p("%G" % 0.00001)      # → 1E-05

# integers coerce to float for these
p("%e" % 5)
p("%g" % 100)
