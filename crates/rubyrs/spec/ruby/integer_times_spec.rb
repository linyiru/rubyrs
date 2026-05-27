# Adapted from ruby/spec core/integer/times_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-polished:
# - `.should ==` → `assert_eq`.
# - skipped (method-not-implemented): the no-block `Enumerator`
#   cases (the final `it "returns an Enumerator"` and the nested
#   `Enumerator#size` block). rubyrs's `Integer#times` requires a
#   block; without one it raises NoMethodError rather than
#   returning an Enumerator. Tracked separately as part of the
#   Enumerator surface (out of B.6 scope).

describe "Integer#times" do
  it "returns self" do
    assert_eq(5.times {}, 5)
    assert_eq(9.times {}, 9)
    assert_eq(9.times { |n| n - 2 }, 9)
  end

  it "yields each value from 0 to self - 1" do
    a = []
    9.times { |i| a << i }
    -2.times { |i| a << i }
    assert_eq(a, [0, 1, 2, 3, 4, 5, 6, 7, 8])
  end

  it "skips the current iteration when encountering 'next'" do
    a = []
    3.times do |i|
      next if i == 1
      a << i
    end
    assert_eq(a, [0, 2])
  end

  it "skips all iterations when encountering 'break'" do
    a = []
    5.times do |i|
      break if i == 3
      a << i
    end
    assert_eq(a, [0, 1, 2])
  end

  it "skips all iterations when encountering break with an argument and returns that argument" do
    assert_eq((9.times { break 2 }), 2)
  end

  it "executes a nested while loop containing a break expression" do
    a = [false]
    b = 1.times do |i|
      while true
        a.shift or break
      end
    end
    assert_eq(a, [])
    assert_eq(b, 1)
  end

  it "executes a nested #times" do
    a = 0
    b = 3.times do |i|
      2.times { a += 1 }
    end
    assert_eq(a, 6)
    assert_eq(b, 3)
  end

  # skipped (method-not-implemented): the no-block Enumerator
  # surface. rubyrs's `Integer#times` requires a block; `5.times`
  # without one raises NoMethodError instead of returning an
  # Enumerator. Out of B.6 scope (Enumerator is a separate
  # rampup item).
  #
  # it "returns an Enumerator" do
  #   result = []
  #   enum = 3.times
  #   enum.each { |i| result << i }
  #   assert_eq(result, [0, 1, 2])
  # end
  #
  # describe "when no block is given" do
  #   describe "returned Enumerator" do
  #     describe "size" do
  #       it "returns self" do
  #         assert_eq(5.times.size, 5)
  #         assert_eq(10.times.size, 10)
  #         assert_eq(0.times.size, 0)
  #       end
  #     end
  #   end
  # end
end
