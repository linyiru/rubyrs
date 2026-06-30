# The `SafeYAML` shim — loaded ONLY by `require "safe_yaml"`
# (concatenated after yaml.rb + psych.rb). Jekyll reads front-matter
# via `SafeYAML.load` / `SafeYAML.load_file`; the real gem subclasses
# Psych::Handler, which we bypass by routing straight to the focused
# loader. Kept out of yaml.rb so a bare `require "yaml"` does NOT define
# `::SafeYAML` (RuboCop's config_loader raises if it is — see yaml.rb).
module SafeYAML
  OPTIONS = {} unless defined?(OPTIONS)

  class << self
    def load(source, *_args, **_opts)
      RubyrsYAMLParse.parse_document(source)
    end

    def load_file(path, *_args, **_opts)
      # Call the parser directly rather than bare `load` — inside this
      # singleton method a bare `load` can resolve to Kernel#load (the
      # file loader) instead of YAML.load.
      RubyrsYAMLParse.parse_document(File.read(path))
    end
  end
end
