# Ruby 3.1 hash/keyword value-omission shorthand `{x:}` / `foo(x:)`.
x = 1
y = 2
p({ x:, y: })

name = "ruby"
version = 3
h = { name:, version:, fixed: 42 }
p h

def show(a:, b:)
  "#{a}-#{b}"
end
a = 10
b = 20
p show(a:, b:)

# Mixed with explicit pairs and a method-call value.
def two = 2
one = 1
p({ one:, two:, three: 3 })
