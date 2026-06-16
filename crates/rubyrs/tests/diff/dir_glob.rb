# Dir.glob / Dir[] / entries / children / exist? over a committed
# data tree (dir_glob_data/), referenced via __dir__ so both runtimes
# resolve the same absolute paths. Output paths are made relative to
# __dir__ so they're machine-independent. Discovery: P3 Jekyll spike —
# Liquid loads its tag files via a Dir glob; Jekyll globs site sources.
base = "#{__dir__}/dir_glob_data"
rel = ->(paths) { paths.map { |p| p.sub("#{__dir__}/", "") }.sort }

p rel.call(Dir["#{base}/*.rb"])               # top-level .rb (no dotfiles)
p rel.call(Dir.glob("#{base}/*"))             # all top-level entries
p rel.call(Dir["#{base}/**/*.rb"])            # recursive .rb
p rel.call(Dir.glob("#{base}/*.{rb,txt}"))    # brace expansion
p rel.call(Dir["#{base}/sub/*"])              # one subdir
p rel.call(Dir["#{base}/?eta.rb"])            # ? single-char wildcard
p rel.call(Dir["#{base}/nope/*.rb"])          # no matches -> []

# LITERAL path segments (the fast-path: one stat, no read_dir of the
# whole dir). Each must still resolve identically to the listing walk.
p rel.call(Dir["#{base}/alpha.rb"])           # literal final file
p rel.call(Dir["#{base}/missing.rb"])         # literal miss -> []
p rel.call(Dir["#{base}/sub"])                # literal dir
p rel.call(Dir["#{base}/sub/delta.rb"])       # literal nested file
p rel.call(Dir["#{base}/sub/nested/epsilon.rb"]) # deep literal chain
p rel.call(Dir["#{base}/sub/*.rb"])           # literal dir + wildcard tail
p rel.call(Dir["#{base}/sub/nested/*.rb"])    # two literals + wildcard

p Dir.entries(base).sort
p Dir.children(base).sort
p Dir.exist?(base)
p Dir.exist?("#{base}/sub")
p Dir.exist?("#{base}/missing")
