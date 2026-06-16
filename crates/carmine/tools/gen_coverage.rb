# Coverage-harness generator (companion to examples/coverage.rs). For every
# rouge lexer, extract its rule table into carmine's JSON format AND record
# rouge's own token stream over the lexer's demo (the golden) + the demo
# text itself. The Rust harness then lexes each demo with carmine and diffs
# against the golden — a measurable carmine↔rouge coverage baseline for the
# drop-in-replacement work. Output is self-contained (no rouge path needed
# by the Rust side).
#
#   ruby crates/carmine/tools/gen_coverage.rb [ROUGE_LIB_DIR]
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

def try_wordlist(blk)
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
  return nil if default.nil? || $wl_sets.empty?
  words = []
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
      ctx = TraceCtx.new
      probe = MatchProbe.new
      begin
        ctx.instance_exec(probe, &blk)
        if probe.touched?
          # Match-dependent. Try the wordlist upgrade (pure include?-based
          # keyword classifier → native `wordlist`); else it's a callback.
          if (wl = try_wordlist(blk))
            @rules << { kind: "wordlist", re: re.source, opts: re.options,
                        sets: wl[:sets], default: wl[:default] }
          else
            @rules << { kind: "callback", re: re.source, opts: re.options }
          end
        else
          @rules << { kind: "actions", re: re.source, opts: re.options, actions: ctx.actions }
        end
      rescue StandardError
        @rules << { kind: "callback", re: re.source, opts: re.options }
      end
    else
      ns = case next_state when nil then nil when Array then next_state.map(&:to_s) else next_state.to_s end
      @rules << { kind: "tok", re: re.source, opts: re.options, tok: tok.qualname, next: ns }
    end
  end
  def mixin(name); @rules << { kind: "mixin", state: name.to_s }; end
end

def extract_table(lx)
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
