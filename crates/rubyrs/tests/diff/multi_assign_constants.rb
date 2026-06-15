# Multiple assignment where the targets are CONSTANTS (and splat-to-
# constant) — `MAJOR, MINOR, BUILD, *OTHER = ...` (rake/version.rb:6).
# Previously raised "unsupported multi-write target: ConstantTargetNode".
module V
  MAJOR, MINOR, BUILD, *OTHER = "13.2.1.pre.4".split(".")
  NUMBERS = [MAJOR, MINOR, BUILD, *OTHER]
end
p [V::MAJOR, V::MINOR, V::BUILD, V::OTHER]
p V::NUMBERS
A1, B1 = 1, 2                      # top-level constants
p [A1, B1]
C1, *D1 = [10, 20, 30, 40]         # trailing splat
p [C1, D1]
*E1 = [1, 2, 3]                    # all into splat
p E1
F1, G1, *H1 = [9]                  # fewer values than targets
p [F1, G1, H1]
