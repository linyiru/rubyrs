## `Proc#arity` — CRuby-shape arity for blocks. Closes
## TRY_RUNS layer #24 — sinatra/base.rb:1810
## (`Sinatra::Base.compile!`) reads `block.arity` to size
## the route block's positional binding list; without this
## method the probe stalls at NoMethodError on `Proc#arity`.
##
## CRuby formula (Tier-1 block params: required + rest only;
## blocks don't support optional or keyword params in rubyrs):
##   has_rest  → -(n_required + 1)
##   else      →  n_required
## The Proto's `rest_param` field isn't populated for blocks
## (rest_slot lives on `BlockHandle`); the arm reads from
## the handle directly. Lock-step with Method#arity is not
## possible without proto-side rest plumbing.

## Shape 1: zero-required + zero-rest — `proc { }` and
## `proc { || }` both report 0.
puts "empty=#{proc { }.arity}"
puts "empty-bars=#{proc { || }.arity}"

## Shape 2: pure required — N for N params.
puts "one-req=#{proc { |a| a }.arity}"
puts "two-req=#{proc { |a, b| [a, b] }.arity}"
puts "three-req=#{proc { |a, b, c| [a, b, c] }.arity}"

## Shape 3: pure rest — -1 for `|*a|`, -1 for anonymous `|*|`.
puts "rest=#{proc { |*a| a }.arity}"
puts "rest-anon=#{proc { |*| }.arity}"

## Shape 4: required + rest — `-(n_required + 1)`.
puts "one-req-rest=#{proc { |a, *b| [a, b] }.arity}"
puts "two-req-rest=#{proc { |a, b, *c| [a, b, c] }.arity}"

## Shape 5: lambda (uses the same arity arm — the Proto and
## BlockHandle are the same shape).
puts "lambda-empty=#{lambda { }.arity}"
puts "lambda-one=#{lambda { |a| a }.arity}"
puts "lambda-rest=#{lambda { |*a| a }.arity}"

## Shape 6: `respond_to?(:arity)` returns true on Procs — used
## by feature-detection idioms before the actual call.
puts "respond-to=#{proc { }.respond_to?(:arity)}"

## Shape 7: a Proc stored in a local round-trips through
## arity — sinatra's `block.arity` for a route handler
## defined as `get('/'){ ... }` works regardless of whether
## the handler runs inline or via a stored Proc.
square = proc { |n| n * n }
puts "stored-proc=#{square.arity}"
puts "stored-proc-call=#{square.call(5)}"

## Shape 8: arity is callable inside conditional logic —
## sinatra's compile! does `if block.arity == 0` to detect
## bare handlers.
def shape_of(blk)
  case blk.arity
  when 0 then "bare"
  when 1 then "single-arg"
  when -1 then "splat"
  else "other(#{blk.arity})"
  end
end

puts "shape-bare=#{shape_of(proc { })}"
puts "shape-single=#{shape_of(proc { |x| x })}"
puts "shape-splat=#{shape_of(proc { |*args| args })}"
puts "shape-other=#{shape_of(proc { |a, b| })}"

## `Proc#arity` rejects any args — ArgumentError shape
## matches CRuby (Copilot review #263 round 1).
err = begin; proc { }.arity(1); "DID-NOT-RAISE"; rescue ArgumentError => e; e.message; end
puts "arity-with-arg=#{err}"
