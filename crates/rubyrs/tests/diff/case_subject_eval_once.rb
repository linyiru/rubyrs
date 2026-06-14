# A `case <expr>` evaluates <expr> exactly ONCE, then matches each `when`
# against the result with `===`. A side-effecting subject must not be
# re-run per `when` (rack's multipart parser does `case consume_boundary`
# where consume_boundary advances a StringScanner — re-evaluation
# corrupted the parse).

$calls = 0
def subject
  $calls += 1
  :b
end

r = case subject
    when :a then "A"
    when :b then "B"
    when :c then "C"
    else "ELSE"
    end
puts "result=#{r} calls=#{$calls}"     # result=B calls=1

# matching the LAST when / else still evaluates the subject once
$calls = 0
r2 = case subject
     when :x then "X"
     when :y then "Y"
     else "DEFAULT"
     end
puts "result=#{r2} calls=#{$calls}"    # result=DEFAULT calls=1

# a subject with a visible side effect: an array consumed via shift
seq = [10, 20, 30]
out = []
3.times do
  out << case seq.shift            # shift must run once per iteration, not per when
         when 10 then "ten"
         when 20 then "twenty"
         when 30 then "thirty"
         else "?"
         end
end
p out                              # ["ten", "twenty", "thirty"]
p seq                              # [] (fully consumed, one shift per case)

# ranges / classes as when-conditions still work (=== semantics)
def classify(n)
  case n
  when 0..9 then "low"
  when 10..99 then "mid"
  else "high"
  end
end
p [classify(5), classify(50), classify(500)]   # ["low", "mid", "high"]

# multi-value when (a, b) still matches via === on the single subject
def kind(x)
  case x
  when Integer, Float then "number"
  when String then "string"
  else "other"
  end
end
p [kind(1), kind(1.5), kind("s"), kind(:sym)]   # ["number","number","string","other"]
