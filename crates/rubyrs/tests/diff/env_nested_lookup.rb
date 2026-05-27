## Bare `ENV` reference from inside a nested class/module body
## must walk up the constant-lookup chain to the toplevel ENV
## instead of failing as `uninitialized constant
## Container::Sub::Inner::ENV`. Closes TRY_RUNS pass-9.7
## layer #20 — sinatra/base.rb:1940 does
##   set :environment, (ENV['APP_ENV'] || ENV['RACK_ENV'] ||
##                      :development).to_sym
## inside `class Base; ...; end` (nested in `module Sinatra`),
## so the bare `ENV` reference compiles to LoadConstChain
## walking `[Sinatra::Base::ENV, Sinatra::ENV, ENV]`. Before
## the fix, the chain walk only consulted `vm.classes` /
## `vm.constants` and didn't know about the lazy-build ENV
## intercept that `Op::LoadConst("ENV")` has had since ADR
## 0017. Bare `ENV` at toplevel worked, but ANY nested
## reference raised NameError.
##
## Fix: extracted the ENV lazy-build into a shared
## `Vm::env_hash_or_init` helper; `Op::LoadConstChain`'s
## fallback now invokes it when the chain's bare-name (last
## entry) is "ENV" before raising NameError.

## Toplevel ENV still works (sanity check that the refactor
## didn't break the existing intercept). Assert on a behavior
## both CRuby and rubyrs agree on — rubyrs's ENV is a real
## Hash; CRuby's ENV is an instance of a magic Object with
## Hash-like methods (documented divergence per ADR 0017),
## so .class.name diverges. Both agree on `respond_to?(:[])`
## and on missing-key returning nil.
puts "toplevel-respond-getter=#{ENV.respond_to?(:[])}"
puts "toplevel-missing-key=#{ENV['DEFINITELY_NOT_SET_AAAAA'].inspect}"

## Bare ENV inside a nested class body — pre-fix this raised
## "uninitialized constant Container::ENV". Now resolves to
## the same toplevel ENV.
class Container
  module Sub
    class Inner
      ## A specific ENV[] read at body load time — the
      ## sinatra-shape access pattern. Returns nil for
      ## non-existent keys; both interpreters agree.
      ENV_PROBE = ENV['DEFINITELY_NOT_SET_AAAAA_BBBBB']
      RESPOND_GETTER = ENV.respond_to?(:[])
    end
  end
end

puts "nested-respond-getter=#{Container::Sub::Inner::RESPOND_GETTER}"
puts "nested-probe=#{Container::Sub::Inner::ENV_PROBE.inspect}"

## Sanity: the ENV Hash is the SAME object whether referenced
## bare at toplevel or via the nested-class chain walk. Pin
## via Hash#equal? (identity, not value equality).
class IdentityCheck
  module Deeper
    SAME_REF = ENV
  end
end
puts "identity-same=#{ENV.equal?(IdentityCheck::Deeper::SAME_REF)}"

## Negative case: a truly missing constant inside the nested
## chain still raises NameError. The ENV intercept is the
## ONLY name-specific fallback in the chain walker.
class Missing
  module Inside
    err = begin
      NoSuchToplevelConst_AAAA
      "did-not-raise"
    rescue NameError
      "NameError"
    end
    puts "missing-const=#{err}"
  end
end
