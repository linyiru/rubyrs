# String#start_with? accepts a Regexp (matched at index 0) and is
# variadic; String#each_byte without a block returns an Enumerator.
p "Hello".start_with?(/[A-Z]/)
p "hello".start_with?(/[A-Z]/)
p "hello".start_with?(/h/)
p "hello".start_with?(/llo/)        # not at start → false
p "hello".start_with?("he", /x/)    # any → true
p "hello".start_with?("xy", "he")
p "hello".start_with?("xy", "z")
p "hello".start_with?
p "abc".each_byte.to_a
p "abc".each_byte.map { |b| b + 1 }
r = []; "ab".each_byte { |b| r << b }; p r
p "x".respond_to?(:each_byte)
