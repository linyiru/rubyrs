# Module#const_added (Ruby 3.2+) fires on constant ASSIGNMENT too — not only
# class/module definitions. `C = Class.new`, `M::X = v`, and a bare `X = v`
# inside a class body all notify the owning module. zeitwerk's nsfile
# namespaces are defined as `Widget = Class.new`, so its prepended const_added
# must fire here to set up child autoloads. First definition only.
class M
  def self.const_added(name)
    puts "const_added: #{name}"
  end
end

M::Foo = Class.new      # -> M.const_added(:Foo)
M::BAR = 42             # -> M.const_added(:BAR)
class M
  Baz = "baz"           # bare in body (qualified M::Baz) -> M.const_added(:Baz)
end

puts M.constants.sort.inspect
