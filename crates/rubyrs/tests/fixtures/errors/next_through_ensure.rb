result = nil
i = 0
while i < 3
  i = i + 1
  begin
    next
  ensure
    result = "ran"
  end
end
puts result.inspect
