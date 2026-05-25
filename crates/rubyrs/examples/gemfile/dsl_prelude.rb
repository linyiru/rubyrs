# Pre-loaded by the gemfile example host before the user's
# unmodified `Gemfile` runs. Lifts the Ruby-side shapes that
# the host-fn API can't naturally consume (`*splat`, `**kwargs`,
# block-yielding scope blocks) down to plain positional
# String / Int / Bool args so each host fn signature stays
# trivial.
#
# Pattern: every host fn receives only primitives. The Hash
# / Array unpacking happens here; the host receives the
# already-stringified pieces. This keeps the host harness
# in `gemfile.rs` ~15 closure lines instead of one-per-shape.
#
# The unmodified Gemfile only sees the public DSL surface:
# `source`, `ruby`, `gem`, `group`, `platforms`, `git`, `path`.
# The `__gemfile_*` shims are an internal seam between the
# prelude and the Rust host — invisible to the Gemfile.

# Bundler usually exposes RUBY_VERSION via Ruby itself; rubyrs
# doesn't have it built in, so we seed it from the prelude.
# The Gemfile uses it for conditional inclusion
# (`if RUBY_VERSION >= "3.4.0"`).
RUBY_VERSION = "3.4.0"

# ---------- top-level DSL ----------

# Public form unchanged from Bundler: `source "https://..."`.
# Host receives a String.
def source(url)
  __gemfile_source(url)
end

# Public form unchanged: `ruby "3.4.0"`.
def ruby(version)
  __gemfile_ruby(version)
end

# The workhorse. Bundler accepts `gem "name", *version_specs, **opts`.
# rubyrs's host-fn API gives the closure a `&[Value]`, with no
# `&Heap` to peer into the trailing Hash or splatted Array. So
# we unpack here: stringify the requirements with `|` as a
# separator (no version spec contains `|`), pull the kwargs we
# care about, then pass everything as plain positionals to the
# host. The host re-splits and records.
def gem(name, *requirements, **opts)
  reqs = requirements.join("|")
  require_kw   = opts.key?(:require)   ? opts[:require].to_s   : ""
  platforms_kw = opts.key?(:platforms) ? opts[:platforms].to_s : ""
  __gemfile_gem(name, reqs, require_kw, platforms_kw)
end

# ---------- block-scoping helpers ----------

# `group :development, :test do ... end` — push the names onto
# the host's group stack, yield (block body runs `gem` calls
# that read the current groups), pop on the way out. The
# `ensure` keeps the stack balanced even if the block raises.
#
# Symbols → comma-joined string so the host fn receives a
# single String it can split, sidestepping the
# Array-element-from-host-fn API gap.
def group(*names)
  __gemfile_push_groups(names.map { |n| n.to_s }.join(","))
  begin
    yield
  ensure
    __gemfile_pop_groups
  end
end

def platforms(*names)
  __gemfile_push_platforms(names.map { |n| n.to_s }.join(","))
  begin
    yield
  ensure
    __gemfile_pop_platforms
  end
end

def git(url)
  __gemfile_push_git(url)
  begin
    yield
  ensure
    __gemfile_pop_git
  end
end

def path(p)
  __gemfile_push_path(p)
  begin
    yield
  ensure
    __gemfile_pop_path
  end
end
