# `def foo(**opts)` — rest-keyword parameter. Captures every kwarg
# the caller passed that wasn't claimed by a named keyword param,
# binds them as a Hash to the rest-kw name.

# Pure rest-kwargs.
def render(**opts)
  opts.each { |k, v| puts "#{k}=#{v}" }
end
render(host: "localhost", port: 3000)
puts "---"

# Mixed: positional + rest-kwargs (no named kw params).
def log(level, **fields)
  puts level
  p fields
end
log("INFO", user: "alice", action: "login")
log("DEBUG")
puts "---"

# Named kwargs + rest-kwargs. Named ones are extracted, rest
# captures the leftover.
def event(name:, **meta)
  puts "name=#{name}"
  p meta
end
event(name: "click", x: 10, y: 20, button: "left")
event(name: "ping")
puts "---"

# Default + rest. CRuby semantics: missing optional kw uses default,
# leftover still flows into **rest.
def request(method: "GET", **headers)
  puts "method=#{method}"
  p headers
end
request(method: "POST", token: "abc", agent: "rubyrs")
request(accept: "json")
puts "---"

# Inside a class.
class Config
  def initialize(**opts)
    @opts = opts
  end
  def to_s
    @opts.map { |k, v| "#{k}=#{v}" }.join(",")
  end
end
puts Config.new(a: 1, b: 2, c: 3).to_s
