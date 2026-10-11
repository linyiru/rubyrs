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
# with the job's and the step's `env:` applied. `uses:` steps (checkout,
# toolchain and cache actions) are not runnable here and cannot be named.
# A step whose script contains a `${{ … }}` expression is refused: it
# needs GitHub's runner context.
require "yaml"

ROOT = File.expand_path("..", __dir__)
WORKFLOW = File.join(ROOT, ".github/workflows/ci.yml")

def die(msg, code = 2) = (warn "ci-step: #{msg}"; exit code)

def run_steps(job) = (job["steps"] || []).select { |s| s["run"] }

mode = :run
case ARGV.first
when "--print" then mode = :print; ARGV.shift
when "--list" then mode = :list; ARGV.shift
end

jobs = YAML.load_file(WORKFLOW).fetch("jobs")
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
  env = (job["env"] || {}).merge(s["env"] || {}).transform_values(&:to_s)
  if mode == :print
    puts "# #{job_id} / #{s['name']}"
    env.each { |k, v| puts "export #{k}=#{v}" }
    puts script, ""
    next
  end
  env["GITHUB_STEP_SUMMARY"] ||= ENV.fetch("GITHUB_STEP_SUMMARY", "/dev/null")
  puts "ci-step: #{job_id} / #{s['name']}"
  ok = system(env, "bash", "--noprofile", "--norc", "-eo", "pipefail", "-c", script, chdir: ROOT)
  exit($?.exitstatus || 1) unless ok
end
