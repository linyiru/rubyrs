# Psych::Parser + Psych::TreeBuilder event-stream layer — the API
# RuboCop's YAMLDuplicationChecker subclasses (config_loader.rb).
# A handler subclassing Psych::TreeBuilder overrides end_mapping,
# calls super, and inspects mapping_node.children.each_slice(2) for
# duplicate keys (key.value + key.start_line).
require "yaml"

p defined?(Psych::Parser)
p defined?(Psych::TreeBuilder)
p defined?(Psych::Nodes::Mapping)
p(Psych::TreeBuilder.ancestors.include?(Psych::Handler))

# Mirror RuboCop's DuplicationCheckHandler exactly.
class DupCheck < Psych::TreeBuilder
  def initialize(&block)
    super()
    @block = block
  end

  def end_mapping
    mapping_node = super
    keys = {}
    mapping_node.children.each_slice(2) do |key, _value|
      duplicate = keys[key.value]
      @block.call(duplicate, key) if duplicate
      keys[key.value] = key
    end
    mapping_node
  end
end

yaml = <<~YAML
  Style/Foo:
    Enabled: true
    Max: 10
  Style/Bar:
    Enabled: false
  Style/Foo:
    Enabled: false
YAML

dups = []
handler = DupCheck.new { |k1, k2| dups << [k1.value, k1.start_line, k2.start_line] }
parser = Psych::Parser.new(handler)
parser.parse(yaml, "test.yml")

# The top-level Style/Foo key is duplicated (lines 0 and 5, 0-based).
p dups
p(parser.handler.equal?(handler))

# root is a Stream; first document child is the top-level mapping.
root = parser.handler.root
p root.class.name
doc = root.children[0]
p doc.class.name
top = doc.children[0]
p top.class.name
# top-level mapping keys, in order (each_slice pairs key,value)
p(top.children.each_slice(2).map { |k, _v| k.value })

# Nested mapping with NO duplicates fires nothing extra; quoted keys.
yaml2 = <<~YAML
  list:
    - a
    - b
  "quoted key": 1
  plain: [1, 2, 3]
YAML
h2 = DupCheck.new { |_a, _b| }
Psych::Parser.new(h2).parse(yaml2, "t2.yml")
m2 = h2.root.children[0].children[0]
p(m2.children.each_slice(2).map { |k, _v| k.value })

# Psych::VERSION is >= 4 (rubocop/cache_config branches on it).
p(Psych::VERSION.split(".").first.to_i >= 4)
