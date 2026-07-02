# Generate crates/rubyrs/src/prism_node_specs.rs from the prism gem's
# GENERATED serialize.rb + node.rb (ADR 0036 Slice 2 — native materialization).
#
# The native deserializer in prism_materialize.rs is a generic loop over a
# per-node-type field table. That table must match, byte-for-byte, the wire
# format the linked prism C library emits AND the ivar layout the prism gem's
# Ruby node classes carry. Both are code-generated inside prism from
# config.yml — so the safest single source of truth is the gem's own
# generated output: serialize.rb's `load_node` case tells us the field
# KINDS/ORDER (the wire format), node.rb's `initialize` tells us the ivar
# NAMES those positional fields land in.
#
# Usage: ruby scripts/gen_prism_node_specs.rb [prism_gem_lib_dir]
# Re-run when bumping the vendored/linked prism version; the output is
# checked in (the build must not depend on a gem install being present).
#
# The materializer independently verifies the blob's version header at run
# time (MAJOR/MINOR/PATCH pinned below) and declines to a pure-Ruby fallback
# on mismatch, so a drifted table can never silently corrupt a parse.

gem_lib = ARGV[0] || "/Users/linyiru/.rbenv/versions/3.4.1/lib/ruby/gems/3.4.0/gems/prism-1.9.0/lib"
serialize = File.read(File.join(gem_lib, "prism/serialize.rb"))
node_rb = File.read(File.join(gem_lib, "prism/node.rb"))

major = serialize[/MAJOR_VERSION = (\d+)/, 1] or abort "no MAJOR_VERSION"
minor = serialize[/MINOR_VERSION = (\d+)/, 1] or abort "no MINOR_VERSION"
patch = serialize[/PATCH_VERSION = (\d+)/, 1] or abort "no PATCH_VERSION"

# --- 1. ivar names per node class, from node.rb initialize signatures -------
# `def initialize(source, node_id, location, flags, f1, f2, ...)` — everything
# after `flags` is a positional field whose ivar is `@<name>`.
ivars = {}
node_rb.scan(/class (\w+) < Node\b.*?def initialize\(([^)]*)\)/m) do |klass, params|
  names = params.split(",").map(&:strip)
  raise "unexpected initialize prefix for #{klass}: #{names[0, 4].inspect}" unless
    names[0, 4] == %w[source node_id location flags]
  ivars[klass] = names[4..]
end

# --- 2. wire field kinds per node type id, from serialize.rb's load_node ----
# Only the RUBY_ENGINE == "ruby" branch (the plain case/when) is parsed; the
# lambda branch encodes the same format.
ruby_branch = serialize[/if RUBY_ENGINE == "ruby"\n(.*?)\n\s*else\n/m, 1] or abort "no ruby branch"

# Split a `Klass.new(...)` argument list at top-level commas.
def split_args(s)
  args, depth, cur = [], 0, +""
  s.each_char do |c|
    case c
    when "(", "{", "[" then depth += 1; cur << c
    when ")", "}", "]" then depth -= 1; cur << c
    when ","
      if depth.zero?
        args << cur.strip
        cur = +""
      else
        cur << c
      end
    else cur << c
    end
  end
  args << cur.strip unless cur.strip.empty?
  args
end

def kind_of(expr)
  case expr
  when "load_varuint" then "VarUint"
  when /\AArray\.new\(load_varuint\) \{ load_node\(/ then "NodeList"
  when /\AArray\.new\(load_varuint\) \{ load_constant\(/ then "ConstantList"
  when /\Aload_node\(/ then "Node"
  when /\Aload_optional_node\(/ then "OptNode"
  when /\Aload_constant\(/ then "Constant"
  when /\Aload_optional_constant\(/ then "OptConstant"
  when "load_string(encoding)" then "Str"
  when "load_location(freeze)" then "Location"
  when "load_optional_location(freeze)" then "OptLocation"
  when "load_integer" then "Integer"
  when "load_double" then "Double"
  when "io.getbyte" then "UInt8"
  else raise "unknown field expr: #{expr.inspect}"
  end
end

specs = {} # type id => [klass, skip_uint32, [[kind, ivar], ...]]
ruby_branch.scan(/when (\d+) then\n(.*?)(?=\n\s*when \d+ then\n|\n\s*end\n)/m) do |id, body|
  id = Integer(id)
  skip_uint32 = body.include?("load_uint32\n")
  m = body.match(/(\w+)\.new\((.*)\)\s*\z/m) or raise "no ctor in when #{id}"
  klass = m[1]
  args = split_args(m[2])
  raise "bad prefix for #{klass}" unless args[0, 3] == %w[source node_id location]
  raise "no flags varuint for #{klass}" unless args[3] == "load_varuint"
  fields = args[4..].map { |a| kind_of(a) }
  names = ivars.fetch(klass) { raise "no node.rb class for #{klass}" }
  unless names.length == fields.length
    raise "field count mismatch for #{klass}: wire #{fields.length} vs ivars #{names.length}"
  end
  specs[id] = [klass, skip_uint32, fields.zip(names)]
end

max_id = specs.keys.max
raise "non-contiguous node ids" unless specs.keys.sort == (1..max_id).to_a

# --- 3. token + diagnostic tables -------------------------------------------
def extract_list(src, name)
  body = src[/#{name} = \[\n(.*?)\n\s*\]/m, 1] or raise "no #{name}"
  body.scan(/^\s*(nil|:\S+?),?\s*$/).map do |(tok)|
    tok == "nil" ? nil : tok.delete_prefix(":").delete_suffix(",")
  end
end
tokens = extract_list(serialize, "TOKEN_TYPES")
diagnostics = extract_list(serialize, "DIAGNOSTIC_TYPES")
raise "TOKEN_TYPES[0] must be the nil terminator" unless tokens[0].nil?

# --- 4. emit -----------------------------------------------------------------
out = +""
out << <<~HEADER
  //! GENERATED by scripts/gen_prism_node_specs.rb from the prism gem's own
  //! generated serialize.rb + node.rb (prism #{major}.#{minor}.#{patch}). Do not edit by
  //! hand — re-run the script when the linked prism version changes.
  //!
  //! One `NodeSpec` per wire node-type id: the Ruby class the interpreted
  //! deserializer would instantiate, and the (wire field kind, ivar name)
  //! pairs its positional constructor args land in, in wire order.

  use super::prism_materialize::FieldKind::{self, *};

  /// Wire-format version this table was generated against. The materializer
  /// verifies the blob header matches before trusting the table.
  pub(crate) const WIRE_VERSION: (u8, u8, u8) = (#{major}, #{minor}, #{patch});

  pub(crate) struct NodeSpec {
      /// Unqualified class name under `Prism::`.
      pub(crate) name: &'static str,
      /// `true` when the wire carries a leading uint32 the Ruby loader
      /// discards (DefNode's serialized-locals length).
      pub(crate) skip_uint32: bool,
      /// `(wire kind, ivar name)` in wire order, excluding the common
      /// `@source`/`@node_id`/`@location`/`@flags` prefix.
      pub(crate) fields: &'static [(FieldKind, &'static str)],
  }

HEADER

out << "/// Indexed by `wire_type - 1` (wire node types are 1-based).\n"
out << "pub(crate) static NODE_SPECS: [NodeSpec; #{max_id}] = [\n"
(1..max_id).each do |id|
  klass, skip, fields = specs[id]
  flds = fields.map { |k, n| "(#{k}, \"@#{n}\")" }.join(", ")
  out << "    // #{id}\n"
  out << "    NodeSpec { name: \"#{klass}\", skip_uint32: #{skip}, fields: &[#{flds}] },\n"
end
out << "];\n\n"

out << "/// Wire token-type table; index 0 is the end-of-tokens terminator.\n"
out << "pub(crate) static TOKEN_TYPES: [&str; #{tokens.length}] = [\n"
tokens.each { |t| out << "    \"#{t}\",\n" } # nil terminator prints as ""
out << "];\n\n"

out << "/// Diagnostic type symbols, by wire diagnostic id.\n"
out << "pub(crate) static DIAGNOSTIC_TYPES: [&str; #{diagnostics.length}] = [\n"
diagnostics.each { |d| out << "    \"#{d}\",\n" }
out << "];\n"

path = File.expand_path("../crates/rubyrs/src/prism_node_specs.rs", __dir__)
File.write(path, out)
puts "wrote #{path}: #{max_id} node specs, #{tokens.length} token types, #{diagnostics.length} diagnostics"
