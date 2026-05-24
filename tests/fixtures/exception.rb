begin
  puts "before"
  raise "something went wrong"
  puts "unreached"
rescue => e
  puts "caught: " + e
end

def boom
  raise "oops"
end

begin
  boom
rescue => e
  puts "outer: " + e
end

# nested
begin
  begin
    raise "inner"
  rescue => e
    puts "inner caught: " + e
    raise "rethrow"
  end
rescue => e
  puts "outer caught: " + e
end

# no exception path
begin
  x = 1 + 2
  puts x
rescue => e
  puts "should not print"
end
