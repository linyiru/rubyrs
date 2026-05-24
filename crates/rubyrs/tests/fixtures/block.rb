sum = 0
[1, 2, 3, 4, 5].each { |x| sum = sum + x }
puts sum

squares = [1, 2, 3, 4].map { |x| x * x }
puts squares.length
puts squares[3]

def shout
  yield "hello"
  yield "world"
end

shout { |w| puts w + "!" }

3.times { |i| puts i }
