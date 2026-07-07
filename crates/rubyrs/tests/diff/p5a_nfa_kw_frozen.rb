# frozen_string_literal: true
# Campaign P5a: the magic comment applies to kw LITERAL Str defaults
# exactly as to any other literal in the file (CRuby evaluates the
# default expression at body entry, where the literal pushes frozen).
# Exercises both the NfaPlan kw serve (bare calls) and the general
# binder's literal-default arm (kwargs-carrying partial fills).

class F
  def probe(s: "x")
    [s.frozen?, s]
  end

  def mutate(s: "x")
    s << "y"
  rescue FrozenError => e
    e.class
  end

  # binder path: `a:` supplied forces CallKw -> general binder,
  # which still literal-fills `s`.
  def partial(a:, s: "lit")
    [a, s.frozen?]
  end
end

f = F.new
30.times { f.probe; f.mutate }
p f.probe
p f.mutate
p f.mutate(s: +"m")          # caller's unfrozen string mutates fine
p f.partial(a: 1)
p f.probe(s: "supplied")     # frozen literal at the CALL site too
