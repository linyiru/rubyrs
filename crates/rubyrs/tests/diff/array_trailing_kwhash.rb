# A trailing `k: v` inside an array literal is a Hash element (Sinatra's
# `set :static_cache_control, [:public, max_age: 0]`).
p [:public, max_age: 0]
p [1, 2, a: 3, b: 4]
p [:x, *[1, 2], k: 9]
p [foo: 1]
