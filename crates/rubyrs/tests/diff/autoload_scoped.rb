# Module/Class-level `autoload :Const, "path"` — Phase 2 of the
# autoload story (issue #224). `Mod.autoload :Foo, "p"` registers
# a lazy load; the FIRST reference to `Mod::Foo` requires the
# target and resolves the constant. Rack 3 / Sinatra register
# 40+ of these (`autoload :Response, 'rack/response'`, ...) at
# module-load time; pre-fix every `Rack::Response` reference
# raised `NameError: uninitialized constant`.
#
# The fixture writes its autoload targets to uniquely-named temp
# files (one per shape) so both runtimes load byte-identical
# source. Each filename is unique to THIS fixture, so parallel
# test execution can't collide.

base = "/tmp/rubyrs_autoload_scoped"

# Shape 1: register, then trigger via a qualified reference.
File.write("#{base}_1.rb", "module M1; class Bar; def hi; 'bar-hi'; end; end; end\n")
module M1
  autoload :Bar, "/tmp/rubyrs_autoload_scoped_1.rb"
end
puts "pending_before=#{M1.autoload?(:Bar).inspect}"
puts "triggered=#{M1::Bar.new.hi}"
# After the trigger fires the entry is consumed -> autoload? nil.
puts "pending_after=#{M1.autoload?(:Bar).inspect}"
# The constant is now a normal const; second ref doesn't re-require.
puts "second_ref=#{M1::Bar.new.hi}"

# Shape 2: explicit-receiver registration form.
File.write("#{base}_2.rb", "module M2; VALUE = 41 + 1; end\n")
module M2; end
M2.autoload(:VALUE, "/tmp/rubyrs_autoload_scoped_2.rb")
puts "explicit_pending=#{M2.autoload?(:VALUE).inspect}"
puts "explicit_value=#{M2::VALUE}"

# Shape 3: a never-referenced autoload stays pending and never
# requires its file (no side effects). autoload? still reports it.
module M3
  autoload :Never, "/tmp/rubyrs_autoload_scoped_does_not_exist.rb"
end
puts "never_pending=#{M3.autoload?(:Never).inspect}"

# Shape 4: autoload? for an unregistered constant is nil.
module M4; end
puts "unregistered=#{M4.autoload?(:Nope).inspect}"

# Shape 5: the autoload target can itself define a deeper
# constant the first reference resolves through.
File.write("#{base}_5.rb", "module M5; module Inner; THE = :deep; end; end\n")
module M5
  autoload :Inner, "/tmp/rubyrs_autoload_scoped_5.rb"
end
puts "deep=#{M5::Inner::THE.inspect}"

# Shape 6: string const-name + string path forms both work.
File.write("#{base}_6.rb", "module M6; class Baz; end; end\n")
module M6; end
M6.autoload("Baz", "/tmp/rubyrs_autoload_scoped_6.rb")
puts "string_form=#{M6::Baz.is_a?(Class)}"
