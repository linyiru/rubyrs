sum = 0
[1, 2, 3, 4, 5].each { |x| sum = sum + x }
puts sum

doubled = [1, 2, 3].map { |x| x * 2 }
puts doubled.length
puts doubled[0]
puts doubled[1]
puts doubled[2]

count = 0
3.times { |i| count = count + i }
puts count

def shout
  yield "a"
  yield "b"
end

shout { |s| puts s + "!" }
