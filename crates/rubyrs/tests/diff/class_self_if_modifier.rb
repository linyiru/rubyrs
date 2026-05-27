## `class << self` body now accepts modifier-form `if` / `unless`
## wrapping a single supported inner stmt. Closes TRY_RUNS
## pass-9.5 layers #13 + #15 — sinatra/base.rb:1590 uses
## `ruby2_keywords(:use) if respond_to?(:ruby2_keywords, true)`
## and line 1659 uses `alias new! new unless method_defined? :new!`.
## Both are the "Ruby 2.7 / cross-version compat guard" pattern.
##
## Recognised inner shapes (scope deliberately narrow):
##   - bare-call (CallNode) — the `ruby2_keywords(:use)` case
##   - `alias new old` (AliasMethodNode) — the
##     `alias new! new unless method_defined? :new!` case
## Other shapes wrapped in if/unless fall through to
## NotImplementedError; the recogniser can widen on demand.

class WithIfMod
  class << self
    def custom_helper
      "from-helper"
    end

    ## Conditional call with literal-true guard — RUNS.
    ## Observable side effect: prints at class-body load time.
    ## Both CRuby and rubyrs evaluate `if true` identically;
    ## any divergence here would surface as a missing or
    ## duplicated "guard-true-fired" line. (Earlier draft of
    ## this fixture used `respond_to?(:custom_helper, true)`
    ## as the guard, but inside `class << self` body
    ## CRuby's `respond_to?` for the just-`def`-ed method
    ## returns false — instance method on the singleton
    ## class, not method on the singleton-class object —
    ## while rubyrs returns true. Avoiding the divergent
    ## guard keeps the fixture focused on the if-modifier
    ## wrapping itself.)
    puts "guard-true-fired" if true

    ## Conditional call with literal-false guard — SKIPPED.
    ## If the modifier admitted both branches by accident,
    ## a second "guard-false-fired" line would appear and
    ## the diff harness would catch it.
    puts "guard-false-fired" if false

    ## Conditional alias when the guard says "don't alias" — skipped.
    alias hi_skipped custom_helper if false

    ## Conditional alias when the guard says "do alias" — installed.
    alias hi_installed custom_helper unless false
  end
end

## The unconditional and the if-true path both work; the if-false
## path didn't define the method.
## (The if-true side effect — `helper-called-at-load-time` —
## already printed during class-body load above. If absent
## from rubyrs's stdout, the guard mis-evaluated and the diff
## harness would catch it.)

puts "installed=#{WithIfMod.hi_installed}"
puts "skipped-respond=#{WithIfMod.respond_to?(:hi_skipped)}"
skipped_direct = begin
  WithIfMod.hi_skipped
rescue NoMethodError
  "NoMethodError"
end
puts "skipped-direct=#{skipped_direct}"

## The conditional-call to nonexistent_method skipped silently;
## demonstrate that body load didn't crash.
puts "body-loaded=true"
