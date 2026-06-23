# RUBY_DESCRIPTION / RUBY_PATCHLEVEL / RUBY_ENGINE_VERSION exist (MRI
# surface). Gems sniff RUBY_DESCRIPTION for the engine / old-version
# bugs (rspec-mocks: `RUBY_DESCRIPTION.include?('2.0.0p247')`).
p RUBY_DESCRIPTION.is_a?(String)
p RUBY_DESCRIPTION.include?('2.0.0p247')   # false
p RUBY_PATCHLEVEL.is_a?(Integer)
p RUBY_ENGINE_VERSION.is_a?(String)
p defined?(RUBY_DESCRIPTION)
