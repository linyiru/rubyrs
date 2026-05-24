name = "world"
puts "hello, #{name}"

n = 42
puts "n is #{n}, twice is #{n * 2}"

def shout(s)
  "#{s}!"
end

puts shout("hi")
puts shout("bye")

a = [1, 2]
b = "x = #{a.length}"
puts b
