# A CRuby-runnable mirror of examples/brewfile.rs: defines the same
# host functions as Ruby methods, then loads the same Brewfile.rb.
# Used for end-to-end comparison via hyperfine — each invocation
# pays one cold start.

$taps     = []
$formulae = []
$casks    = []
$mas      = []

def tap(name);      $taps     << name        end
def brew(name);     $formulae << name        end
def cask(name);     $casks    << name        end
def mas(name, id);  $mas      << [name, id]  end

load File.join(__dir__, "Brewfile.rb")

# Match the rubyrs example's summary output so the comparison is
# apples-to-apples (same work, same printf).
puts "Collected Brewfile contents:"
printf "  %3d taps\n", $taps.length
printf "  %3d formulae (brew)\n", $formulae.length
printf "  %3d casks\n", $casks.length
printf "  %3d mas apps\n", $mas.length
puts
puts "first 5 brews: #{$formulae.first(5).inspect}"
puts "first 5 casks: #{$casks.first(5).inspect}"
