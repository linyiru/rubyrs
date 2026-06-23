# Symbol#start_with? / #end_with? delegate to the string form (CRuby).
# dry-configurable peels a `name=` setter with `name.end_with?("=")`.
p :hello.end_with?("lo")
p :hello.end_with?("x")
p :hello.start_with?("he")
p :hello.start_with?("x")
p :setter=.end_with?("=")
