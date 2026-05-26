# Enumerable — stub class (we don't have real Modules in this
# subset). CRuby's Enumerable defines ~50 methods
# (each_with_index, map, select, reject, inject, sort, to_a, ...)
# all in terms of a host class's `#each`. For built-in
# collections (Array/Hash/Range), iteration methods are wired in
# `vm/iter.rs`'s block-dispatch paths, not via Enumerable
# include; for user classes the host provides `def each`
# directly. Either way, the Enumerable-derived methods aren't
# automatically gained through an empty stub.
#
# Why keep the stub anyway: `class Foo; include Enumerable; def
# each; ...; end; end` (commonly executed while loading a class
# body, but also supported at arbitrary runtime points and via
# the explicit `Foo.include(Enumerable)` form) pushes Enumerable
# onto Foo's `includes` chain (vm/dispatch.rs's include arm;
# lookup walks the chain at method-dispatch time, no copy).
# Empty Enumerable adds nothing to dispatch but doesn't crash.
# Before this stub, `include Enumerable` raised "wrong argument
# type NilClass (expected Module)" and the file failed to load.
# Affected: rake/linked_list.rb at minimum (Plan A try-run
# target), plus any other codebase that does the same
# `include Enumerable + def each` pattern. Methods like `.map`
# on a user `LinkedList` instance still NoMethodError at call
# time — documented divergence, follow-up PR.

class Enumerable
end
