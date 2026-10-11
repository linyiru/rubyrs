# scripts/ci-step.rb — run a CI job's `run:` steps locally, verbatim.
#
#   ruby scripts/ci-step.rb <job> [<step name>...]   run them (all run: steps if none named)
#   ruby scripts/ci-step.rb --print <job> [<step>...] print the shell each would run
#   ruby scripts/ci-step.rb --list [<job>]            list jobs, or one job's run: steps
#
# Why: scripts/check.sh is built from these, so a local check runs the
# exact commands .github/workflows/ci.yml runs instead of a copy that
# drifts. A step name that no longer exists is an error (exit 2), which
# is how a CI rename shows up locally instead of silently skipping.
#
# Each step runs the way GitHub's default shell does
# (`bash --noprofile --norc -eo pipefail`), from the repository root,
# with the workflow's, the job's and the step's `env:` applied. `uses:` steps (checkout,
# toolchain and cache actions) are not runnable here and cannot be named.
# A step whose script contains a `${{ … }}` expression is refused: it
# needs GitHub's runner context.
require "yaml"
require "shellwords"

ROOT = File.expand_path("..", __dir__)
WORKFLOW = File.join(ROOT, ".github/workflows/ci.yml")

# Plain `def … end` on purpose: a stale system Ruby (macOS ships 2.6) must
# be able to parse this file, so the error a user sees is doctor's
# "install CRuby 3.4", not a syntax error.
def die(msg, code = 2)
  warn "ci-step: #{msg}"
  exit code
end

def run_steps(job)
  (job["steps"] || []).select { |s| s["run"] }
end

mode = :run
case ARGV.first
when "--print" then mode = :print; ARGV.shift
when "--list" then mode = :list; ARGV.shift
end

workflow = YAML.load_file(WORKFLOW)
jobs = workflow.fetch("jobs")
if mode == :list && ARGV.empty?
  jobs.each_key { |k| puts k }
  exit 0
end

job_id = ARGV.shift or die("usage: ruby scripts/ci-step.rb [--print|--list] <job> [<step name>...]")
job = jobs[job_id] or die("no job #{job_id.inspect} in ci.yml (jobs: #{jobs.keys.join(', ')})")
available = run_steps(job)
if mode == :list
  available.each { |s| puts s["name"] }
  exit 0
end

steps = if ARGV.empty?
  available
else
  ARGV.map do |name|
    available.find { |s| s["name"] == name } or
      die("job #{job_id} has no run: step named #{name.inspect}; it has:\n  " + available.map { |s| s["name"].inspect }.join("\n  "))
  end
end

steps.each do |s|
  script = s["run"]
  die("step #{s['name'].inspect} uses a GitHub expression (${{ … }}); it cannot run locally") if script.include?("${{")
  # GitHub's precedence: workflow < job < step. The workflow level carries
  # RUSTFLAGS=-D warnings, so leaving it out would build with a laxer
  # warning policy than CI.
  env = [workflow, job, s].map { |h| h["env"] || {} }.reduce(:merge).transform_values(&:to_s)
  # Colour is cosmetic; a caller writing logs to files (check.sh) keeps its own.
  env.delete("CARGO_TERM_COLOR") if ENV.key?("CARGO_TERM_COLOR")
  if mode == :print
    puts "# #{job_id} / #{s['name']}"
    env.each { |k, v| puts "export #{k}=#{v.shellescape}" }
    puts script, ""
    next
  end
  env["GITHUB_STEP_SUMMARY"] ||= ENV.fetch("GITHUB_STEP_SUMMARY", "/dev/null")
  puts "ci-step: #{job_id} / #{s['name']}"
  ok = system(env, "bash", "--noprofile", "--norc", "-eo", "pipefail", "-c", script, chdir: ROOT)
  exit($?.exitstatus || 1) unless ok
end
