# Proc#source_location — [file, line] introspection (the rouge-native
# IR compiler uses it to locate rule blocks for AST translation).
p1 = proc { |m| m }
file, line = p1.source_location
puts file.end_with?("proc_source_location.rb")
puts line.is_a?(Integer) && line >= 3 && line <= 4
p2 = lambda do
  :x
end
_, l2 = p2.source_location
puts l2.is_a?(Integer) && l2 >= 7 && l2 <= 8
