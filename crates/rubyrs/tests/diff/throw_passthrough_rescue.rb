# throw must fly past intervening rescue clauses (CRuby: a non-local
# jump, not a StandardError) — minitest's assert_throws wraps the
# yield in `rescue ArgumentError` to DETECT wrong-tag throws and a
# rescuable carrier broke every assert_throws.
caught = true
value = catch(:boom) do
  begin
    throw :boom
  rescue ArgumentError => e
    puts "wrongly-rescued"
  end
  caught = false
end
p [caught, value]
# value-carrying throw through a rescue sandwich
r = catch(:t) do
  begin
    throw :t, 42
  rescue StandardError
    puts "no"
  end
end
p r
# wrong-tag throw IS an ArgumentError (UncaughtThrowError) at the site
catch(:a) do
  begin
    throw :b
  rescue ArgumentError => e
    puts "site: #{e.message}"
  end
end
# nested cross-tag, ensure ordering
r2 = catch(:outer) do
  catch(:inner) do
    begin
      throw :outer, "jumped"
    ensure
      puts "ensure-ran"
    end
  end
  "not-reached"
end
p r2
# uncaught at top of a method propagates as UncaughtThrowError
def t3
  throw :nope
end
begin
  t3
rescue UncaughtThrowError => e
  p [e.tag, e.value]
end
puts "done"
