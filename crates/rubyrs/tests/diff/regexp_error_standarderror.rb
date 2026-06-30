# An uncompilable regex raises RegexpError, a StandardError — so
# `rescue`/`rescue StandardError` catches it (RuboCop's per-cop error
# handler relies on this; previously rubyrs raised an uncatchable
# SyntaxError). `Regexp.new("[")` is malformed in CRuby too.
p RegexpError.ancestors.include?(StandardError)
p RegexpError.new("x").is_a?(StandardError)
begin
  Regexp.new("[")
  puts "no-raise"
rescue RegexpError
  puts "caught RegexpError"
rescue => e
  puts "caught via StandardError: #{e.class}"
end
# bare rescue (StandardError) catches it too
result = begin
  Regexp.new("(")
  "no-raise"
rescue
  "bare-rescue-caught"
end
p result
