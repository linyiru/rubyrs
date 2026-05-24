name = "Ruby"
year = 1995
puts "Hello, #{name}! Born #{year}."

# Symbol basics
s = :foo
puts s
puts s.to_s
puts :bar == :bar
puts :bar == :baz

# Hash with symbol keys
h = {name: "Ruby", year: 1995, author: :matz}
puts h[:name]
puts h[:author]
puts h[:author].to_s

# Interpolation with method call + math
def shout(w)
  "#{w}!"
end
puts shout("hi")
x = 5
puts "x is #{x}, x*2 is #{x * 2}"

# Interpolation inside an array element
items = ["one", "two #{1 + 1}", "three"]
items.each { |i| puts i }

# Empty interpolation
puts "x = #{}"
