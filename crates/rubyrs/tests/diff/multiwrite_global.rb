## `verbose, $VERBOSE = $VERBOSE, nil` — multi-write with a
## global variable on the LHS. Pre-fix rubyrs's compiler
## rejected `GlobalVariableTargetNode` in the multi-write
## target list with `SyntaxError: unsupported multi-write
## target: GlobalVariableTargetNode(...)`.
##
## Discovery context: rackup-2.2.1's lib/rackup.rb:13 uses
## the canonical "silence Ruby 3.4 deprecation warning"
## idiom — `verbose, $VERBOSE = $VERBOSE, nil` around the
## `require 'webrick'`. sinatra-4 transitively requires
## rackup, so loading `sinatra/base` triggered this. The
## fix threads `MultiWriteTarget::Global` from
## `GlobalVariableTargetNode` through to `Op::StoreGlobal`.
## (TRY_RUNS pass-10 layer #8.)

## Initialise globals explicitly so the diff doesn't depend
## on CRuby's default `$VERBOSE = false` (rubyrs leaves
## unset globals as nil — a separate divergence).
$VERBOSE = false

## Shape 1: the canonical idiom. Old and new values swap via
## a single multi-write.
verbose, $VERBOSE = $VERBOSE, nil
puts "verbose=#{verbose.inspect}"
puts "VERBOSE=#{$VERBOSE.inspect}"

## Shape 2: restore the global from a local.
$VERBOSE = verbose
puts "restored=#{$VERBOSE.inspect}"

## Shape 3: multi-write with two globals on the LHS.
$X = 1; $Y = 2
$X, $Y = $Y, $X
puts "swap-X=#{$X.inspect}"
puts "swap-Y=#{$Y.inspect}"

## Shape 4: globals mixed with locals and ivars in a single
## multi-write target list (in arbitrary order).
class Container
  attr_reader :ix
  def store!
    @ix, $G_HEAD, tail = 99, "head", "tail"
    [tail]
  end
end
c = Container.new
remain = c.store!
puts "ivar=#{c.ix.inspect}"
puts "global=#{$G_HEAD.inspect}"
puts "tail=#{remain.first.inspect}"

## Shape 5: splat + global. The splat target is a local; the
## post-splat target is a global. Confirms the global write
## doesn't mis-interact with `__mw_post` slicing.
a, *m, $TAIL = 1, 2, 3, 4
puts "head=#{a.inspect}"
puts "mid=#{m.inspect}"
puts "tail-glob=#{$TAIL.inspect}"
