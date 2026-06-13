# CRuby's case methods (upcase/downcase/capitalize/swapcase + the `!`
# forms) raise `ArgumentError: input string invalid` when the receiver
# holds bytes that are invalid for its encoding — e.g. a UTF-8 string
# carrying a stray \xBF. rack's MethodOverride relies on it
# (`_method.to_s.upcase rescue ArgumentError`). Valid strings (incl.
# multibyte UTF-8) convert normally; this fixture pins both sides.

bad = "\xBF".dup.force_encoding("UTF-8")   # invalid UTF-8
[:upcase, :downcase, :capitalize, :swapcase].each do |m|
  begin
    bad.send(m)
    puts "#{m}: NO RAISE"
  rescue ArgumentError => e
    puts "#{m}: ArgumentError: #{e.message}"
  end
end

# `!` forms raise too (non-frozen receiver).
[:upcase!, :downcase!, :capitalize!, :swapcase!].each do |m|
  s = "\xBF".dup.force_encoding("UTF-8")
  begin
    s.send(m)
    puts "#{m}: NO RAISE"
  rescue ArgumentError => e
    puts "#{m}: ArgumentError"
  end
end

# Valid strings — no raise. ASCII + multibyte UTF-8 (é).
puts "ABC".downcase
puts "abc".upcase
puts "café".upcase
puts "Hello World".swapcase
puts "hELLO".capitalize

# A valid-but-empty string is a no-op (no raise).
puts "".upcase.inspect

# A two-codepoint invalid sequence also raises.
begin
  "a\xC3\x28b".dup.force_encoding("UTF-8").upcase
  puts "mixed: NO RAISE"
rescue ArgumentError
  puts "mixed: ArgumentError"
end
