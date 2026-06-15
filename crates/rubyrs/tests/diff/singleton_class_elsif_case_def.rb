# `class << self` body whose `def`s are wrapped in an if/ELSIF/else chain
# or a case/when. The per-statement desugar admits only a single if/else
# of defs and bails on `elsif` / `case`; routing the whole body through
# the real eigenclass-body op installs the taken branch's defs as class
# methods. Surfaced by listen's MonotonicTime on the Bridgetown boot path.
module M
  class << self
    if false
      def kind; "a"; end
    elsif true
      def kind; "b"; end
    else
      def kind; "c"; end
    end
  end
end
p M.kind

module N
  class << self
    case 2
    when 1 then def tag; :one; end
    when 2 then def tag; :two; end
    else def tag; :other; end
    end
  end
end
p N.tag

# elsif chain, later branch taken, plus a method outside the conditional.
module P
  class << self
    if false
      def pick; 1; end
    elsif false
      def pick; 2; end
    elsif true
      def pick; 3; end
    end
    def always; :here; end
  end
end
p P.pick
p P.always
