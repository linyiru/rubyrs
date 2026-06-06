# Symbol query / case-transform methods operating on the underlying
# name. Discovery: P3 Jekyll spike — forwardable-extended's
# `def_modern_delegator` calls `accessor.empty?` on a Symbol, and
# kramdown / liquid lean on Symbol case helpers.
p :foo.empty?
p :"".empty?
p :foo.length
p :"".length
p :héllo.length       # char count, not bytes
p :foo.size

p :abc.upcase
p :ABC.downcase
p :abc.capitalize
p :aBcD.swapcase
p :"ABC".upcase       # idempotent
p :"".upcase          # empty stays empty

# Round-trips back to a Symbol (not a String).
p :abc.upcase.class
p :abc.upcase == :ABC
