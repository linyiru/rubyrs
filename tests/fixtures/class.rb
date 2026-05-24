class Counter
  def initialize(start)
    @count = start
  end

  def inc
    @count = @count + 1
  end

  def value
    @count
  end
end

c = Counter.new(10)
c.inc
c.inc
c.inc
puts c.value

class Greeter
  def initialize(name)
    @name = name
  end

  def hello
    "Hello, " + @name + "!"
  end
end

puts Greeter.new("Ruby in Rust").hello
