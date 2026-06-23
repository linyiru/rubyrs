# `method` / `public_method` / `singleton_method` accept a String name
# (to_sym'd) on any receiver — including a Class/Module. dry-types builds
# coercers with `::Kernel.method(primitive.name)` (a String).
p Kernel.method("Integer").class
p Math.method("sqrt").call(9.0)
p "x".method("upcase").call
p String.method("new").call("hi")
p 5.method("+").call(3)
