# %w[] word arrays and %i[] symbol arrays. Lowercase forms
# (`%w` / `%i`) are non-interpolating; uppercase (`%W` / `%I`)
# allow `#{expr}` interpolation. Prism parses these as ordinary
# ArrayNodes containing StringNode / SymbolNode children, so
# rubyrs supports them out of the box once those translators
# exist — this fixture pins the contract.

# Basic %w word array.
p %w[hello world]
p %w[one]
p %w[]
p %w[a b c d e]

# Different brackets — same semantics.
p %w(parens style)
p %w{curly braces}

# Basic %i symbol array.
p %i[red green blue]
p %i[only]
p %i[]
p %i(parens syms)

# Word array with hyphens and underscores.
p %w[snake_case kebab-case CamelCase]

# Mixed with regular literal arrays.
fruits = %w[apple banana cherry]
puts fruits.length
puts fruits.first
puts fruits.last
puts fruits.join(", ")

# %i with map.
levels = %i[debug info warn error fatal]
p levels.map(&:to_s)

# %w as a method argument.
def first_of(arr)
  arr[0]
end
puts first_of(%w[red green blue])

# %W with interpolation.
n = 42
p %W[count: #{n} status: ok]

# Iteration over %w.
%w[a b c].each { |w| puts w.upcase }

# Inside a class.
class Config
  def levels
    %i[debug info warn error fatal]
  end
  def names
    %w[server client agent]
  end
end

c = Config.new
p c.levels
p c.names

# Splat in `when` (`when *%w[...]`) isn't supported yet — use
# explicit literal lists as the workaround.
def category2(s)
  case s
  when "red", "green", "blue" then "color"
  when "ruby", "python", "go" then "language"
  else "other"
  end
end
puts category2("red")
puts category2("ruby")
puts category2("zebra")
