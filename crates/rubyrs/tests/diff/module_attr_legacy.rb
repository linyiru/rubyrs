## `Module#attr` — the pre-1.9 legacy alias for `attr_reader`.
## CRuby 3.4 still supports both shapes:
##   - `attr :name`, `attr :name1, :name2, ...` → reader(s)
##   - `attr :name, true` (1.8-only accessor form) → reader + writer
##   - `attr :name, false` → reader only (equivalent to bare form)
##
## Discovery context: rack-3.1.10/lib/rack/builder.rb:132 and
## rackup-2.2.1/lib/rackup/stream.rb:20-21 use the single-symbol
## reader form. sinatra-4 transitively requires both, so loading
## `sinatra/base` tripped on this. (TRY_RUNS pass-10 layer #10.)

## Shape 1: bare reader form — `attr :a` ≡ `attr_reader :a`.
class A
  attr :one
end
a = A.new
a.instance_variable_set(:@one, "value")
puts "one=#{a.one}"
puts "writer?=#{a.respond_to?(:one=)}"

## Shape 2: multi-symbol reader form.
class B
  attr :a, :b, :c
end
b = B.new
b.instance_variable_set(:@a, 1)
b.instance_variable_set(:@b, 2)
b.instance_variable_set(:@c, 3)
puts "abc=#{b.a},#{b.b},#{b.c}"
puts "writer-a?=#{b.respond_to?(:a=)}"

## Shape 3: 1.8-only `attr :name, true` accessor form. CRuby 3.4
## warns and accepts; rubyrs accepts silently.
class C
  attr :x, true
end
c = C.new
c.x = 10
puts "writer-fired=#{c.x}"
puts "writer?=#{c.respond_to?(:x=)}"

## Shape 4: `attr :name, false` — reader only.
class D
  attr :y, false
end
d = D.new
d.instance_variable_set(:@y, :sym)
puts "reader-only=#{d.y.inspect}"
puts "writer?=#{d.respond_to?(:y=)}"

## Shape 5: respond_to? advertises only what was defined.
puts "A-instance-methods=#{A.instance_methods(false).sort.inspect}"
puts "C-instance-methods=#{C.instance_methods(false).sort.inspect}"
