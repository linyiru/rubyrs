## `Module#define_method(:name) { |args| body }` — dynamically
## install a block-as-method on a class's instance-methods
## table. Mirrors `Op::DefMethodBlock` (the parsed-`def`
## opcode), but entered via runtime dispatch instead.
## Closes TRY_RUNS pass-9.7d layer #21 — sinatra/base.rb:1735
## (inside `define_singleton`) does
##   singleton_class.class_eval do
##     ...
##     define_method(name, &content)
##   end
## which is the bare-call-inside-class_eval shape (no_recv +
## self_val == the class).
##
## Both call shapes supported:
##   - explicit receiver: `cls.define_method(:foo) { ... }`
##   - bare-call inside class_eval / Module.new block where
##     `self` is the class.

## Shape 1: bare-call inside a regular class body (self is
## the class).
class Greeter
  define_method(:greet) { |name| "hello-#{name}" }
  define_method(:shout) { |name| "HELLO-#{name.upcase}" }
end

puts "bare=#{Greeter.new.greet("world").inspect}"
puts "bare-shout=#{Greeter.new.shout("ruby").inspect}"

## Shape 2: explicit receiver.
class Waver; end
Waver.define_method(:wave) { "wave" }
puts "explicit=#{Waver.new.wave.inspect}"

## Shape 3: bare-call inside class_eval (sinatra's pattern).
## class_eval's block runs with self = the class.
class TargetClass; end
TargetClass.class_eval do
  define_method(:from_class_eval) { "via-class-eval" }
end
puts "via-class-eval=#{TargetClass.new.from_class_eval.inspect}"

## Shape 4: with &Proc block_arg (sinatra's exact call shape).
proc_body = proc { |x| "proc-#{x}" }
class WithBlockArg; end
WithBlockArg.class_eval do
  define_method(:from_proc, &proc_body)
end
puts "via-block-arg=#{WithBlockArg.new.from_proc(42).inspect}"

## CRuby returns the method name as a Symbol — pin that
## return shape too.
class ReturnShape; end
result = ReturnShape.define_method(:hi) { "hi" }
puts "return-shape=#{result.inspect}"

## Wrong-arity raises ArgumentError.
class WrongArity; end
err = begin
  WrongArity.define_method { "no args" }
  "DID-NOT-RAISE"
rescue ArgumentError
  "ArgumentError"
end
puts "wrong-arity=#{err}"

## Non-symbol/string name raises TypeError.
class NonSymName; end
err = begin
  NonSymName.define_method(42) { "wrong" }
  "DID-NOT-RAISE"
rescue TypeError
  "TypeError"
end
puts "non-symbol-name=#{err}"

## Closure semantics: define_method captures the surrounding
## scope. Both rubyrs and CRuby agree the captured local
## is read at call time, not defined time.
counter = 0
class Closure; end
Closure.define_method(:bump) {
  counter += 1
  counter
}
puts "closure-1=#{Closure.new.bump}"
puts "closure-2=#{Closure.new.bump}"
puts "closure-3=#{Closure.new.bump}"

## respond_to?(:define_method) on a Class — feature detection
## must agree with what dispatch will accept (Copilot review
## #245 round 1). Pre-fix the Class whitelist in
## `Vm::responds_to` omitted `define_method`, so this returned
## false even though the call works.
class FeatureCheck; end
puts "respond-to-define-method=#{FeatureCheck.respond_to?(:define_method)}"

## String name returns the interned symbol (`:from_str`), same
## as Symbol-name path — pins return-value shape for the
## String→Sym intern arm.
class StringName; end
sym_back = StringName.define_method("from_str") { "ok" }
puts "string-name-returns=#{sym_back.inspect}"
puts "string-name-call=#{StringName.new.from_str.inspect}"

## User-defined `def self.define_method(...)` on a class
## overrides the built-in intrinsic (Copilot review #245
## round 1). Singleton-method precedence parallels
## `Module.new` / `Hash.new`.
class Overridden
  def self.define_method(*a, &blk)
    "user-override:#{a.inspect}:block=#{!blk.nil?}"
  end
end
puts "override=#{Overridden.define_method(:ignored) { nil }}"

## Parsed `def name; ...; end` returns `:name` (Symbol) — pin
## CRuby's documented return value. Pre-fix rubyrs's
## `Op::DefMethodBlock` returned nil; aligned with the
## runtime-dispatch path in #245 round 1.
class DefReturn
  result = define_method(:from_def) { 42 }
  RESULT = result
end
puts "def-return=#{DefReturn::RESULT.inspect}"

## No-block call shapes raise `ArgumentError ("tried to create
## Proc object without a block")` — matches CRuby. Pre-fix the
## explicit-receiver no-block call fell through to
## NoMethodError; bare no-block inside a class body did the
## same. (PR #245 Copilot round 2 #2.)
class NoBlock; end
err = begin
  NoBlock.define_method(:x)
  "DID-NOT-RAISE"
rescue ArgumentError => e
  "ArgumentError: #{e.message}"
end
puts "explicit-no-block=#{err}"

err = begin
  NoBlock.define_method(:x, &nil)
  "DID-NOT-RAISE"
rescue ArgumentError => e
  "ArgumentError: #{e.message}"
end
puts "explicit-nil-block=#{err}"

class BareNoBlock
  err = begin
    define_method(:y)
    "DID-NOT-RAISE"
  rescue ArgumentError => e
    "ArgumentError: #{e.message}"
  end
  puts "bare-no-block=#{err}"
end
