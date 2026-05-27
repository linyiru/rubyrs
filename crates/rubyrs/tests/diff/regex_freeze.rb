## `Regexp#freeze` — compatibility no-op. Regex values are
## immutable by construction (no mutating instance methods),
## so freeze has nothing to enforce, but CRuby still defines
## the method so user code's `/pat/.freeze` doesn't trip.
## TRY_RUNS pass-7 layer #5 — sinatra/base.rb:32 has
## `HEADER_PARAM = /.../.freeze` at class body load, which
## previously raised `NoMethodError: undefined method 'freeze'
## for Regexp` and blocked all of sinatra/base.rb body
## execution past that line.

## freeze on a literal: returns the receiver for chaining.
r = /foo/
ret = r.freeze
puts "ret-eq-r: #{ret.equal?(r)}"

## frozen? returns true regardless (Regexp is immutable
## by construction, so this is the right answer semantically).
puts "frozen?: #{r.frozen?}"
puts "fresh-frozen?: #{/bar/.frozen?}"

## respond_to? agrees with the primitive arm.
puts "respond-freeze: #{r.respond_to?(:freeze)}"
puts "respond-frozen?: #{r.respond_to?(:frozen?)}"

## The actual sinatra pattern: assign-and-freeze inside a
## class body (the line that was previously the third
## blocker in TRY_RUNS pass 7).
class HeaderHolder
  HEADER_PARAM = /\s*[\w.]+=(?:[\w.]+|"(?:[^"\\]|\\.)*")?\s*/.freeze
end
puts "sinatra-pattern-match: #{HeaderHolder::HEADER_PARAM.match?('a=1')}"
puts "sinatra-pattern-frozen?: #{HeaderHolder::HEADER_PARAM.frozen?}"

## Wrong-arity: CRuby raises ArgumentError, not NoMethodError.
## Distinct from "the method exists at all" — pin both the
## class (ArgumentError) and the standard "wrong number of
## arguments" shape so any drift surfaces in the diff.
begin
  r.freeze(:extra)
  puts "wrong-arity=NOT-RAISED"
rescue ArgumentError => e
  puts "wrong-arity=#{e.class}: #{e.message}"
end
