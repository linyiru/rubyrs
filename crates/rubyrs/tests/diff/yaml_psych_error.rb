# Psych::SyntaxError resolves (jekyll rescues it); YAML and Psych are
# the same module, and the exception hierarchy is StandardError-rooted.
require "yaml"
p defined?(Psych::SyntaxError)
p defined?(YAML::SyntaxError)
p Psych::SyntaxError.ancestors.include?(StandardError)
p YAML.equal?(Psych)
