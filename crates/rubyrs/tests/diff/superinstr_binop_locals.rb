# Exercises the `<local> <op> <local>` superinstruction fusion
# (Op::BinOpLocalLocal). Each `a <op> b` where both a and b are
# locals compiles to one fused op; this fixture proves the fused
# op is byte-identical to LoadLocal+LoadLocal+BinOp across every
# operand type and operator the fallback chain must cover.

# --- Int x Int: every operator, including comparisons -----------
a = 17
b = 5
puts a + b
puts a - b
puts a * b
puts a / b
puts a % b
puts(a < b)
puts(a <= b)
puts(a > b)
puts(a >= b)
puts(a == b)
puts(a != b)
puts(a == a)

# floor-division / modulo with negatives (Ruby floor semantics)
x = -7
y = 3
puts x / y
puts x % y

# --- Int overflow promotion (apply_int -> BigInt) ---------------
big1 = 4_000_000_000
big2 = 3_000_000_000
puts big1 * big2
puts big1 + big2

# --- Float x Float and mixed Int/Float --------------------------
f1 = 3.5
f2 = 2.0
puts f1 + f2
puts f1 * f2
puts f1 / f2
puts(f1 > f2)
puts a + f1
puts f1 + a
puts(a > f1)

# --- Rational x Rational (and mixed) ----------------------------
r1 = Rational(1, 2)
r2 = Rational(1, 3)
puts r1 + r2
puts r1 * r2
puts(r1 > r2)
puts r1 + a

# --- String x String (primitive_call path) ----------------------
s1 = "foo"
s2 = "bar"
puts s1 + s2
puts(s1 < s2)
puts(s1 == s2)
puts(s1 == s1)
puts s1 * 3 == s1 + s1 + s1 ? "ok" : "no"

# --- Array x Array (primitive_call path) ------------------------
arr1 = [1, 2]
arr2 = [3, 4]
p arr1 + arr2
p(arr1 == arr2)

# --- User-defined operators (fall to do_call) -------------------
class Vec
  attr_reader :v
  def initialize(v) = @v = v
  def +(o) = Vec.new(@v + o.v)
  def <(o) = @v < o.v
  def ==(o) = @v == o.v
  def to_s = "Vec(#{@v})"
end
u1 = Vec.new(10)
u2 = Vec.new(20)
puts((u1 + u2).to_s)
puts(u1 < u2)
puts(u1 == u2)

# user class with <=> via Comparable
class Money
  include Comparable
  attr_reader :cents
  def initialize(c) = @cents = c
  def <=>(o) = cents <=> o.cents
end
m1 = Money.new(100)
m2 = Money.new(250)
puts(m1 < m2)
puts(m1 > m2)
puts(m1 == m2)

# --- In-loop fusion: the hot `i < n` shape ----------------------
i = 0
n = 10
total = 0
while i < n
  total = total + i
  i = i + 1
end
puts total

# nested two-local comparison driving a conditional
lo = 3
hi = 7
count = 0
j = 0
while j < hi
  count = count + 1 if j > lo
  j = j + 1
end
puts count

# --- ZeroDivisionError parity (Div/Mod local/local guard) -------
num = 10
zero = 0
begin
  puts num / zero
rescue ZeroDivisionError => e
  puts "div: #{e.message}"
end
begin
  puts num % zero
rescue ZeroDivisionError => e
  puts "mod: #{e.message}"
end

# --- Error parity for incompatible operands ---------------------
# (Int + String raises; the fused op must take the same do_call
# fallback the unfused sequence would. We assert an error was
# raised rather than its exact class — rubyrs's subset raises
# NoMethodError where CRuby raises TypeError, an orthogonal gap.)
begin
  num_local = 1
  str_local = "x"
  puts num_local + str_local
  puts "no error"
rescue StandardError
  puts "err raised"
end
