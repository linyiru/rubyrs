# private_constant: explicit `M::X` access raises (even from outside), while
# const_get, bare/lexical reads, and module_eval('X') still work. public_constant
# re-exposes. CRuby semantics.
module M
  X = 1
  Y = 2
  private_constant :X
  def self.read_bare; X; end          # lexical bare read works
end

begin; M::X; rescue NameError => e; puts e.message; end
puts M.const_get(:X)                   # const_get bypasses -> 1
puts M::Y                              # public -> 2
puts M.read_bare                       # lexical -> 1
puts M.module_eval('X')                # module_eval bare -> 1

M.public_constant(:X)
puts M::X                              # re-exposed -> 1
