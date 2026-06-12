# Kernel#sleep user-override gate: a user-defined `sleep` method on
# the receiver's class wins over the Kernel builtin for bare calls
# inside instance methods (CRuby method-resolution order). The
# builtin stays live for everything else — `sleep(0)` returns 0.
# minitest's test_minitest_test.rb stubs sleep on a test instance
# to fake timing without blocking.

class Sleeper
  def sleep(n)
    "slept #{n}"
  end

  def run
    sleep(3)
  end
end

p Sleeper.new.run

class EigenSleeper
  def run
    sleep(2)
  end
end

obj = EigenSleeper.new
def obj.sleep(n)
  "eigen slept #{n}"
end
p obj.run

p sleep(0)
