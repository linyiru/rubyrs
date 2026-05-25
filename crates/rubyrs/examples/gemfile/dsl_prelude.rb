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

# RUBY_VERSION is now seeded by the rubyrs preamble (see
# `Runtime::load_preamble`), so the Gemfile can rely on
# `if RUBY_VERSION >= "3.4.0"` directly. The earlier
# prelude-level assignment was removed to avoid an
# already-initialised-constant reassignment.

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
# v2 host fn (`register_fn_v2` + `HostCtx`) reads the splat as an
# Array and the kwargs as a Hash directly, with no Ruby-side
# massage — `HostCtx` exposes `resolve_array`, `resolve_hash`, and
# `resolve_sym` so Bundler's `:require => false` /
# `:platforms => :mri` kwargs are unpacked entirely in typed
# Rust. The Gemfile lines reach the host byte-for-byte unchanged.
def gem(name, *requirements, **opts)
  __gemfile_gem_v2(name, requirements, opts)
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
