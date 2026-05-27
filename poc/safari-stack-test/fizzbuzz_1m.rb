# 1M iteration fizzbuzz microbench. Used as the headline arithmetic /
# dispatch-loop benchmark in docs/BENCHMARKS.md; the script body
# stresses Op::BinOpInt, Op::IncLocal, method dispatch, and string
# concat via to_s. Result printed at end is a sanity-check digest so
# the comparison harness can spot incorrect optimisation that would
# change output.
def fizzbuzz(n)
  if    n % 15 == 0 then "FizzBuzz"
  elsif n % 3  == 0 then "Fizz"
  elsif n % 5  == 0 then "Buzz"
  else n.to_s end
end

i = 1; acc = 0
while i <= 1000000
  acc = acc + fizzbuzz(i).length
  i = i + 1
end
puts acc
