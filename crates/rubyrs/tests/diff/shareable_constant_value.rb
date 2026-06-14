# `# shareable_constant_value` magic comment — Prism wraps the governed
# constant write in a ShareableConstantNode. rubyrs has no Ractor model,
# so it unwraps to the inner write. (Surfaced by stdlib time.rb.)
# shareable_constant_value: literal
GREETING = "hello"
NUMBERS = [1, 2, 3]
TABLE = { "a" => 1, "b" => 2 }

module Config
  DEFAULTS = { retries: 3, timeout: 10 }
end

p GREETING
p NUMBERS
p TABLE
p Config::DEFAULTS
