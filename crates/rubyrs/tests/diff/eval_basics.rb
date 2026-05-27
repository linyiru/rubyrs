# `Kernel#eval(string)` / `Class#class_eval(string)` — runtime
# parse + compile + run.
#
# Motivating consumer: tilt-2.7.0 `lib/tilt/template.rb:1868`
#
#   def eval_compiled_method(method_source, offset, scope_class)
#     (scope_class || Object).class_eval(method_source, eval_file, line - offset)
#   end
#
# Tilt assembles a multi-line `def __tilt_xxx; ...; end` wrapped in
# a `Tilt::TOPOBJECT.class_eval do ... end` (the source string
# self-wraps in a nested block-form). With Kernel#eval + the
# string-form Class#class_eval intercept, tilt's compile pipeline
# advances past `eval_compiled_method` end-to-end.
#
# DIVERGENCE (documented in docs/SUBSET.md):
#   - `Class#class_eval(string)` does NOT switch to the receiver
#     class's class-body context. Bare
#     `Foo.class_eval("def bar; end")` lands `bar` at top level,
#     not on `Foo`. Tilt's self-wrapped shape uses the inner
#     block-form (which DOES switch context via the existing
#     `invoke_block_with_self` path), so its defs land correctly.
#   - The optional 2nd `binding` arg to `Kernel#eval` is silently
#     ignored (no Binding type in rubyrs); eval'd code sees only
#     top-level scope, not the caller's locals.
#   - `file` / `line` args are accepted; only `file` is wired
#     through to source registration.

# --- Kernel#eval: basic expression evaluation ---
puts eval("1 + 2")                              # 3
puts eval("'hello' + ' ' + 'world'")            # hello world

# --- Kernel#eval: defines methods that are then callable ---
eval("def __ev_helper; 42; end")
puts __ev_helper                                # 42

# --- Kernel#eval: returns the last expression value ---
result = eval("x = 10; y = 20; x + y")
puts result                                     # 30

# --- Class#class_eval(string) with the tilt-shape self-wrap ---
class Receiver
end
Receiver.class_eval(<<~RUBY)
  Receiver.class_eval do
    def greet
      "hello from receiver"
    end
  end
RUBY
puts Receiver.new.greet                         # hello from receiver

# --- Class#class_eval(string) with file + line args (signature
#     compatibility — file is used for source registration) ---
class Marker
end
Marker.class_eval(<<~RUBY, "custom.rb", 1)
  Marker.class_eval do
    def label
      "marked"
    end
  end
RUBY
puts Marker.new.label                           # marked

# --- module_eval is an alias for class_eval (string form) ---
class ModEvalTarget
end
ModEvalTarget.module_eval(<<~RUBY)
  ModEvalTarget.class_eval do
    def alive?
      true
    end
  end
RUBY
puts ModEvalTarget.new.alive?                   # true
