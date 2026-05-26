# `super(*args)` / `super(a, *rest, b)` — splat inside a
# super call. Previously raised `unsupported node: SplatNode`
# at AST translation because the SuperNode arm did a flat
# `tr()` over every argument without the splat-grouping
# treatment regular Call sites used.
#
# Surfaced by Rack 3 `lib/rack/headers.rb`'s
# `super(*a.map!{|k| downcase_key(k)})` shape — three
# instances of `super(*expr)` in the Hash-method overrides.
# That file now loads cleanly after this fix.
#
# Compiles to a new `Op::ApplySuper(SymId)` op mirroring
# the existing `Op::ApplyCall`'s receiver-form sibling:
# pops an assembled Array and uses its elements as the
# positional args, then runs the same defining-class →
# superclass lookup `Op::Super` does.

class Parent
  def greet(*words)
    "p:" + words.join(" ")
  end
  def fetch(key, *rest)
    "p:" + key + ":" + rest.inspect
  end
  def transform(prefix, *suffixes)
    "p:" + prefix + "(" + suffixes.join(",") + ")"
  end
end

class Child < Parent
  # Plain `super(*args)` — splat the whole arg list.
  def greet(*words)
    super(*words)
  end

  # Mix of fixed arg + splat: `super(key.upcase, *rest)`.
  def fetch(key, *rest)
    super(key.upcase, *rest)
  end

  # Multiple splat positions / mixed shapes.
  def transform(prefix, *suffixes)
    extras = ["x", "y"]
    super(prefix.upcase, *extras, *suffixes)
  end
end

c = Child.new

# 0-, 1-, 2-arg cases.
puts c.greet
puts c.greet("hello")
puts c.greet("hello", "world")

# Mixed: fixed + splat.
puts c.fetch("name")
puts c.fetch("name", 1, 2, 3)
puts c.fetch("name", [])           # rest = [[]]

# Multiple splats.
puts c.transform("hi")             # extras only
puts c.transform("hi", "a", "b")   # extras + suffixes

# Calls compose correctly through several levels:
# Grandchild.greet → Child.greet → Parent.greet
class Grandchild < Child
  def greet(*words)
    super(*words.map(&:upcase))
  end
end
puts Grandchild.new.greet("hello", "world")
