## `Class#singleton_class` builds a lazy eigenclass shell.
## Method installs inside `singleton_class.class_eval { … }`
## land on the underlying class's singleton-methods table so
## `Cls.method_name` dispatch finds them. Closes TRY_RUNS
## pass-9.7d layer #23 — sinatra/base.rb:1735's
## `define_singleton` (used by `set`) builds method installers
## via this idiom; pre-fix the methods landed on the
## instance-methods table because `singleton_class` was a
## Tier-1 stub that returned the receiver itself.

## Shape 1: `define_method` inside `singleton_class.class_eval`
## installs a class-level (singleton) method.
class A1; end
A1.singleton_class.class_eval do
  define_method(:greet) { |name| "hi-#{name}" }
end
puts "define-method=#{A1.greet("world").inspect}"
puts "respond-to-singleton=#{A1.respond_to?(:greet)}"

## Shape 2: parsed `def name; …; end` inside
## `singleton_class.class_eval` also installs as a class
## method (sinatra's `class_eval("def #{name}() …; end")`
## fallback in `define_singleton`).
class A2; end
A2.singleton_class.class_eval do
  def shout(s)
    s.upcase
  end
end
puts "def-keyword=#{A2.shout("rubyrs").inspect}"
puts "respond-to-def=#{A2.respond_to?(:shout)}"

## Shape 3: `singleton_class` identity — repeated calls return
## the same Class object (CRuby invariant). The view is cached
## on the underlying class's `singleton_view` slot.
class A3; end
sc1 = A3.singleton_class
sc2 = A3.singleton_class
puts "identity=#{sc1.equal?(sc2)}"
puts "view-class=#{sc1.class.name}"

## Shape 4: the eigenclass shell is NOT the class itself —
## installs on the shell DON'T land on the underlying class's
## instance-methods table, only on its singleton table.
class A4
  def already_here
    "instance"
  end
end
A4.singleton_class.class_eval do
  define_method(:on_class) { "class-side" }
end
puts "instance-side-not-affected=#{A4.new.already_here.inspect}"
puts "class-side=#{A4.on_class.inspect}"
err = begin
  A4.new.on_class
  "DID-NOT-RAISE"
rescue NoMethodError
  "NoMethodError"
end
puts "no-instance-leak=#{err}"

## Shape 5: sinatra's full `define_singleton` idiom — a
## class-level proxy that takes a name + Proc and installs
## a reader. Pin the actual sinatra usage shape end-to-end.
class A5
  def self.define_singleton(name, content)
    singleton_class.class_eval do
      define_method(name, &content)
    end
  end
end
A5.define_singleton(:default_encoding, proc { "utf-8" })
A5.define_singleton(:add_charset, proc { ["application/json", "text/html"] })
puts "sinatra-shape-1=#{A5.default_encoding.inspect}"
puts "sinatra-shape-2=#{A5.add_charset.inspect}"
## `set :add_charset, [...]` then `settings.add_charset << x`
## requires the getter Proc to return an Array that supports
## mutation visible through subsequent reads.
A5.add_charset << "image/svg+xml"
puts "sinatra-shape-mutation=#{A5.add_charset.inspect}"
