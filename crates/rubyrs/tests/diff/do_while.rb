# `begin ... end while cond` / `... until cond` — post-condition
# loops. Body runs at least once before the cond is evaluated,
# even when the cond is initially false.

# Body runs once even when cond is false from the start.
ran = false
begin
  ran = true
end while false
puts ran                      # true

# Counted descending — body runs once when initial is past cond.
i = 5
begin
  i = i + 1
end while i < 3
puts i                        # 6

# Looping form — same shape as `while` but tail-tested.
n = 1
total = 0
begin
  total = total + n
  n = n + 1
end while n <= 10
puts total                    # 55

# until variant.
i = 0
begin
  i = i + 1
end until i >= 4
puts i                        # 4

# Inside a method.
class Counter
  def initialize(limit)
    @limit = limit
  end
  def count
    n = 0
    begin
      n = n + 1
    end while n < @limit
    n
  end
end
puts Counter.new(7).count     # 7
puts Counter.new(0).count     # 1 (body runs once)

# Pre-condition `while` still works (regression).
i = 0
while i < 3
  puts "pre-#{i}"
  i = i + 1
end
