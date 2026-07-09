# Ticket 2: reopening a natively-served BLOCK collection method on the
# BASE Array/Hash/Range class must win over the native iterator. The
# block-form serve (`collection_call_block`) ran BEFORE the class-chain
# lookup for a PLAIN (untagged) receiver, so a `class Array; def map`
# base reopen was shadowed (subclass overrides and String#gsub reopens
# already worked; the no-BLOCK base reopen already worked too).

class Array; def map; "reopened-map"; end; end
p [1, 2, 3].map { |x| x * 2 }

class Hash; def each; "reopened-each"; end; end
p({ a: 1, b: 2 }.each { |k, v| })

class Array; def select; "reopened-select"; end; end
p [1, 2, 3].select { |x| x > 1 }

class Array; def each; "reopened-aeach"; end; end
p [1, 2].each { |x| x }

# Regression guard: a subclass override still wins (goes through the
# override_tag path, unaffected by the base-reopen gate).
class MyArr < Array; def reject; "sub-reject"; end; end
p MyArr.new(3) { |i| i + 1 }.reject { |x| x.even? }

# Regression guard: the no-BLOCK base reopen keeps working.
class Array; def first(*); "reopened-first"; end; end
p [9, 8, 7].first
