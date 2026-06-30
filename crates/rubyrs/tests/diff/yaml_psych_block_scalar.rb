# Psych block scalars (`|` literal, `>` folded, with `-`/`+`/clip
# chomping) and anchors, materialized via the TreeBuilder + ToRuby path
# RuboCop's config_loader uses. Driver: rubocop's config/default.yml is
# 607 keys of `Description: >-` folded scalars + a `&supported_styles`
# anchor; the subset parser previously stopped at the first block
# scalar (7 keys).
require "yaml"

def to_ruby(src)
  parser = Psych::Parser.new(Psych::TreeBuilder.new)
  parser.parse(src, "t.yml")
  tree = parser.handler.root.children[0]
  cl = YAML::ClassLoader::Restricted.new(%w[Regexp Symbol], [])
  YAML::Visitors::ToRuby.new(YAML::ScalarScanner.new(cl), cl).accept(tree)
end

y = <<~YAML
  folded: >-
    line one
    line two
  literal: |
    a
    b
  clip: >
    hi there
  nested:
    desc: >-
      wrapped text
      continues
    enabled: true
  seq: &anchored
    - nested
    - compact
YAML

r = to_ruby(y)
p r["folded"]
p r["literal"]
p r["clip"]
p r["nested"]
p r["seq"]
p r.keys
