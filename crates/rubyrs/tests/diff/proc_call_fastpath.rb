# do_call's early `proc.call(args)` fast path (Block receiver + bare
# `call`) must match the general callable path across arities, closures,
# lambda-vs-proc arity strictness, break-from-proc, and the alias forms.
add = ->(a, b) { a + b }
p add.call(2, 3)
p add.(4, 5)
p add[6, 7]
dbl = lambda { |x| x * 2 }
p dbl.call(10)
begin; dbl.call(1, 2); rescue ArgumentError => e; p :lambda_strict; end
pr = proc { |x, y| [x, y] }
p pr.call(1)                       # proc lenient: [1, nil]
n = 0; inc = proc { n += 1 }; inc.call; inc.call; p n   # closure
p [1, 2, 3].map(&->(x) { x * x })  # block-arg form still works
# break from a stored proc called via .call -> LocalJumpError
runner = proc { break 99 }
begin; runner.call; p :no_error; rescue LocalJumpError; p :break_from_proc; end
# call returning a fresh Rack-style array (the benchmark shape)
app = ->(env) { [200, { "ct" => "text/plain" }, ["hi #{env[:p]}"]] }
p app.call({ p: "x" })
