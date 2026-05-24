# ensure runs on normal completion
begin
  puts "body"
ensure
  puts "ensure"
end

# ensure runs after a rescue
begin
  raise "boom"
rescue => e
  puts "rescued: " + e.message
ensure
  puts "always"
end

# ensure runs even when no rescue catches; exception still propagates
def must_ensure
  begin
    raise "uncaught"
  ensure
    puts "cleanup"
  end
end

begin
  must_ensure
rescue => e
  puts "outer caught: " + e.message
end

# Multiple statements in ensure body
begin
  puts "x"
ensure
  puts "y"
  puts "z"
end
