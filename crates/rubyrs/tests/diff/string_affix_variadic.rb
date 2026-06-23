# String#start_with? / #end_with? are VARIADIC — true if the string
# starts/ends with ANY argument. start_with? also accepts a Regexp
# (matched at index 0); end_with? takes Strings only.
p "hello".start_with?("he")
p "hello".start_with?("x", "he")
p "hello".start_with?("x", "y")
p "hello".start_with?(/[a-z]/)
p "hello".start_with?
p "hello".end_with?("lo")
p "hello".end_with?("x", "lo")
p "hello".end_with?("x", "y")
p "hello".end_with?
# Symbol delegates to the string form (variadic too)
p :hello.end_with?("x", "lo")
p :hello.start_with?("x", "he")
