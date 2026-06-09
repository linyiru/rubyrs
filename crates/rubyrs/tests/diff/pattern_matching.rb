# Pattern matching: `case/in`, `expr => pat`, `expr in pat`. Desugared to
# `===` (value patterns) + the deconstruct / deconstruct_keys protocol
# (structural), with `&&`/`||` short-circuit and side-effect bindings.

# value pattern + binding via `=>`
case 5
in Integer => n then p [:int, n]
in String then p :str
end

# array pattern
case [1, 2, 3]
in [a, b, c] then p [a, b, c]
end

# array with rest, and rest + post
case [1, 2, 3, 4]
in [first, *rest] then p [first, rest]
end
case [1, 2, 3, 4, 5]
in [a, *mid, y, z] then p [a, mid, y, z]
end

# hash pattern with value sub-patterns
case { name: "Bob", age: 30 }
in { name: String => nm, age: Integer => ag } then p [nm, ag]
end

# hash shorthand binding + **rest + **nil
case { x: 1, y: 2 }
in { x:, y: } then p [x, y]
end
case { a: 1, b: 2, c: 3 }
in { a:, **others } then p [a, others]
end
case { only: 1 }
in { only:, **nil } then p [:exact, only]
end

# guard (if / unless)
case 7
in n if n > 5 then p [:big, n]
in n then p [:small, n]
end
case 3
in n unless n > 5 then p [:kept, n]
end

# alternation
case 2
in 1 | 2 | 3 then p :low
in _ then p :high
end

# pin
expected = 42
case 42
in ^expected then p :pinned_ok
end
prefix = "foo"
case "foobar"
in ^prefix then p :no_eq
in String => s then p [:str, s]
end

# nested structural
case { user: { name: "Al", roles: [:admin, :user] } }
in { user: { name:, roles: [first_role, *] } } then p [name, first_role]
end

# range / class value patterns
case 50
in 0..9 then p :ones
in 10..99 then p :tens
end

# custom object implementing the protocol
class Point
  attr_reader :x, :y
  def initialize(x, y); @x = x; @y = y; end
  def deconstruct; [x, y]; end
  def deconstruct_keys(keys); { x: x, y: y }; end
end
case Point.new(1, 2)
in [px, py] then p [:arr, px, py]
end
case Point.new(3, 4)
in { x:, y: } then p [:hash, x, y]
end

# one-liner `in` (boolean, binds)
p(({ a: 1 } in { a: Integer }))
p((5 in String))

# one-liner `=>` (binding, raises on no match)
{ a: 1, b: 2 } => { a:, b: }
p [a, b]

# no match with no else -> NoMatchingPatternError
begin
  case 99
  in String then :x
  end
rescue NoMatchingPatternError
  p :no_match
end

# `=>` no match raises too
begin
  3 => String
rescue NoMatchingPatternError
  p :req_no_match
end
