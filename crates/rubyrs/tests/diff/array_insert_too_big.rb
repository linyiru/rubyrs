# Array#insert with an index past the array-size limit raises a
# catchable IndexError ("index N too big") instead of driving an
# aborting multi-exabyte allocation on a default-config interpreter.
begin
  [].insert(2**62, 1)
rescue IndexError => e
  puts "IndexError: #{e.message}"
end

# Normal padding / negative-index inserts are unaffected.
p [1, 2].insert(5, :x)
p [1, 2, 3].insert(-2, :y)
