# Array#join recurses into nested Arrays (CRuby), each nested array
# contributing its own join(sep) — minitest's exception_details
# embeds filter_backtrace(bt) as a nested element and joins.
p [1, [2, [3, 4]], 5].join("|")
p ["a", ["b"], []].join("-")
p [1, [2], 3].join
p ["x", nil, ["y"]].join("-")
a = [1]
a << a
begin
  a.join(",")
rescue ArgumentError => e
  puts "cycle: #{e.message}"
end
