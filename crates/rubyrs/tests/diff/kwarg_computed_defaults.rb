# Keyword-arg default expressions — non-literal defaults are now
# accepted at parse time and evaluated per-call by a prologue
# (`Op::JumpIfKwArgGiven(kw_idx, off)`) at method-body entry,
# mirroring the existing positional-default prologue. Pre-fix,
# any non-literal kwarg default raised a SyntaxError at AST
# lowering.
#
# This was the blocker for `require 'mustermann'` in the Sinatra
# spike — mustermann's `def initialize(input, type: DEFAULT_TYPE,
# operator: :|)` uses constants and symbols as kwarg defaults.

# 1. Constant-reference default (the mustermann shape).
class DefaultType; end
def f1(type: DefaultType)
  type
end
puts f1.name
puts f1(type: Integer).name

# 2. Prior-param reference — kwarg default can read positional
# params (and earlier kwargs) bound by the binder before the
# prologue runs.
def f2(a, count: a + 1)
  "a=#{a} count=#{count}"
end
puts f2(5)
puts f2(5, count: 99)

# 3. Method call chain on a constant — Mustermann's
# `escape: URI_PARSER.regexp[:UNSAFE]` shape. The expression
# evaluates fresh on every call where the kwarg is omitted.
class Parser
  def regexp
    { UNSAFE: /[^a-z]/, GLOBAL: /\A.*\z/ }
  end
end
URI_PARSER = Parser.new
def f3(parser: URI_PARSER, pat: parser.regexp[:UNSAFE])
  pat.source
end
puts f3
puts f3(pat: /custom/)

# 4. String interpolation default — reads `who` via the prior-
# param mechanism.
def f4(who, greeting: "Hello, #{who}!")
  greeting
end
puts f4("World")
puts f4("World", greeting: "Hi.")

# 5. Mixed literal and computed defaults — verify both fast and
# prologue paths fire correctly in the same signature.
def f5(x, lit: 7, comp: x * 2, str: "fixed")
  [lit, comp, str]
end
puts f5(10).inspect
puts f5(10, lit: 99).inspect
puts f5(10, comp: 0).inspect
puts f5(10, lit: 1, comp: 2, str: "z").inspect

# 6. Splat + kwarg with computed default — the
# `def m(*input, type: Foo)` pattern Mustermann's
# `def self.new(*input, type: DEFAULT_TYPE, operator: :|, **opts)`
# uses (minus the kw-rest, which exercises a separate path).
class Foo; end
def f6(*input, type: Foo, operator: :|)
  "n=#{input.size} type=#{type} op=#{operator}"
end
puts f6
puts f6(1, 2, 3)
puts f6("a", "b", type: String, operator: :+)

# 7. Required positional + required kwarg + computed-default
# kwarg — verify the missing-required-kwarg ArgumentError still
# fires (binder didn't lose the required path).
def f7(a, b:, c: a + b)
  [a, b, c]
end
puts f7(1, b: 2).inspect
puts f7(10, b: 20, c: 99).inspect
begin
  f7(1)
rescue ArgumentError => e
  puts "required-kw: #{e.message}"
end

# 8. Closure over default — verify the default expression sees
# the method's lexical scope, not the caller's.
OUTER_VAL = 42
def f8(x: OUTER_VAL)
  x
end
puts f8
puts f8(x: 1)
