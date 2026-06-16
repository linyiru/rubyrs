# Coverage-harness generator (companion to examples/coverage.rs). For every
# rouge lexer, extract its rule table into carmine's JSON format AND record
# rouge's own token stream over the lexer's demo (the golden) + the demo
# text itself. The Rust harness then lexes each demo with carmine and diffs
# against the golden — a measurable carmine↔rouge coverage baseline for the
# drop-in-replacement work. Output is self-contained (no rouge path needed
# by the Rust side).
#
# Run under `--parser=parse.y`: the wordlist upgrade VALIDATES each
# classifier block via `RubyVM::AbstractSyntaxTree.of(block)`, which Ruby
# 3.4's default Prism backend can't provide for compiled iseqs. Without it
# the AST gate returns nil → blocks stay callbacks (still SOUND, just less
# coverage).
#
#   ruby --parser=parse.y crates/carmine/tools/gen_coverage.rb [ROUGE_LIB_DIR]
#   CARMINE_COV_DIR=/tmp/carmine_cov cargo run -p carmine --example coverage
#
# Env: CARMINE_COV_DIR (output dir, default /tmp/carmine_cov). Optional
# ROUGE_LIB_DIR arg prepends a rouge checkout to the load path.
require "json"
$LOAD_PATH.unshift(ARGV[0]) if ARGV[0] && File.directory?(ARGV[0])
require "rouge"
OUT = ENV["CARMINE_COV_DIR"] || "/tmp/carmine_cov"
require "fileutils"; FileUtils.mkdir_p(OUT)

# --- recording DSL (mirrors tools/extract.rb) ---
class TraceCtx
  attr_reader :actions
  class Bail < StandardError; end
  def initialize; @actions = []; end
  def groups(*toks); @actions << ["groups", toks.map(&:qualname)]; end
  def token(tok, val = :__default__); raise Bail unless val == :__default__; @actions << ["token", tok.qualname]; end
  def push(st = :__self__); @actions << ["push", st.to_s]; end
  def pop!(n = 1); @actions << ["pop", n]; end
  def goto(st); @actions << ["goto", st.to_s]; end
  def method_missing(*); raise Bail; end
  def respond_to_missing?(*); true; end
end

# Stand-in for the MatchData `|m|` the rouge block receives. ANY access
# flips `touched` — a block that reads the match is match-dependent and
# must be a `callback` (its emitted tokens vary per match), never recorded
# as static `actions`. Returns benign values so the block runs far enough
# to reveal the access (the recorded actions are discarded once touched).
class MatchProbe
  def initialize; @touched = false; end
  def touched?; @touched; end
  def [](*); @touched = true; ""; end
  def to_s; @touched = true; ""; end
  def to_str; @touched = true; ""; end
  def to_ary; @touched = true; []; end
  def method_missing(*); @touched = true; self; end
  def respond_to_missing?(*); true; end
end

# --- generic wordlist (keyword-classifier) upgrade ---
# The most common rouge callback is `do |m| if SET.include?(m[0]) then
# token A elsif SET2.include?(m[0]) then token B else token Name end end`.
# carmine's engine has a native `wordlist` rule (classify the match by set
# membership), so detecting this shape turns a decline into a MATCH.
#
# SOUNDNESS (the drop-in must never diverge): a block is only upgraded if
# EVERY use of `m[0]` is as an `include?` ARGUMENT and it emits exactly one
# `token T` (no value, no groups/push/pop). Enforced by:
#  - WLWord (the value of m[0]) raises on every method → any non-include?
#    use (==, =~, .start_with?, value-token) bails to callback;
#  - the include? hook captures the receiver SETS (returning false so the
#    word's methods are never touched), giving the COMPLETE candidate words
#    via `set.to_a`;
#  - tabulation re-runs the REAL block per candidate word (real sets) so the
#    recorded token is exactly what rouge emits; a non-member probes default.
require "set"

class WLBail < StandardError; end
class WLWord < BasicObject
  def method_missing(*); ::Kernel.raise ::WLBail; end
end
WL_WORD = WLWord.new

module WLIncludeHook
  def include?(arg)
    if $wl_capture && arg.equal?(WL_WORD)
      $wl_sets << self
      return false # take the else/next branch so every set is visited
    end
    super
  end
end
[::Array, ::Set, ::Hash].each { |k| k.prepend(WLIncludeHook) }

# m[0] → the word (WL_WORD in discovery, a real String in tabulation);
# any other match access bails (capture groups, pre_match, …).
class WLMatch
  def initialize(w); @w = w; end
  def [](i); i == 0 ? @w : raise(WLBail); end
  def method_missing(*); raise WLBail; end
  def respond_to_missing?(*); true; end
end

# Records the single emitted token; bails on anything else (value-token,
# groups, push, pop, goto …) so only pure classifiers survive.
class WLCtx
  attr_reader :tok
  def token(t, val = :__d__); raise WLBail if val != :__d__ || @tok; @tok = t.qualname; end
  def method_missing(*); raise WLBail; end
  def respond_to_missing?(*); true; end
end

# --- AST→IR compiler (value-token + conditional/stateful blocks) ---
# After the keyword classifier, the big callback families are VALUE-tokens
# (`token Name::Function, m[1]`) and CONDITIONAL/STATEFUL blocks (heredoc
# queues, `state?`-guards, `m[i]=="lit"` classifiers). carmine's engine runs
# a Conditional-Action IR (src/ir.rs); this COMPILES a rule block's AST
# directly to that IR — it does NOT run the block, because with conditionals
# the branch taken depends on the match, so running once would be unsound.
# Conditions compile to `if` ops the ENGINE evaluates per match.
#
# SOUNDNESS: the decline boundary IS the compiler — any AST shape it doesn't
# recognize returns nil → the rule stays a `callback` (never wrong). The
# translation faithfully mirrors rouge's semantics (and the Rust twin
# crates/rubyrs/src/rouge_ir.rs, already trusted for fence highlighting):
# m[i]→capture i (0 = whole match), engine `emit` merges adjacent same-type
# tokens exactly like rouge's lex stream. Run under `--parser=parse.y`
# (Prism iseqs have no `.of`).
module IrCompiler
  N = RubyVM::AbstractSyntaxTree::Node
  def self.node?(x); x.is_a?(N); end

  # The lexer CLASS being extracted — lets `self.class.keywords`-style
  # CLASS-LEVEL data (match-independent, memoized Sets) resolve to literal
  # word lists at extract time, exactly the data the wordlist hook captures
  # at runtime. Set by extract_table before each lexer.
  class << self; attr_accessor :klass; end

  # block → ops array (JSON-ready), or nil to decline.
  def self.compile(blk)
    scope = begin
      RubyVM::AbstractSyntaxTree.of(blk)
    rescue StandardError
      return nil
    end
    return nil unless node?(scope) && scope.type == :SCOPE
    mvar = (scope.children[0] || [])[0] # the `|m|` match param
    @aliases = {} # leading `name = m[i]` binds (per block)
    body = scope.children[2]
    stmts = ((node?(body) && body.type == :BLOCK) ? body.children : [body]).compact
    out = []
    peeling = true
    stmts.each do |s|
      next if peeling && alias_bind?(s, mvar) # `name = m[i]` → record, drop
      peeling = false
      next if debug_print?(s)
      op = stmt(s, mvar); return nil if op.nil?
      out << op
    end
    out.empty? ? nil : out
  end

  # A leading `lvar = m[i]` / `lvar = m[i].downcase|upcase` alias: record
  # `lvar → [group, fold]` (used bare later, it means that capture, folded)
  # and drop the binding from the op stream. A non-leading reassignment is
  # NOT peeled → its LASGN reaches stmt → nil → decline, never stale.
  def self.alias_bind?(s, mvar)
    return false unless node?(s) && %i[LASGN DASGN].include?(s.type)
    r = resolve_group(s.children[1], mvar)
    return false if r.nil?
    @aliases[s.children[0]] = r
    true
  end

  # A body (BLOCK of statements, or a single statement) → [ops] or nil.
  def self.stmt_list(body, mvar)
    stmts = (node?(body) && body.type == :BLOCK) ? body.children : [body]
    out = []
    stmts.each do |s|
      next if s.nil? || debug_print?(s)
      op = stmt(s, mvar)
      return nil if op.nil?
      out << op
    end
    out
  end

  # `puts/p/print/pp …` (optionally `… if @debug`) — stdout diagnostics that
  # never touch the token stream; elided like every native path.
  def self.debug_print?(n)
    return false unless node?(n)
    if %i[IF UNLESS].include?(n.type)
      cond, body, = n.children
      return false unless node?(cond) && cond.type == :IVAR
      ss = ((node?(body) && body.type == :BLOCK) ? body.children : [body]).compact
      return !ss.empty? && ss.all? { |x| print_call?(x) }
    end
    print_call?(n)
  end

  def self.print_call?(n)
    node?(n) && %i[FCALL VCALL].include?(n.type) && %i[puts p print pp].include?(n.children[0])
  end

  def self.stmt(n, mvar)
    return nil unless node?(n)
    case n.type
    when :FCALL, :VCALL, :CALL, :OPCALL
      call_stmt(n, mvar)
    when :IF, :UNLESS
      pred, a, b = n.children
      c = cond(pred, mvar); return nil if c.nil?
      t = branch(a, mvar); return nil if t.nil?
      e = branch(b, mvar); return nil if e.nil?
      ["if", n.type == :UNLESS ? ["not", c] : c, t, e]
    when :IASGN
      v = expr(n.children[1], mvar); return nil if v.nil?
      ["iset", n.children[0].to_s.sub(/\A@/, ""), v]
    when :CASE
      # `case m[i] (.downcase) when "a","b" then … else … end` over string
      # literals → desugar to nested `if gin` ops (CASE2 / non-string whens
      # decline).
      gr = group_ref(n.children[0], mvar); return nil if gr.nil?
      g, fold = gr
      chain = case_chain(n.children[1], g, fold, mvar)
      (chain && chain.size == 1) ? chain[0] : nil
    end
  end

  # An if/unless branch → [ops]; a nil branch (no else) is empty; failure
  # propagates as nil (distinct from the empty `[]`).
  def self.branch(n, mvar)
    n.nil? ? [] : stmt_list(n, mvar)
  end

  # Desugar a WHEN chain into nested `if gin` ops. `node` is a WHEN node, the
  # else-body, or nil. A WHEN yields a single `["if", gin, then, else]` op.
  def self.case_chain(node, g, fold, mvar)
    return [] if node.nil?
    return stmt_list(node, mvar) unless node?(node) && node.type == :WHEN
    list, body, nxt = node.children
    return nil unless node?(list) && list.type == :LIST
    items = list.children.compact
    return nil unless !items.empty? && items.all? { |e| node?(e) && e.type == :STR }
    lits = items.map { |e| e.children[0] }
    cnd = fold ? ["ginf", g, fold, lits] : ["gin", g, lits]
    then_ops = stmt_list(body, mvar); return nil if then_ops.nil?
    else_ops = case_chain(nxt, g, fold, mvar); return nil if else_ops.nil?
    [["if", cnd, then_ops, else_ops]]
  end

  def self.call_stmt(n, mvar)
    # `@ivar << [a, b, …]`
    if n.type == :OPCALL && n.children[1] == :<<
      recv, _op, args = n.children
      return nil unless node?(recv) && recv.type == :IVAR
      a = arglist(args)
      return nil unless a.size == 1 && node?(a[0]) && a[0].type == :LIST
      tuple = a[0].children.compact.map { |e| expr(e, mvar) }
      return nil if tuple.any?(&:nil?)
      return ["lpush", recv.children[0].to_s.sub(/\A@/, ""), tuple]
    end
    return nil unless %i[FCALL VCALL].include?(n.type) # bare-receiver DSL only
    mid = n.children[0]
    args = n.type == :FCALL ? arglist(n.children[1]) : []
    case mid
    when :token
      return nil unless args.size.between?(1, 2)
      tok = const_qualname(args[0]); return nil if tok.nil?
      return ["token", tok] if args.size == 1
      v = expr(args[1], mvar); return nil if v.nil?
      ["token", tok, v]
    when :groups
      return nil if args.empty?
      toks = args.map { |a| const_qualname(a) }
      toks.any?(&:nil?) ? nil : ["groups", toks]
    when :push
      return ["push", nil] if args.empty?
      (args.size == 1 && sym?(args[0])) ? ["push", args[0].children[0].to_s] : nil
    when :pop!
      return ["pop", 1] if args.empty?
      (args.size == 1 && int?(args[0])) ? ["pop", args[0].children[0]] : nil
    when :goto
      (args.size == 1 && sym?(args[0])) ? ["goto", args[0].children[0].to_s] : nil
    when :recurse
      # `recurse` / `recurse text` — re-lex with the same lexer. Bare → whole
      # match; an arg must be a compilable value expr.
      return ["recurse", ["g", 0]] if args.empty?
      return nil unless args.size == 1
      v = expr(args[0], mvar); v.nil? ? nil : ["recurse", v]
    when :delegate
      # `delegate <SelfClass>[, text]` is recurse (a fresh sub-lex of the SAME
      # class). ONLY self-delegation is handled here — resolving the target to
      # the very class being extracted is sound; delegating to ANOTHER lexer
      # needs the cross-lexer registry (not yet) → decline.
      return nil unless args.size.between?(1, 2)
      return nil unless resolve_lexer_const(args[0]).equal?(klass) && !klass.nil?
      text = args.size == 2 ? expr(args[1], mvar) : ["g", 0]
      text.nil? ? nil : ["recurse", text]
    end
  end

  def self.cond(n, mvar)
    return nil unless node?(n)
    # `c1 && c2` / `c1 || c2` — recurse both sides (each must be a valid cond).
    if %i[AND OR].include?(n.type)
      a = cond(n.children[0], mvar); return nil if a.nil?
      b = cond(n.children[1], mvar); return nil if b.nil?
      return [n.type == :AND ? "and" : "or", a, b]
    end
    return ["ivar", n.children[0].to_s.sub(/\A@/, "")] if n.type == :IVAR
    # `state?(:s)` / `in_state?(:s)` — rouge synonyms for "current top state".
    if %i[FCALL VCALL].include?(n.type) && %i[state? in_state?].include?(n.children[0])
      args = n.type == :FCALL ? arglist(n.children[1]) : []
      return (args.size == 1 && sym?(args[0])) ? ["instate", args[0].children[0].to_s] : nil
    end
    if n.type == :CALL && n.children[1] == :include?
      recv, _mid, args = n.children
      lits = string_set(recv); return nil if lits.nil?
      a = arglist(args); return nil unless a.size == 1
      gr = group_ref(a[0], mvar); return nil if gr.nil?
      g, fold = gr
      return fold ? ["ginf", g, fold, lits] : ["gin", g, lits]
    end
    if n.type == :OPCALL && %i[== !=].include?(n.children[1])
      recv, _op, args = n.children
      gr = group_ref(recv, mvar); return nil if gr.nil?
      g, fold = gr
      a = arglist(args)
      return nil unless a.size == 1 && node?(a[0]) && a[0].type == :STR
      eq = fold ? ["geqf", g, fold, a[0].children[0]] : ["geq", g, a[0].children[0]]
      return n.children[1] == :!= ? ["not", eq] : eq
    end
    if n.type == :OPCALL && n.children[1] == :!
      inner = cond(n.children[0], mvar)
      return inner.nil? ? nil : ["not", inner]
    end
    # `m[i] =~ /re/` — regex-match condition (unanchored). The regex literal
    # carries its source + flags; the engine compiles it (cond semantics).
    if n.type == :CALL && n.children[1] == :=~
      recv, _mid, args = n.children
      g = group_index(recv, mvar); return nil if g.nil?
      a = arglist(args)
      return nil unless a.size == 1 && node?(a[0]) && a[0].type == :REGX
      re = a[0].children[0]
      return nil unless re.is_a?(Regexp)
      return ["gmatch", g, re.source, re.options]
    end
    nil
  end

  def self.expr(n, mvar)
    return nil unless node?(n)
    # `m[i]` or a bare alias var → capture group value.
    g = group_index(n, mvar); return ["g", g] unless g.nil?
    case n.type
    when :STR then ["lit", n.children[0]]
    when :TRUE then ["bool", true]
    when :FALSE then ["bool", false]
    when :DSTR then interp(n, mvar)
    when :CALL
      if n.children[1] == :include?
        recv, _mid, args = n.children
        lits = string_set(recv); return nil if lits.nil?
        a = arglist(args); return nil unless a.size == 1
        gi = group_index(a[0], mvar); gi.nil? ? nil : ["gin", gi, lits]
      end
    end
  end

  # String interpolation `"a#{m[i]}b…"` → `["cat", parts…]`. Only literal
  # chunks and `#{m[i]}` group interpolations; anything else declines.
  # RubyVM DSTR nests: [head_str, EVSTR | LIST | nested DSTR, …].
  def self.interp(n, mvar)
    parts = []
    ok = flatten_dstr(n, mvar, parts)
    return nil unless ok
    parts.empty? ? nil : ["cat", *parts]
  end

  def self.flatten_dstr(n, mvar, parts)
    return false unless node?(n)
    n.children.each do |c|
      case c
      when nil then next
      when String then parts << ["lit", c]
      else
        return false unless node?(c)
        case c.type
        when :STR then parts << ["lit", c.children[0]]
        when :EVSTR
          g = group_index(c.children[0], mvar); return false if g.nil?
          parts << ["g", g]
        when :LIST, :DSTR
          return false unless flatten_dstr(c, mvar, parts)
        else
          return false
        end
      end
    end
    true
  end

  # ---- leaf helpers ----
  def self.arglist(n)
    return [] if n.nil?
    return n.children.compact if node?(n) && n.type == :LIST
    [n]
  end

  def self.sym?(n); node?(n) && n.type == :SYM; end
  def self.int?(n); node?(n) && n.type == :INTEGER; end

  # A group reference for a classifier condition: `[index, fold]` where fold
  # is "down"/"up" for `m[i].downcase`/`.upcase` (rouge's case-insensitive
  # classifiers), or nil for a plain `m[i]`/alias. nil if not a group ref.
  # Fold-aware resolution of a group reference → `[index, fold]` (fold is
  # nil / "down" / "up"), else nil. Handles `m[i]`, `m[i].downcase|upcase`,
  # and a bare alias var bound to any of those.
  def self.resolve_group(n, mvar)
    return nil unless node?(n)
    if n.type == :CALL && n.children[1] == :[]
      recv, _mid, args = n.children
      return nil unless node?(recv) && %i[DVAR LVAR].include?(recv.type) && recv.children[0] == mvar
      a = arglist(args)
      return nil unless a.size == 1 && node?(a[0]) && a[0].type == :INTEGER
      v = a[0].children[0]
      return (v.is_a?(Integer) && v >= 0) ? [v, nil] : nil
    end
    if n.type == :CALL && %i[downcase upcase].include?(n.children[1]) && arglist(n.children[2]).empty?
      inner = resolve_group(n.children[0], mvar)
      return nil if inner.nil? || !inner[1].nil? # no double-fold
      return [inner[0], n.children[1] == :downcase ? "down" : "up"]
    end
    if %i[DVAR LVAR].include?(n.type) && @aliases&.key?(n.children[0])
      return @aliases[n.children[0]].dup
    end
    nil
  end

  # A group reference in a CONDITION (fold-aware): `[index, fold]` or nil.
  def self.group_ref(n, mvar)
    resolve_group(n, mvar)
  end

  # `m[i]` / a bare UNFOLDED alias → capture index i (value position). A
  # folded group can't be emitted verbatim, so it returns nil there.
  def self.group_index(n, mvar)
    r = resolve_group(n, mvar)
    (r && r[1].nil?) ? r[0] : nil
  end

  # An array/`%w` literal of string literals → [String], else nil.
  def self.string_array(n)
    return nil unless node?(n) && n.type == :LIST
    els = n.children.compact
    return nil if els.empty? || !els.all? { |e| node?(e) && e.type == :STR }
    els.map { |e| e.children[0] }
  end

  # A string set in an `include?` receiver position: an inline literal array,
  # OR a CLASS-LEVEL data expression (`self.class.keywords`, a `CONST`) which
  # we evaluate against the lexer class. Sound because such data is
  # match-independent and memoized — the same words the runtime wordlist hook
  # would capture. Non-string / instance-dependent / unresolvable → nil.
  def self.string_set(n)
    return string_array(n) if node?(n) && n.type == :LIST
    v = eval_class_data(n)
    return nil unless v.respond_to?(:to_a)
    a = v.to_a
    (!a.empty? && a.all? { |x| x.is_a?(String) }) ? a : nil
  rescue StandardError
    nil
  end

  # Resolve a match-INDEPENDENT class-data node to its value: `self.class.M`
  # (no-arg accessor) or a bare `CONST`, against the lexer class. Anything
  # instance-dependent (`self.M`) or with arguments is refused (nil).
  def self.eval_class_data(n)
    return nil unless node?(n) && klass
    case n.type
    when :CONST
      klass.const_get(n.children[0])
    when :CALL
      recv, mid, args = n.children
      return nil unless arglist(args).empty?
      return nil unless node?(recv) && recv.type == :CALL && recv.children[1] == :class &&
                        node?(recv.children[0]) && recv.children[0].type == :SELF
      klass.respond_to?(mid) ? klass.public_send(mid) : nil
    end
  rescue StandardError
    nil
  end

  # Resolve a token constant node (`Keyword`, `Name::Builtin`, `Str`) to its
  # rouge qualname via the live token tree (handles aliases like Str →
  # Literal::String). nil if it isn't a resolvable constant path.
  def self.const_qualname(n)
    path = const_path(n)
    return nil if path.nil?
    tok = path.reduce(Rouge::Token::Tokens) { |mod, seg| mod.const_get(seg) }
    tok.respond_to?(:qualname) ? tok.qualname : nil
  rescue StandardError
    nil
  end

  def self.const_path(n)
    return nil unless node?(n)
    case n.type
    when :CONST then [n.children[0]]
    when :COLON3 then [n.children[0]]
    when :COLON2
      parent, name = n.children
      pp = parent.nil? ? [] : const_path(parent)
      pp.nil? ? nil : pp + [name]
    end
  end

  # Resolve a constant path (`Dart`, `Rouge::Lexers::Shell`) to the live
  # class, trying the lexer namespace then top level. Used to detect
  # self-delegation (`delegate <SelfClass>` == recurse).
  def self.resolve_lexer_const(n)
    path = const_path(n)
    return nil if path.nil?
    [defined?(Rouge::Lexers) ? Rouge::Lexers : Object, Object].each do |base|
      begin
        return path.reduce(base) { |m, s| m.const_get(s) }
      rescue StandardError
        next
      end
    end
    nil
  end
end

def try_ir(blk)
  IrCompiler.compile(blk)
end

# --- AST classifier validator (run the tool under `--parser=parse.y`) ---
# Runtime tracing alone is UNSOUND for `m[0] == "lit"` / `case m[0] when
# "lit"` branches: the literal owns the comparison (`"lit" == word` /
# `"lit" === word`), so the word probe never sees it → those words would be
# silently misclassified as default. The AST sees every branch explicitly,
# so we VALIDATE that a block is purely an if/case tree over `m[0]` whose
# conditions are only `==` / `include?` / `when`-literals with single-`token`
# leaves, and COLLECT the `==`/`when` literals (the `include?` sets are still
# captured at runtime). If anything is unrecognized → nil (→ callback). With
# the literals + sets forming a COMPLETE candidate set, the real-block
# tabulation below is sound.
module ClassifierAST
  N = RubyVM::AbstractSyntaxTree::Node
  def self.node?(x); x.is_a?(N); end
  def self.str?(x); node?(x) && x.type == :STR; end

  # The matched word: `m[0]` (CALL :[] on a `params` var, index 0) OR a bare
  # alias var `w` where `w = m[0]` was seen (in `aliases`).
  def self.m0?(n, params, aliases)
    return false unless node?(n)
    return true if %i[DVAR LVAR].include?(n.type) && aliases.include?(n.children[0])
    return false unless n.type == :CALL && n.children[1] == :[]
    recv, _mid, args = n.children
    return false unless node?(recv) && %i[DVAR LVAR].include?(recv.type) && params.include?(recv.children[0])
    a = node?(args) && args.type == :LIST ? args.children.compact : []
    a.size == 1 && node?(a[0]) && a[0].type == :INTEGER && a[0].children[0] == 0
  end

  # A recognized condition over the matched word; collects == literals.
  # `SET.include?(word)` is accepted (set captured at runtime) — SET is any
  # expression. `word == "lit"` / `"lit" == word` collects the literal.
  def self.cond?(c, params, aliases, lits)
    return false unless node?(c)
    if c.type == :CALL && c.children[1] == :include?
      _set, _mid, args = c.children
      a = node?(args) && args.type == :LIST ? args.children.compact : []
      return a.size == 1 && m0?(a[0], params, aliases)
    end
    if c.type == :OPCALL && c.children[1] == :==
      a, _op, args = c.children
      b = (node?(args) && args.type == :LIST) ? args.children.compact[0] : nil
      if m0?(a, params, aliases) && str?(b) then lits << b.children[0]; return true end
      if m0?(b, params, aliases) && str?(a) then lits << a.children[0]; return true end
    end
    false
  end

  # A leaf must be a single `token X` call (FCALL/VCALL :token).
  def self.leaf?(n)
    return true if n.nil?
    return false unless node?(n)
    %i[FCALL VCALL].include?(n.type) && n.children[0] == :token
  end

  def self.branch(n, params, aliases, lits)
    return true if leaf?(n)
    return false unless node?(n)
    case n.type
    when :IF, :UNLESS
      cond, a, b = n.children
      cond?(cond, params, aliases, lits) && branch(a, params, aliases, lits) && branch(b, params, aliases, lits)
    when :CASE, :CASE2
      subj = n.children[0]
      return false unless subj.nil? || m0?(subj, params, aliases)
      n.children[1..].all? { |w| branch(w, params, aliases, lits) }
    when :WHEN
      list, body, nxt = n.children
      return false unless node?(list) && list.type == :LIST
      items = list.children.compact # LIST has a trailing nil terminator
      return false unless items.all? { |e| str?(e) && (lits << e.children[0]) }
      branch(body, params, aliases, lits) && branch(nxt, params, aliases, lits)
    else
      false
    end
  end

  # Returns the ==/when literals if `blk` is a validated pure classifier,
  # else nil. nil also when the AST is unavailable (Prism build w/o parse.y).
  def self.literals(blk)
    scope = begin
      RubyVM::AbstractSyntaxTree.of(blk)
    rescue StandardError
      return nil
    end
    return nil unless node?(scope) && scope.type == :SCOPE
    param = (scope.children[0] || []).first
    return nil if param.nil?
    params = [param]
    aliases = []
    body = scope.children[2]
    # Unwrap a BLOCK body's leading `w = m[0]` alias assignments.
    if node?(body) && body.type == :BLOCK
      stmts = body.children
      stmts[0..-2].each do |s|
        return nil unless node?(s) && %i[LASGN DASGN].include?(s.type) && m0?(s.children[1], params, aliases)
        aliases << s.children[0]
      end
      body = stmts.last
    end
    lits = []
    branch(body, params, aliases, lits) ? lits.uniq : nil
  end
end

def try_wordlist(blk)
  # SOUNDNESS gate: only a structurally-validated pure classifier (AST)
  # may be upgraded — this captures the ==/when literals runtime tracing
  # can't see, so the candidate set is COMPLETE.
  ast_lits = ClassifierAST.literals(blk)
  return nil if ast_lits.nil?
  $wl_sets = []
  $wl_capture = true
  dctx = WLCtx.new
  begin
    dctx.instance_exec(WLMatch.new(WL_WORD), &blk)
  rescue Exception
    return nil
  ensure
    $wl_capture = false
  end
  default = dctx.tok
  return nil if default.nil?
  words = ast_lits.dup
  $wl_sets.each do |s|
    return nil unless s.respond_to?(:each)
    s.each { |w| words << w if w.is_a?(String) }
  end
  words.uniq!
  return nil if words.empty?
  order = []
  by_tok = {}
  words.each do |w|
    c = WLCtx.new
    begin
      c.instance_exec(WLMatch.new(w), &blk)
    rescue Exception
      return nil
    end
    return nil if c.tok.nil?
    next if c.tok == default
    (by_tok[c.tok] ||= (order << c.tok; []))
    by_tok[c.tok] << w
  end
  { sets: order.map { |t| [t, by_tok[t]] }, default: default }
end

class Recorder
  attr_reader :rules
  def initialize; @rules = []; end
  def rule(re, tok = nil, next_state = nil, &blk)
    if blk
      @rules << classify_block(re, blk)
    else
      ns = case next_state when nil then nil when Array then next_state.map(&:to_s) else next_state.to_s end
      @rules << { kind: "tok", re: re.source, opts: re.options, tok: tok.qualname, next: ns }
    end
  end

  # Classify a rule BLOCK. A block that performs only static DSL calls
  # (no match access) records as `actions`. Otherwise it's match-dependent
  # — TraceCtx#token even Bails on a value token (`token T, m[1]`), so the
  # match-dependent path is reached BOTH on `probe.touched?` AND on a Bail.
  # We then try a sound upgrade (wordlist classifier / straight-line IR);
  # anything else stays a `callback`.
  def classify_block(re, blk)
    ctx = TraceCtx.new
    probe = MatchProbe.new
    begin
      ctx.instance_exec(probe, &blk)
      unless probe.touched?
        return { kind: "actions", re: re.source, opts: re.options, actions: ctx.actions }
      end
    rescue StandardError
      # match-dependent (value token, dynamic state, unmodeled call) → fall
      # through to the sound-upgrade attempts below.
    end
    if (wl = try_wordlist(blk))
      { kind: "wordlist", re: re.source, opts: re.options, sets: wl[:sets], default: wl[:default] }
    elsif (ir = try_ir(blk))
      { kind: "ir", re: re.source, opts: re.options, ops: ir }
    else
      { kind: "callback", re: re.source, opts: re.options }
    end
  end
  def mixin(name); @rules << { kind: "mixin", state: name.to_s }; end
end

def extract_table(lx)
  IrCompiler.klass = lx # enable self.class.<data> resolution for this lexer
  states = {}
  lx.state_definitions.each do |name, dsl|
    rec = Recorder.new
    rec.instance_eval(&dsl.instance_variable_get(:@defn))
    states[name] = rec.rules
  end
  # `start { push :foo }` blocks set the initial stack above :root. Trace
  # each (no-arg, run on the lexer at reset) for its pushes; ivar inits
  # like `@q = []` are no-ops here (carmine lazily treats ivars as empty).
  # An uncapturable start (calls beyond push/ivar) Bails → empty (best
  # effort; surfaces as a divergence in the harness rather than silently).
  start_push = []
  (lx.start_procs || []).each do |pr|
    ctx = TraceCtx.new
    begin
      ctx.instance_exec(&pr)
      ctx.actions.each { |a| start_push << a[1] if a[0] == "push" }
    rescue StandardError
      # leave whatever pushes were captured before the Bail
    end
  end
  shortnames = {}
  Rouge::Token.each_token { |t| shortnames[t.qualname] = t.shortname }
  { lexer: lx.name, start_push: start_push, states: states, shortnames: shortnames }
end

manifest = []
Rouge::Lexer.all.sort_by(&:tag).each do |lx|
  # rouge's own demo reader; lexers without a demo file raise → skip.
  demo = (lx.demo rescue nil)
  next if demo.nil?
  tag = lx.tag
  rec = { tag: tag, class: lx.name }
  File.write(File.join(OUT, "#{tag}.demo"), demo)
  begin
    table = extract_table(lx)
    rec[:callback_rules] = table[:states].values.flatten.count { |r| r[:kind] == "callback" }
    rec[:total_rules]    = table[:states].values.flatten.size
    File.write(File.join(OUT, "#{tag}.table.json"), JSON.generate(table))
  rescue => e
    rec[:extract_error] = "#{e.class}: #{e.message[0, 60]}"
  end
  begin
    golden = lx.new.lex(demo).map { |tok, val| [tok.qualname, val] }
    File.write(File.join(OUT, "#{tag}.golden.json"), JSON.generate(golden))
    rec[:golden_tokens] = golden.size
  rescue => e
    rec[:golden_error] = "#{e.class}: #{e.message[0, 60]}"
  end
  manifest << rec
end
File.write(File.join(OUT, "manifest.json"), JSON.pretty_generate(manifest))
cb = manifest.count { |r| (r[:callback_rules] || 0) > 0 }
puts "lexers=#{manifest.size} with_callback_rules=#{cb} extract_errors=#{manifest.count { |r| r[:extract_error] }} golden_errors=#{manifest.count { |r| r[:golden_error] }}"
