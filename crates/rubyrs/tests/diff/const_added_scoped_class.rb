# Module#const_added (Ruby 3.2+) fires on the PARENT module the moment a
# constant is defined — including the compact `class A::B` / `module A::B`
# form, where the owner is the resolved parent A and the cname is the short
# last component B. zeitwerk's namespace child-autoload setup relies on this
# (it Module.prepends a const_added). The bare nested form fires on the
# lexical scope. Reopening a class does NOT re-fire (first definition only).
class M
  def self.const_added(name)
    puts "const_added: #{name}"
  end
end

class M::Foo; end        # compact -> M.const_added(:Foo)
module M::Bar; end       # compact -> M.const_added(:Bar)

class M
  class Baz; end         # bare nested -> M.const_added(:Baz)
end

class M::Foo; end        # reopen -> no re-fire

puts M.constants.sort.inspect
