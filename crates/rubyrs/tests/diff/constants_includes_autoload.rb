# Module#constants lists registered-but-unloaded autoloads too (CRuby), and
# Object.constants lists top-level constants (bare-keyed). zeitwerk's reload
# re-arms autoloads; test_reloading checks constants.include? before the
# constant is referenced.
module MzC
  Y = 1
  autoload :Z, File.expand_path("nonexistent_z.rb", __dir__)
end
p MzC.constants.sort                     # [:Y, :Z]

TopConstZc = 42
Object.autoload(:TopAutoZc, File.expand_path("nope.rb", __dir__))
p Object.constants.include?(:TopConstZc)  # true (defined top-level)
p Object.constants.include?(:TopAutoZc)   # true (armed autoload)
p Object.constants.include?(:NoSuchTopZc) # false
