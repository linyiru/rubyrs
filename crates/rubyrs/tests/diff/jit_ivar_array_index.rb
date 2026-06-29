# ADR 0034 Step 1 (value-op) — `@arr[i]` with a VARIABLE Int index in a compiled
# method body lowers to a native array-element read (reusing jit_ivar_array_get_int).
# A step toward the rubocop AST-walk trunk (`kids[i]`). Parity must hold
# interpreter == JIT == CRuby, including the deopts (OOB, non-Int element).

@a = (0...20).to_a

def total(n)
  s = 0
  i = 0
  while i < n
    s += @a[i]
    i += 1
  end
  s
end
p total(20)
p total(0)        # empty loop

# Out-of-bounds index -> nil (must deopt cleanly, not a wrong answer / crash).
def at(i); @a[i]; end
p at(0)
p at(19)
p at(100)         # OOB -> nil

# Non-Int element -> deopt to the interpreter (the native reader is Int-only).
@b = [1, "two", :sym, 4]
def atb(i); @b[i]; end
p atb(0)
p atb(1)          # "two"
p atb(2)          # :sym
p atb(3)

# Negative index (Ruby wraps from the end) -> deopt path must match CRuby.
def neg; @a[-1]; end
p neg
