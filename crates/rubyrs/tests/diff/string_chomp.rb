## `String#chomp` / `chomp!` — strip a trailing record
## separator. CRuby semantics: no-arg strips one trailing
## `\r\n` / `\n` / `\r`; arg `""` strips ALL trailing
## newlines (paragraph mode); String arg strips that exact
## suffix if present; nil arg returns self unchanged.
##
## Discovery context: tilt-2.7.0 `StringTemplate#prepare`
## embeds a literal `.chomp` call in the heredoc-wrapped
## source it evals at render time. Pre-fix `String#chomp`
## raised NoMethodError, blocking `tpl.render`.
## (TRY_RUNS pass-10 layer #7.)

## Shape 1: no-arg — strip one trailing newline.
puts "abc\n".chomp.inspect
puts "abc\r\n".chomp.inspect
puts "abc\r".chomp.inspect
puts "abc".chomp.inspect
puts "".chomp.inspect

## Shape 2: only ONE separator is stripped, even on
## multi-newline tails. `chomp` is NOT `rstrip`.
puts "abc\n\n".chomp.inspect
puts "abc\r\n\r\n".chomp.inspect

## Shape 3: explicit String suffix — exact match, no
## per-character stripping.
puts "abcXX".chomp("XX").inspect
puts "abcXX".chomp("X").inspect
puts "abcXX".chomp("YY").inspect
puts "abc".chomp("abc").inspect
puts "abc".chomp("abcd").inspect

## Shape 3b: `"\n"` separator is the universal record
## separator — atomically eats trailing `\r\n`, then bare
## `\n`. Pre-fix the implementation only stripped the
## final `\n`, leaving a stray `\r`. (Copilot review
## #298 round 1.)
puts "abc\r\n".chomp("\n").inspect
puts "abc\n".chomp("\n").inspect
puts "abc\r".chomp("\n").inspect

## Shape 4: `""` (paragraph mode) — strip ALL trailing
## newlines.
puts "abc\n\n\n".chomp("").inspect
puts "abc\r\n\r\n".chomp("").inspect
puts "abc\n".chomp("").inspect
puts "abc".chomp("").inspect

## Shape 5: nil arg — returns receiver unchanged.
puts "abc\n".chomp(nil).inspect

## Shape 6: `chomp!` in-place — returns nil if no change,
## self otherwise.
s = "abc\n"
r = s.chomp!
puts "chomp!-mut=#{s.inspect} ret=#{r.inspect}"
s = "abc"
r = s.chomp!
puts "chomp!-noop=#{s.inspect} ret=#{r.inspect}"
s = "abcXX"
r = s.chomp!("XX")
puts "chomp!-suffix=#{s.inspect} ret=#{r.inspect}"
s = "abcXX"
r = s.chomp!("ZZ")
puts "chomp!-miss=#{s.inspect} ret=#{r.inspect}"

## Shape 6b: `chomp!("\n")` atomically eats trailing
## `\r\n`. (Copilot review #298 round 1.)
s = "abc\r\n"
r = s.chomp!("\n")
puts "chomp!-crlf=#{s.inspect} ret=#{r.inspect}"

## Shape 6c: non-String/non-nil separator raises
## TypeError (not ArgumentError). (Copilot review #298
## round 1.)
err = begin
  "abc".chomp(1)
  "no-raise"
rescue TypeError => e
  e.message.include?("Integer into String") ? "TypeError-Integer" : "TypeError-other-#{e.message}"
end
puts "non-str-arg=#{err}"

err = begin
  "abc".dup.chomp!(1)
  "no-raise"
rescue TypeError => e
  e.message.include?("Integer into String") ? "TypeError-Integer" : "TypeError-other-#{e.message}"
end
puts "non-str-arg!=#{err}"

## Shape 7: `chomp!` on frozen — FrozenError. Per CRuby
## the frozen check fires BEFORE the no-change check, so
## a frozen unchanged receiver also raises.
err = begin
  "abc\n".freeze.chomp!
  "no-raise"
rescue FrozenError => e
  "FrozenError"
end
puts "chomp!-frozen=#{err}"

## Shape 8: respond_to? advertises both.
puts "respond-chomp=#{"".respond_to?(:chomp)}"
puts "respond-chomp!=#{"".respond_to?(:chomp!)}"
