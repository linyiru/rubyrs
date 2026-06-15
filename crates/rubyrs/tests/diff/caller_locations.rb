# Kernel#caller_locations — like caller but returns
# Thread::Backtrace::Location objects (.path / .lineno / .label /
# .absolute_path / .base_label / .to_s). zeitwerk's loader reads
# caller_locations(1, 1).first.path. Asserts path-basename + lineno +
# shape (the absolute_path-vs-path distinction is a filesystem-symlink
# detail, and .label inherits caller's class-prefix divergence, so
# neither is asserted here).

def deep_a; deep_b; end
def deep_b; deep_c; end
def deep_c
  locs = caller_locations
  [locs.class, locs.size >= 2]
end

cls, deep = deep_a
p cls
p deep

def two; one; end
def one
  locs = caller_locations(1, 2)
  locs.map { |l| [File.basename(l.path), l.lineno, l.is_a?(Thread::Backtrace::Location)] }
end
p two

# Single-frame slice — zeitwerk's exact shape.
def caller_path
  caller_locations(1, 1).first.path
end
def calls_it; caller_path; end
p File.basename(calls_it)

# to_s round-trips path:lineno:in 'label'
def show; caller_locations(1, 1).first.to_s; end
def host; show; end
s = host
p(s.start_with?(File.expand_path(__FILE__)) || s.include?("caller_locations.rb"))
p s.include?(":in '")

# Top-level: empty (no caller frames above main).
p caller_locations.class
p caller_locations(1).is_a?(Array)
