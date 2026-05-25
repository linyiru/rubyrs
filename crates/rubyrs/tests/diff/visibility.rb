# Method visibility — `private` / `protected` / `public` in class
# bodies. The bare form (`private`) switches the mode for any
# subsequent `def`; the with-args form (`private :foo, :bar`)
# retroactively flips the named methods.

class Foo
  def pub_one; "pub-one"; end
  private
  def priv_one; "priv-one"; end
  def priv_two; "priv-two"; end
  public
  def pub_two; "pub-two"; end
  protected
  def prot_one; "prot-one"; end
end

f = Foo.new
puts f.pub_one
puts f.pub_two

# Private methods reject explicit-receiver calls.
begin
  puts f.priv_one
rescue NoMethodError
  puts "blocked priv_one"
end

begin
  puts f.priv_two
rescue NoMethodError
  puts "blocked priv_two"
end

# Implicit-self calls inside the class are allowed (no receiver).
class Bar
  def open
    secret
  end
  def echo
    "[" + secret + "]"
  end
  private
  def secret
    "shh"
  end
end

b = Bar.new
puts b.open
puts b.echo

# The with-args form: `private :method_name, :other` flips
# already-defined methods.
class Baz
  def a; "a"; end
  def b; "b"; end
  def c; "c"; end
  private :b, :c
end

baz = Baz.new
puts baz.a
begin
  puts baz.b
rescue NoMethodError
  puts "blocked b"
end
begin
  puts baz.c
rescue NoMethodError
  puts "blocked c"
end

# `public` re-opens the mode mid-body.
class Mode
  private
  def hidden_one; "h1"; end
  public
  def visible_one; "v1"; end
  private
  def hidden_two; "h2"; end
end

m = Mode.new
puts m.visible_one
begin
  puts m.hidden_one
rescue NoMethodError
  puts "blocked hidden_one"
end
begin
  puts m.hidden_two
rescue NoMethodError
  puts "blocked hidden_two"
end

# `public :sym` un-private's a method.
class Toggle
  def open; "open"; end
  def shut; "shut"; end
  private :open
  public :open
end
puts Toggle.new.open
puts Toggle.new.shut

# Subclassing inherits visibility.
class Parent
  def visible; "parent visible"; end
  private
  def hidden; "parent hidden"; end
end

class Child < Parent
  def use
    hidden
  end
end

c = Child.new
puts c.visible
puts c.use
begin
  puts c.hidden
rescue NoMethodError
  puts "blocked inherited hidden"
end

# `private` at the toplevel is a no-op (we don't define methods
# visible from main, so nothing observable changes).
private
puts "toplevel-private ok"

# Visibility doesn't leak across class bodies.
class A
  private
  def s; "a-s"; end
end
class B
  def t; "b-t"; end
end

begin
  A.new.s
rescue NoMethodError
  puts "A.s blocked"
end
puts B.new.t

# Reopened class — visibility starts fresh as public.
class Resume
  private
  def x; "x"; end
end
class Resume
  # back to public default
  def y; "y"; end
end
puts Resume.new.y
begin
  puts Resume.new.x
rescue NoMethodError
  puts "Resume.x still blocked"
end
