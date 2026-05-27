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

    ## Conditional call to a method that exists — runs.
    custom_helper if respond_to?(:custom_helper, true)

    ## Conditional call to a method that doesn't exist — skipped.
    nonexistent_method if respond_to?(:nonexistent_method)

    ## Conditional alias when the guard says "don't alias" — skipped.
    alias hi_skipped custom_helper if false

    ## Conditional alias when the guard says "do alias" — installed.
    alias hi_installed custom_helper unless method_defined?(:never_defined)
  end
end

## The unconditional and the if-true path both work; the if-false
## path didn't define the method.
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
