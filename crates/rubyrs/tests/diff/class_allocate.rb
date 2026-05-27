## `Class#allocate` — bare-instance allocator without calling
## `initialize`. Used by frameworks for unmarshalling / dup /
## clone / ORM hydration, and surfaced by the TRY_RUNS pass-7
## probe's `ERB.new` stub (layer #4 — the only remaining Cat H
## gap before this PR).

## allocate produces an instance whose class is the receiver,
## with NO initialize call (no side-effects from the ctor).
class Box
  def initialize(x); @value = x; @initialized = true; end
  def value; @value; end
  def initialized?; @initialized; end
end

a = Box.allocate
puts "class=#{a.class.name}"
puts "no-init-marker=#{a.instance_variable_get(:@initialized).inspect}"
puts "no-value=#{a.instance_variable_get(:@value).inspect}"
puts "no-ivars=#{a.instance_variables.empty?}"

## Compare against new: ivars set by initialize.
b = Box.new(42)
puts "new-value=#{b.value}"
puts "new-init-marker=#{b.initialized?}"

## Allocated instance can be hydrated via instance_variable_set
## (the common use case — unmarshalling / Marshal#load / ORM
## record hydration).
a.instance_variable_set(:@value, "hydrated")
puts "hydrated=#{a.value}"

## Primitive class shells raise TypeError matching CRuby's
## "allocator undefined for X". Pin both class and message.
## KNOWN GAP: CRuby additionally allows allocate on String /
## Array / Hash / Range (producing bare instances of the
## builtin), which rubyrs does not yet route through the
## TypedData allocator. Out of scope here — pass-7 layer #4
## only needs user-class allocate to unblock ERB-style stubs.
[Integer, Float, Symbol,
 TrueClass, FalseClass, NilClass, Proc].each do |k|
  begin
    k.allocate
    puts "#{k.name}.allocate=NOT-RAISED"
  rescue TypeError => e
    puts "#{k.name}.allocate=#{e.class}: #{e.message}"
  end
end

## Module shells reject allocate. CRuby raises NoMethodError on
## these; rubyrs approximates with TypeError ("allocator undefined
## for X") as a safe fence until a proper Module/Class allocator
## lands. Locked in here so the bare-Instance regression (where
## `Module.new.allocate` returned a meaningless `#<>`) stays
## fixed; the test pins the rubyrs surface, NOT CRuby's exact
## NoMethodError shape (see KNOWN GAP in the dispatch arm).
## Print only "raised + non-empty message" — rubyrs uses
## TypeError, CRuby uses NoMethodError; bridging the class name
## is out of scope. The key invariant is that SOMETHING is
## raised (no more bogus bare-Instance leaks).
m = Module.new
begin; m.allocate; rescue StandardError => e
  puts "Module.new.allocate=raised:#{!e.message.empty?}"
end
begin; Module.allocate; rescue StandardError => e
  puts "Module.allocate=raised:#{!e.message.empty?}"
end

## Wrong-arity: zero args expected.
begin
  Box.allocate(1)
rescue ArgumentError => e
  puts "wrong-arity=#{e.class}: #{e.message}"
end

## respond_to? — every Class responds_to allocate, even ones
## whose actual call would raise TypeError (matches CRuby's
## "method exists, but allocator may be undefined" surface).
puts "respond-user=#{Box.respond_to?(:allocate)}"
puts "respond-int=#{Integer.respond_to?(:allocate)}"

## Class can be reopened after allocate; the allocated instance
## sees the new method (allocate vs subsequent re-open ordering
## is irrelevant — method lookup is dynamic).
class Box
  def hi; "hi-from-box"; end
end
puts "reopened=#{a.hi}"
