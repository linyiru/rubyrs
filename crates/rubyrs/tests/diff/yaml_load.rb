# Focused YAML loader (front-matter / config subset): block maps,
# block sequences, typed scalars, quoted strings, flow [..]/{..},
# comments, `---`. Parity is against CRuby's real Psych for this
# subset. Discovery: P3 Jekyll spike — jekyll reads front-matter via
# SafeYAML.load, backed here by rubyrs's vendored YAML.load.
require "yaml"

p YAML.load("title: Hello World\nlayout: post\n")
p YAML.load("---\ntitle: Hello\ntags:\n  - ruby\n  - jekyll\n---\n")
p YAML.load("nested:\n  a: 1\n  b: 2\nflag: true\ncount: 42\npi: 3.14\n")
p YAML.load("list:\n- one\n- two\n- three\n")
p YAML.load("quoted: \"a: b # c\"\nsingle: 'it''s here'\n")
p YAML.load("flow_seq: [1, 2, 3]\nflow_map: {x: 1, y: 2}\n")
p YAML.load("empty_val:\nnil_val: ~\nexplicit_null: null\n")
p YAML.load("# a comment\nkey: value  # trailing comment\n")
p YAML.load("permalink: /:categories/:year/\npublished: false\n")
p YAML.load("authors:\n  - name: Alice\n    role: dev\n  - name: Bob\n    role: ops\n")
p YAML.load("neg: -5\nbignum: 1000000\nfloaty: -0.25\n")
p YAML.load("")
p YAML.load("just a scalar")
