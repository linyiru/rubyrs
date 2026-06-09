# Collection inspect dispatches each element's real `inspect` (custom
# override / Exception message) and is cycle-safe: a self-referential
# Array/Hash renders `[...]` / `{...}` instead of overflowing the stack.

class Custom
  def inspect; "<<C>>"; end
end
c = Custom.new

# nested custom inspect (was: #<Custom>)
p [c]
p [1, c, "s"]
p({ k: c, "n" => c })
p [[c], [c, c]]

# nested Exception inspect (was: #<RuntimeError>)
e = (raise "boom" rescue $!)
p [e]
p({ err: e })

# self-reference cycles (were: stack overflow crash)
a = [1]
a << a
p a                       # [1, [...]]
h = {}
h[:self] = h
p h                       # {self: {...}}

# mutual cycle
x = []
y = [x]
x << y
p x                       # [[[...]]]

# deep acyclic nesting still fully expands
p [1, [2, [3, [4, [5]]]]]
p({ a: { b: { c: 1 } } })

# to_s aliases inspect for collections
puts [1, c].to_s
puts({ a: c }.to_s)

# inspect of the array via interpolation
puts "arr=#{[1, c]}"
