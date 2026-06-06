# CRuby's `YAML` is literally `Psych` (the same object). rubyrs mirrors
# that after `require "yaml"`. Discovery: P3 Jekyll spike — safe_yaml's
# engine probe is `defined?(Psych) && YAML == Psych ? "psych" : "syck"`;
# without the alias it fell to the legacy syck path.
require "yaml"
p defined?(YAML)
p defined?(Psych)
p(YAML == Psych)
p(YAML.equal?(Psych))
p YAML.is_a?(Module)
# the engine-probe expression safe_yaml evaluates
engine = (defined?(Psych) && YAML == Psych) ? "psych" : "syck"
p engine
