# A scoped const reference `Scope::Name` whose `Scope::Name` has a
# pending autoload must fire that autoload — even when `Name` ALSO exists
# as a core toplevel class (Array/Hash/String/…). dry-types' `Types::
# Array` (a zeitwerk autoload shadowing core Array) otherwise bound
# `::Array`, and `::Array.new(SomeClass)` raised a TypeError.
File.write("/tmp/_sca_array.rb", <<~RB2)
  module Scoped
    module Types
      class Array
        def self.tag = "scoped-array"
      end
    end
  end
RB2
module Scoped
  module Types
    autoload :Array, "/tmp/_sca_array.rb"
    class Nominal
      def self.get = Types::Array         # -> Scoped::Types::Array, not ::Array
    end
  end
end
p Scoped::Types::Nominal.get.name
p Scoped::Types::Nominal.get.tag
p Scoped::Types::Nominal.get.equal?(Scoped::Types::Array)
