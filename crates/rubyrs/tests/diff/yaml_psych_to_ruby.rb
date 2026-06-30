# Psych node-tree → Ruby materialization, exactly as RuboCop's
# config_loader does it (reuse the parsed tree, then ClassLoader +
# ScalarScanner + Visitors::ToRuby). yaml/psych are the same module.
require "yaml"

yaml = <<~YAML
  AllCops:
    NewCops: enable
    TargetRubyVersion: 3.1
    Exclude:
      - "vendor/**/*"
      - bin/*
    MaxFilesInCache: 20000
  Style/StringLiterals:
    Enabled: true
    EnforcedStyle: double_quotes
  Layout/LineLength:
    Max: 120
    Enabled: false
YAML

# RuboCop builds the tree via Psych::Parser/TreeBuilder, then:
parser = Psych::Parser.new(Psych::TreeBuilder.new)
parser.parse(yaml, "rubocop.yml")
tree = parser.handler.root.children[0]   # the Document node

class_loader = YAML::ClassLoader::Restricted.new(%w[Regexp Symbol], [])
scanner = YAML::ScalarScanner.new(class_loader)
visitor = YAML::Visitors::ToRuby.new(scanner, class_loader)
result = visitor.accept(tree)

p result
p result["AllCops"]["TargetRubyVersion"]
p result["AllCops"]["TargetRubyVersion"].class
p result["AllCops"]["NewCops"]
p result["AllCops"]["Exclude"]
p result["AllCops"]["MaxFilesInCache"]
p result["Style/StringLiterals"]["Enabled"]
p result["Layout/LineLength"]["Max"]
p result["Layout/LineLength"]["Enabled"]

# scalar coercion edge cases
p Psych::ScalarScanner.new(class_loader).tokenize("true")
p Psych::ScalarScanner.new(class_loader).tokenize("120")
p Psych::ScalarScanner.new(class_loader).tokenize("3.1")
p Psych::ScalarScanner.new(class_loader).tokenize("Style/Foo")
p Psych::ScalarScanner.new(class_loader).tokenize(":a_symbol")
p Psych::ScalarScanner.new(class_loader).tokenize("warning")
p Psych::ScalarScanner.new(class_loader).tokenize("")

# Restricted disallows non-permitted classes
begin
  YAML::ClassLoader::Restricted.new(%w[Symbol], []).load("File")
  p "no-raise"
rescue Psych::DisallowedClass => e
  p "disallowed"
end
