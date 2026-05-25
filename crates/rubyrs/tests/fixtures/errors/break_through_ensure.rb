result = nil
while true
  begin
    break 42
  ensure
    result = "ran"
  end
end
puts result.inspect
