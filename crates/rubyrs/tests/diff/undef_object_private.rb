# undef_method of Object's PRIVATE instance methods (copy + missing hooks),
# which live in native dispatch — must be resolvable, not "undefined method".
# sequel does `undef_method :dup, :clone, :initialize_copy, :initialize_clone,
# :initialize_dup` to disallow copying a Database.
c = Class.new do
  undef_method :initialize_copy, :initialize_clone, :initialize_dup
  undef_method :respond_to_missing?
end
p c.is_a?(Class)                 # true (all undefs succeeded)

# undef'ing dup/clone makes copying raise NoMethodError (sequel's intent)
d = Class.new { undef_method :dup, :clone }
begin; d.new.dup;   rescue NoMethodError; puts "dup undef'd"; end
begin; d.new.clone; rescue NoMethodError; puts "clone undef'd"; end

# a genuinely-undefined name still raises NameError
begin
  Class.new { undef_method :totally_not_a_real_method }
rescue NameError
  puts "NameError"
end
