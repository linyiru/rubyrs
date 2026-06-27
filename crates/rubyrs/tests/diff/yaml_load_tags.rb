require "yaml"
YAML.load_tags["!x"] = "X"
p YAML.load_tags["!x"]
p YAML.dump_tags.is_a?(Hash)
p YAML.respond_to?(:load_tags)
