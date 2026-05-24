begin
  puts "before"
  raise "x"
  puts "unreached"
rescue => e
  puts "caught: " + e.to_s
end

def boom
  raise "oops"
end

begin
  boom
rescue => e
  puts "outer: " + e.to_s
end

begin
  begin
    raise "inner"
  rescue => e
    puts "inner: " + e.to_s
    raise "rethrow"
  end
rescue => e
  puts "outer caught: " + e.to_s
end
