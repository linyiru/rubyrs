module Minitest
  module Focus
    def focus(*); end          # marks the next def; harness stub = no-op
  end
  class Test
    extend Focus
  end
end
