# Symbol#inspect quoting: bare `:name` for identifier / operator
# names, quoted `:"..."` (string-escaped) otherwise. Exercised via
# `p` (Value::to_inspect), `.inspect`, and inside Array / Hash
# inspect. Discovery: P3 Jekyll spike — `:"".inspect` and spaced
# symbols diverged from CRuby's `p`.

# bare identifier forms
p :foo
p :Foo
p :_bar
p :foo123
p :foo?
p :foo!
p :foo=
p :@ivar
p :@@cvar
p :$glob

# operator method names print bare
p :+
p :-
p :*
p :<=>
p :==
p :[]
p :[]=
p :<<

# quoted forms (need `:"..."`)
p :""
p :"with space"
p :"1leading"
p :"a-b"
p :"foo.bar"
p :"has\"quote"
p :"tab\there"

# .inspect directly + nested in collections
puts :"with space".inspect
puts :"".inspect
p [:a, :"x y", :""]
p({ a: 1, :"x-y" => 2, "plain" => 3 })
