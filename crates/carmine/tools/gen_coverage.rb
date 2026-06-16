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
class Recorder
  attr_reader :rules
  def initialize; @rules = []; end
  def rule(re, tok = nil, next_state = nil, &blk)
    if blk
      ctx = TraceCtx.new
      begin
        ctx.instance_exec(:__stub__, &blk)
        @rules << { kind: "actions", re: re.source, opts: re.options, actions: ctx.actions }
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
  shortnames = {}
  Rouge::Token.each_token { |t| shortnames[t.qualname] = t.shortname }
  { lexer: lx.name, states: states, shortnames: shortnames }
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
