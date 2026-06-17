# Module#set_temporary_name (Ruby 3.3+) — name an anonymous module/class.
# Surfaced by sequel (`mod.set_temporary_name(yield)`).
m = Module.new
r = m.set_temporary_name("tmp_name")
p m.name                       # "tmp_name"
p r.equal?(m)                  # true (returns self)
p m.to_s                       # "tmp_name"
m.set_temporary_name(nil)      # clear
p m.name                       # nil

c = Class.new
c.set_temporary_name("widget_klass")
p c.name                       # "widget_klass"
p c.new.class.name             # "widget_klass"

# Not a constant PATH (some segment isn't a constant) → allowed.
p Module.new.tap { |x| x.set_temporary_name("a::b") }.name                  # "a::b"
p Module.new.tap { |x| x.set_temporary_name("Foo::bar") }.name              # "Foo::bar"
p Module.new.tap { |x| x.set_temporary_name("Sequel::SQL::X::_base") }.name # sequel's shape
begin; Module.new.set_temporary_name("Foo_x"); rescue ArgumentError => e; puts e.message; end # a valid const → rejected

# rejections
begin; Module.new.set_temporary_name("Foo"); rescue ArgumentError => e; puts e.message; end
begin; Module.new.set_temporary_name("Foo::Bar"); rescue ArgumentError => e; puts e.message; end
begin; Module.new.set_temporary_name("::X"); rescue ArgumentError => e; puts e.message; end
begin; Module.new.set_temporary_name(""); rescue ArgumentError => e; puts e.message; end
begin; String.set_temporary_name("x"); rescue => e; puts "#{e.class}: #{e.message}"; end
