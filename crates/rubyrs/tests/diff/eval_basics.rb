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

# --- defined?(eval) reports method (Kernel#eval mirror) ---
puts defined?(eval)                             # method

# --- respond_to?(:class_eval) / :module_eval lights up ---
class WhiteListed
end
puts WhiteListed.respond_to?(:class_eval)       # true
puts WhiteListed.respond_to?(:module_eval)      # true

# --- User `def self.class_eval(s)` override wins over the
#     string-form intercept (singleton-method ordering parity). ---
class OverrideClassEval
  def self.class_eval(s)
    "override:#{s}"
  end
end
puts OverrideClassEval.class_eval("ignored")    # override:ignored

# --- No-arg, no-block class_eval/module_eval raises ArgumentError
#     (not NoMethodError) since respond_to? advertises the method.
class NoArgs
end
begin
  NoArgs.class_eval
rescue ArgumentError
  puts "class_eval() → ArgumentError"
end
begin
  NoArgs.module_eval
rescue ArgumentError
  puts "module_eval() → ArgumentError"
end

# --- Non-String source arg raises TypeError (not NoMethodError) ---
class BadSrcArg
end
begin
  BadSrcArg.class_eval(123)
rescue TypeError
  puts "class_eval(non-string) → TypeError"
end

# --- block + args raises ArgumentError "expected 0" (CRuby parity)
class BlockPlusArgs
end
begin
  BlockPlusArgs.class_eval(123) { 1 }
rescue ArgumentError
  puts "class_eval(args) {} → ArgumentError"
end
begin
  BlockPlusArgs.class_eval("def x; end") { 1 }
rescue ArgumentError
  puts "class_eval(str) {} → ArgumentError"
end

# --- Bad filename arg raises TypeError (not ArgumentError) ---
class FilenameType
end
begin
  FilenameType.class_eval("1", 123)
rescue TypeError
  puts "class_eval(bad-file) → TypeError"
end

# --- Same TypeError shape for Kernel#eval's file arg ---
begin
  eval("1", nil, 123)
rescue TypeError
  puts "eval(bad-file) → TypeError"
end

# --- Arity guard fires BEFORE type guard (CRuby check order).
#     `eval(non-str, ..., extra)` reports ArgumentError for the
#     out-of-signature arity rather than masking with TypeError.
begin
  eval(123, nil, "file", 1, :extra)
rescue ArgumentError
  puts "eval(>4 args) → ArgumentError"
end
class ArityOrderCheck
end
begin
  ArityOrderCheck.class_eval(123, "file", 1, :extra)
rescue ArgumentError
  puts "class_eval(>3 args) bad-src → ArgumentError"
end

# --- Non-Integer line arg raises TypeError (CRuby parity).
#     Float is accepted (has `to_int`), but String/Symbol/nil aren't.
begin
  eval("1", nil, "file", "not-int")
rescue TypeError
  puts "eval(bad-line) → TypeError"
end
class LineCheck
end
begin
  LineCheck.class_eval("1", "file", "not-int")
rescue TypeError
  puts "class_eval(bad-line) → TypeError"
end

# --- Wrong arity raises ArgumentError (1..3 supported) ---
class ArityCheck
end
begin
  ArityCheck.class_eval("1", "file", 1, "extra")
  puts "no raise"
rescue ArgumentError
  puts "class_eval(>3 args) → ArgumentError"
end

# --- Bare `class_eval(...)` inside a class body (no explicit
#     receiver) reaches the receiver-form dispatch via the
#     no-recv → receiver-form bridge. ---
class BareCall
  class_eval(<<~RUBY)
    BareCall.class_eval do
      def echo
        "bare-call"
      end
    end
  RUBY
end
puts BareCall.new.echo                          # bare-call

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
