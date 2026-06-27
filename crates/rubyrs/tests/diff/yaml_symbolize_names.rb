# YAML.load / load_file honor Psych's `symbolize_names: true` (Symbol keys,
# recursively). i18n's backend loads locale files this way.
require "yaml"
src = "en:\n  errors:\n    messages:\n      blank: \"can't be blank\"\n  list:\n    - a\n    - b\n"
p YAML.load(src, symbolize_names: true)
p YAML.load(src)[:en]                    # nil — string keys without the option
p YAML.load(src)["en"]["errors"]["messages"]["blank"]
