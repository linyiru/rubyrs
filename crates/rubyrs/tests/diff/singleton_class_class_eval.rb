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
## CRuby: `A.singleton_class.name` is `nil` even though
## `to_s` / `inspect` render "#<Class:A>". rubyrs stores the
## display name in `Class.name` for diagnostics; the
## `Class#name` primitive detects the shell and returns nil
## to match CRuby. (Code-review #253 round 6 #1.)
puts "shell-name=#{sc1.name.inspect}"

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
## Close the Array over a local so the proc returns the SAME
## object on every call — this is what sinatra's `set
## :add_charset, [...]` actually does (the array literal is
## captured in the setter's closure). Pre-fix this fixture
## passed `proc { [literal] }`, which allocates fresh each
## call and silently masks any mutation regression
## (code-review #253 round 1 #5).
charset_arr = ["application/json", "text/html"]
A5.define_singleton(:add_charset, proc { charset_arr })
puts "sinatra-shape-1=#{A5.default_encoding.inspect}"
puts "sinatra-shape-2=#{A5.add_charset.inspect}"
## `set :add_charset, [...]` then `settings.add_charset << x`
## requires the getter Proc to return an Array that supports
## mutation visible through subsequent reads — the actual
## sinatra/base.rb:1965 idiom `settings.add_charset <<
## %r{^text/}` would fail otherwise.
A5.add_charset << "image/svg+xml"
puts "sinatra-shape-mutation=#{A5.add_charset.inspect}"
puts "sinatra-shape-identity=#{A5.add_charset.equal?(A5.add_charset)}"

## `def name; …; end` inside `singleton_class.class_eval`
## installs as a singleton method (parallel to the
## `define_method` shape above — both routes through
## `install_method`). This PR also fixes the `defining_class`
## plumbing so it points at the underlying real class
## (code-review #253 round 1 #1), but `super` from inside a
## `define_method`-installed block is a separate Tier-1 gap
## not exercised here; once that lands the fixture can pin
## end-to-end super behavior. The Parent6/Child6 setup below
## just verifies the parsed-`def` path on the shell and that
## the parent's existing singleton method is still reachable.
class Parent6
  def self.greet
    "parent"
  end
end
class Child6 < Parent6
end
Child6.singleton_class.class_eval do
  def shouted_greet
    "CHILD"
  end
end
puts "parsed-def-on-shell=#{Child6.shouted_greet.inspect}"
puts "parent-still-reachable=#{Parent6.greet.inspect}"

## `alias_method` inside `singleton_class.class_eval` must
## resolve the source method via the underlying real class's
## singleton-method chain (NOT the shell's empty instance-
## methods table). Pre-fix the source lookup ran
## `lookup_method_uncached(shell, :greet)`, missed, and raised
## NameError. (Code-review #253 round 2 #1.)
class A7
  def self.greet
    "original"
  end
end
A7.singleton_class.class_eval do
  alias_method :hi, :greet
end
puts "alias-on-shell=#{A7.hi.inspect}"
puts "alias-preserves-original=#{A7.greet.inspect}"

## Aliasing a BUILT-IN class method (e.g. `:name`) inside
## `singleton_class.class_eval` must also resolve through the
## underlying real class's primitive whitelist — pre-fix the
## whitelist probe ran with the shell's "#<Class:…>" name and
## never matched. (Code-review #253 round 3 #1.)
class A8; end
A8.singleton_class.class_eval do
  alias_method :class_name, :name
end
puts "alias-builtin-on-shell=#{A8.class_name.inspect}"

## `shell.new` / `shell.allocate` / `shell.new { … }` must
## raise CRuby's TypeError ("can't create instance of
## singleton class"). Pre-fix rubyrs silently allocated an
## orphan instance whose class was the shell — surface
## broken downstream as a confusing NoMethodError on
## anything called on it. (Code-review #253 round 9 #1.)
class A9; end
%i[shell_new shell_allocate shell_new_block].each do |shape|
  err = begin
    case shape
    when :shell_new       then A9.singleton_class.new
    when :shell_allocate  then A9.singleton_class.allocate
    when :shell_new_block then A9.singleton_class.new { :body }
    end
    "DID-NOT-RAISE"
  rescue TypeError => e
    "TypeError: #{e.message}"
  end
  puts "#{shape}=#{err}"
end
