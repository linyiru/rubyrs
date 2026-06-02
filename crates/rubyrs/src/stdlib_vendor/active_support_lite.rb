# Tier 3 pure-Ruby ActiveSupport-lite — subset matched to the
# `active_support/all` API real Rack apps reach for. Built from
# the gap inventory in `poc/as_lite/GAPS.md` (M27-followup spike):
# Tier A (`blank?`/`present?`/`presence` family + Array second/
# third/fourth + `Object#try`/`#in?`), Tier B (Regexp-dependent
# String slice — camelize / underscore / dasherize / titleize /
# humanize / squish / truncate / blank?), and Tier C (Hash
# transforms — symbolize/stringify_keys + deep variants +
# deep_merge).
#
# Gated behind the `stdlib` Cargo feature (ADR 0017 row 125;
# ADR 0026 v2 menu item 3). Default Tier-1 builds keep the
# lenient `require "active_support"` stub — the constant exists
# but no method gets added. `--features stdlib` evaluates this
# file's body on the running Vm and gives the canonical API
# its real semantics.
#
# Tier D (Numeric duration helpers, Time.current, Time.zone)
# is deliberately NOT in scope here — see poc/as_lite/GAPS.md
# §"Tier D — DEFERRED" for the ADR-shaped reasoning. Real apps
# that need timezone math should treat that as a separate
# infrastructure decision.
#
# Implementation notes specific to rubyrs:
#   - `each_with_object` isn't on rubyrs's Hash/Array yet, so
#     accumulation uses `inject(init) { |acc, el| ... ; acc }`.
#   - `$1` / `$~` inside `gsub` blocks doesn't reliably capture
#     groups, so the regex shapes use the block arg `|m|` (full
#     match) and post-process explicitly. Slightly more code,
#     same observable behaviour.
#   - All sibling-method calls inside reopened-class instance
#     methods use bare calls (no explicit `self.`); validated by
#     the `vm/dispatch.rs` primitive-reopen bridge shipped at
#     commit b8feb3ce.

# ---- Tier A — blank? / present? / presence + Array extras + Object#try/#in? ----

class Object
  # Vanilla AS spec: an object is "blank" if `respond_to?(:empty?) && !!empty?`,
  # OR if it's `nil` / `false`. Numbers, symbols, etc. are never blank.
  # Overriding per-class below is cheaper than the dispatch dance
  # (`respond_to?(:empty?)` would call into method-table lookup on
  # every call); Object stays as the "neither nil nor empty-able"
  # fallback.
  def blank?
    false
  end

  def present?
    !blank?
  end

  def presence
    self if present?
  end

  # `obj.try(:method)` — call method if it exists, return nil
  # otherwise. AS's `try` also takes a block; the `&block` arg
  # is forwarded.
  def try(*args, &block)
    if args.empty?
      block ? instance_eval(&block) : self
    elsif respond_to?(args.first)
      send(*args, &block)
    else
      nil
    end
  end

  # `obj.in?([list])` — `collection.include?(obj)`. Used by
  # AS-flavoured input validation: `state.in?(VALID_STATES)`.
  def in?(collection)
    collection.include?(self)
  end
end

class NilClass
  # NilClass needs its own `blank?` / `present?` / `presence` —
  # NOT just inherited from Object — because rubyrs's primitive-
  # reopen-bridge (vm/dispatch.rs, commit b8feb3ce) excludes
  # `Value::Nil` self from the bare-call lookup arm. That
  # exclusion is load-bearing for toplevel ArgumentError parity
  # but it means Object#present? calling bare `blank?` on a
  # real-nil receiver can't find NilClass#blank? via normal
  # inheritance. Explicit overrides sidestep the exclusion;
  # behaviour matches AS.
  def blank?
    true
  end

  def present?
    false
  end

  def presence
    nil
  end
end

class FalseClass
  def blank?
    true
  end
end

class TrueClass
  def blank?
    false
  end
end

class String
  # Whitespace-only test. Matches AS — `\A\s*\z` covers `""`,
  # `"   "`, `"\n\t"`. Empty string returns true (`\s*` matches
  # zero chars).
  def blank?
    match?(/\A\s*\z/)
  end
end

class Array
  def blank?
    empty?
  end
end

class Hash
  def blank?
    empty?
  end
end

# AS's `Numeric#blank?` is hard-coded to `false` — numbers are
# never blank regardless of value (yes, `0.blank?` → false).
class Numeric
  def blank?
    false
  end
end

# Object#try on nil short-circuits to nil so `users.first.try(:name)`
# doesn't NoMethodError when first is nil.
class NilClass
  def try(*args)
    nil
  end

  def try!(*args)
    nil
  end
end

class Array
  def second
    self[1]
  end

  def third
    self[2]
  end

  def fourth
    self[3]
  end

  def fifth
    self[4]
  end

  # `arr.in_groups_of(n, fill)` — split into N-sized chunks. When
  # `fill == false`, the trailing partial group is left short; when
  # `fill` is anything else (default nil), the last group is padded
  # to N. AS's signature is `(number, fill_with = nil)`.
  def in_groups_of(number, fill_with = nil)
    out = []
    i = 0
    n = length
    while i < n
      chunk = []
      j = 0
      while j < number && (i + j) < n
        chunk << self[i + j]
        j += 1
      end
      if fill_with != false
        while chunk.length < number
          chunk << fill_with
        end
      end
      out << chunk
      i += number
    end
    out
  end
end

# ---- Tier C — Hash transforms (symbolize/stringify, deep_*, deep_merge) ----

class Hash
  def symbolize_keys
    out = {}
    each do |k, v|
      new_k = k.is_a?(String) ? k.to_sym : k
      out[new_k] = v
    end
    out
  end

  def stringify_keys
    out = {}
    each do |k, v|
      new_k = k.is_a?(Symbol) ? k.to_s : k
      out[new_k] = v
    end
    out
  end

  def deep_symbolize_keys
    inject({}) do |acc, (k, v)|
      new_k = k.is_a?(String) ? k.to_sym : k
      new_v = case v
              when Hash then v.deep_symbolize_keys
              when Array then v.map { |x| x.is_a?(Hash) ? x.deep_symbolize_keys : x }
              else v
              end
      acc[new_k] = new_v
      acc
    end
  end

  def deep_stringify_keys
    inject({}) do |acc, (k, v)|
      new_k = k.is_a?(Symbol) ? k.to_s : k
      new_v = case v
              when Hash then v.deep_stringify_keys
              when Array then v.map { |x| x.is_a?(Hash) ? x.deep_stringify_keys : x }
              else v
              end
      acc[new_k] = new_v
      acc
    end
  end

  # `h1.deep_merge(h2)` — recursive merge: Hash-vs-Hash recurses,
  # anything else uses h2's value. AS also takes a `&block` for
  # custom conflict resolution; not modelled here (the common case
  # is the default block-free form).
  def deep_merge(other)
    result = dup
    other.each do |k, v_other|
      v_self = result[k]
      if v_self.is_a?(Hash) && v_other.is_a?(Hash)
        result[k] = v_self.deep_merge(v_other)
      else
        result[k] = v_other
      end
    end
    result
  end
end

# ---- Tier B — Regexp-dependent String slice ----

class String
  # Collapse runs of whitespace → single space, then trim ends.
  def squish
    gsub(/\s+/, ' ').strip
  end

  # `"active_record".camelize` → `"ActiveRecord"`.
  # `"active_record".camelize(:lower)` → `"activeRecord"`.
  #
  # rubyrs note: `gsub(...) { $1 }` doesn't reliably capture into
  # `$1` from the block — use the block's full-match arg `|m|` and
  # extract the post-underscore char by index instead.
  def camelize(first_letter = :upper)
    s = gsub(/_([a-zA-Z])/) { |m| m[1].upcase }
    if first_letter == :lower
      s.length > 0 ? s[0].downcase + s[1..] : s
    else
      s.length > 0 ? s[0].upcase + s[1..] : s
    end
  end

  # `"ActiveRecord".underscore` → `"active_record"`.
  # Two-pass: insert `_` between consecutive caps + a cap-lower,
  # then between a lower/digit + cap. Matches AS's regex order.
  def underscore
    s = gsub(/([A-Z]+)([A-Z][a-z])/) { |m| m[0..-2] + '_' + m[-1] }
    s.gsub(/([a-z0-9])([A-Z])/) { |m| m[0] + '_' + m[1] }.downcase
  end

  # `"puma_server".dasherize` → `"puma-server"`. Pure tr — no
  # underscore→camelcase round-trip; AS does the same.
  def dasherize
    tr('_', '-')
  end

  # `"puni puni".titleize` → `"Puni Puni"`. AS's implementation is
  # `underscore.humanize.gsub(/\b'?[a-z]/) { |m| m.capitalize }` but
  # for the Tier-B scope (already-spaced or underscored input) the
  # simpler split + capitalize covers the documented cases.
  def titleize
    underscore.gsub('_', ' ').split.map { |w| w.length > 0 ? w[0].upcase + w[1..] : w }.join(' ')
  end

  # `"employee_id".humanize` → `"Employee"`. AS strips trailing
  # `_id`, swaps underscores for spaces, capitalises the first
  # letter.
  def humanize
    s = self.dup
    s = s[0..-4] if s.end_with?('_id')
    s = s.gsub('_', ' ').strip
    s.length > 0 ? s[0].upcase + s[1..].downcase : s
  end

  # `s.truncate(20)` → up to 20 chars including the trailing
  # `omission` (default `"..."`). When the string is already <= the
  # cap, returns it unchanged. AS also takes a `separator:` kwarg
  # for word-boundary truncation — not modelled (rare use case).
  def truncate(truncate_at, omission: '...')
    return self if length <= truncate_at
    keep = truncate_at - omission.length
    keep = 0 if keep < 0
    self[0, keep] + omission
  end
end
