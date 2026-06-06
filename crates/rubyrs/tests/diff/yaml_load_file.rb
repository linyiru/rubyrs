# YAML.load_file reads + parses a file (front-matter / config subset).
# Referenced via __dir__ so both runtimes resolve the same path.
# Discovery: P3 Jekyll spike — jekyll reads _config.yml via
# SafeYAML.load_file → our YAML loader. The load_file body must call
# the parser directly, not a bare `load` (which would resolve to
# Kernel#load and try to require the YAML text as a file path).
require "yaml"
p YAML.load_file("#{__dir__}/yaml_data/sample.yml")
