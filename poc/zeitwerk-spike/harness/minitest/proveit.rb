# Harness stub for minitest-proveit: `prove_it!` makes a test class
# require at least one assertion per test. No-op for the harness.
module Minitest
  module ProveIt
    module ClassMethods
      def prove_it!; end
      def proveit!; end
    end
  end
  class Test
    extend ProveIt::ClassMethods
  end
end
