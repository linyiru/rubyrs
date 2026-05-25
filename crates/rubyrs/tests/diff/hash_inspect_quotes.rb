# Hash#inspect — Symbol keys that aren't valid bareword identifiers
# (hyphen, space, digit-leading, etc.) get wrapped in double quotes
# in CRuby's output: `{"X-Token": "abc"}` instead of `{X-Token: "abc"}`.
# Bareword-safe symbols keep the shorthand: `{name: 1}`.

# Mixed bareword + non-bareword keys.
p({ "X-Token": "abc", normal: 1, "with space": "x" })

# Trailing ? / ! on a method-shaped name — still bareword in CRuby's
# hash output. `name=:` form isn't valid hash literal syntax.
p({ empty?: true, save!: false })

# All numeric content — would be invalid as a bareword.
p({ "404": "not found", "500": "server error" })

# Each key class displays its own way.
p({ "kebab-case-key": [1, 2, 3] })
p({ snake_case: 42, "with-dash": 7 })

# Non-Symbol keys still use the hash-rocket form.
p({ "string_key" => 1, 1 => "int_key" })

# Nested.
p({ outer: { "X-Inner": 99 } })
