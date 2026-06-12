# raise SomeClass stamps @message with the class name (CRuby shape).
begin; raise ArgumentError; rescue => e; p e.message; end
begin; raise TypeError; rescue => e; p e.message; end
# SyntaxError exists and sits under ScriptError (not StandardError):
p SyntaxError.ancestors.include?(ScriptError)
begin
  begin
    raise SyntaxError, "icky"
  rescue StandardError
    puts "wrong"
  end
rescue SyntaxError => e
  p e.message
end
# block frames report 'block in <method>' / 'block in <main>'.
# CRuby 3.4 qualifies with the receiver class ('block in
# Object#outer'); Tier-1 reports the bare method name — normalize
# the qualifier away (documented divergence; minitest's own
# comparisons do the same kind of normalization).
def outer
  [1].each { raise "boom" }
end
begin; outer; rescue => e; puts e.backtrace.first.sub(/.*:in /, "").sub("Object#", ""); end
begin
  [2].each { raise "top" }
rescue => e
  puts e.backtrace.first.sub(/.*:in /, "")
end
