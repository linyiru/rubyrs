# Vendored OptionParser — the on/parse! subset minitest's
# process_args exercises.
require "optparse"
opts = {}
op = OptionParser.new do |o|
  o.banner = "test options:"
  o.on("-s", "--seed SEED", Integer, "Sets random seed.") { |m| opts[:seed] = m.to_i }
  o.on("-e", "--exclude PATTERN") { |a| opts[:exclude] = a }
  o.on("--no-plugins", "Bypass") { |v| opts[:plugins] = v }
  o.on("-v", "--verbose") { opts[:verbose] = true }
end
argv = ["--seed", "42", "--exclude", "Alpha#x", "--no-plugins", "positional", "-v"]
op.parse!(argv)
p opts
p argv
begin
  op.parse!(["--bogus"])
rescue OptionParser::InvalidOption => e
  puts "invalid: #{e.message}"
end
argv2 = ["--seed=7", "-eFoo", "--", "-v"]
op.parse!(argv2)
p [opts[:seed], opts[:exclude], argv2]
# non-destructive parse
keep = ["--seed", "3", "rest"]
op.parse(keep)
p [opts[:seed], keep]
