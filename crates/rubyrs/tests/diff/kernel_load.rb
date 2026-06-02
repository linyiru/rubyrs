# `Kernel#load` — Sinatra GAPS.md Gap #12 fix. Distinguishing
# semantics from `require`: no `$LOADED_FEATURES` dedup (always
# re-runs), no `.rb` auto-extension (literal path only), always
# returns true on success.
#
# Stages a tempfile under /tmp (the diff harness runs both
# runtimes with FS read enabled; this is a test fixture, not
# embed-mode sandbox). File.write only — Dir / FileUtils aren't
# yet in the rubyrs surface and aren't needed here. CRuby and
# rubyrs both write to the same fixed path; CRuby runs after
# rubyrs in the harness so the second run overwrites — both
# read the same content.
LOAD_TGT = "/tmp/rubyrs_kernel_load_test.rb"
File.write(LOAD_TGT, '$counter ||= 0; $counter += 1; puts "lib loaded, counter=#{$counter}"')

# 1. Three back-to-back loads — each re-runs the body. require would
#    dedup after the first.
load LOAD_TGT
load LOAD_TGT
load LOAD_TGT

# 2. Return value — load always returns true on success. require
#    returns false on subsequent calls; load doesn't have that
#    second-call concept.
puts load(LOAD_TGT).inspect

# 3. Interleave with require: load doesn't populate
#    $LOADED_FEATURES, so require can still run-once afterwards.
require LOAD_TGT
require LOAD_TGT  # no-op
load    LOAD_TGT  # runs again

puts "final counter=#{$counter}"

# 4. Error surface — class names + messages.
begin
  load "/definitely/not/a/real/path/foo.rb"
rescue LoadError => e
  puts "LoadError: #{e.message}"
end

begin
  load 42
rescue TypeError => e
  puts "TypeError: #{e.message}"
end

begin
  load
rescue ArgumentError => e
  puts "ArgumentError: #{e.message}"
end

# No explicit cleanup — File.delete isn't yet in the rubyrs
# surface, and the File.write at the top overwrites the staging
# file each run, so leftover content doesn't bleed across
# invocations. $counter resets at process boundary regardless.
