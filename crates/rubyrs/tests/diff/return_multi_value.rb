# `return a, b` / `next a, b` / `break a, b` — multi-value
# control-flow forms collapse to an Array literal (CRuby
# semantics).
#
# Previously the AST translation kept only the FIRST argument
# from a multi-arg `return` / `next` / `break`, silently
# dropping everything after. That broke destructuring
# assignment from method returns (`x, y = some_method`) and
# any caller using `*foo` to splat the result.
#
# Motivating use: MRI's `lib/erb/compiler.rb:466`
# (`return enc, frozen`) consumed by `*magic_comment` splat
# at line 326.

# --- return a, b → Array ---
def two
  return 1, 2
end
puts two.inspect                                # [1, 2]
puts two.class                                  # Array

# --- 3-arg form ---
def three
  return "a", :b, 3
end
puts three.inspect                              # ["a", :b, 3]

# --- Splat in return-args: `return a, *b, c` ---
def mid_splat
  return :first, *[1, 2], :last
end
puts mid_splat.inspect                          # [:first, 1, 2, :last]

# --- Pure splat with Array inner: passes through unchanged ---
# `return *arr` where arr is already an Array returns arr.
# This matches CRuby (which evaluates `return *expr` as
# `return Array(expr)` and Array() of an Array is the Array
# itself). The typical real-world shape: `return *method`
# where `method` already returns an Array.
def pure_splat
  return *[10, 20, 30]
end
puts pure_splat.inspect                         # [10, 20, 30]
# DIVERGENCE (not pinned): `return *5` returns `5` in rubyrs
# but `[5]` in CRuby — see the impl-site comment in
# `collect_multi_return_value` (ast.rs). Inherits the same
# gap as `[*5]` returning `5` instead of `[5]`. Closing it
# needs Kernel#Array.

# --- Destructuring consumer ---
def named_pair
  return :alpha, :beta
end
x, y = named_pair
puts x                                          # alpha
puts y                                          # beta

# --- Splat-destructure ---
a, *rest = three
puts a                                          # a
puts rest.inspect                               # [:b, 3]

# --- Multi-arg next from a block ---
def collect
  [0, 1, 2].map do |i|
    next i, i * 2
  end
end
puts collect.inspect                            # [[0, 0], [1, 2], [2, 4]]

# --- Multi-arg break from a loop ---
def search
  [1, 2, 3].each do |n|
    break :found, n if n == 2
  end
end
puts search.inspect                             # [:found, 2]

# --- ERB-shape probe ---
# Mirror lib/erb/compiler.rb:466: `return enc, frozen` then
# splat into a constructor's keyword positions.
def detect_magic
  return "UTF-8", nil
end
def consume(enc=nil, frozen=nil)
  "enc=#{enc.inspect} frozen=#{frozen.inspect}"
end
puts consume(*detect_magic)                     # enc="UTF-8" frozen=nil
