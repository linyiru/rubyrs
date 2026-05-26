# Brewfile-shape DSL workload for the wasm perf gate. Mirrors the
# real-shape `examples/brewfile/Brewfile.rb` plus the host-side
# wrapper that `examples/brewfile.rs` provides — but inlined into
# one file so the wasm CLI (which can't `register_fn` from the
# host side) can drive the same shape standalone.
#
# Measures end-to-end "cwasm spawn + rubyrs init + parse +
# compile + dispatch" against a workload that exercises the
# embed-niche thesis: a real DSL with declarations, a class def,
# and a couple of `.each` loops. The wall-time number goes into
# `perf/wasm_baselines.tsv` as the P2-A pivot signal.

$taps     = []
$formulae = []
$casks    = []
$mas      = []

def tap(name);      $taps     << name        end
def brew(name);     $formulae << name        end
def cask(name);     $casks    << name        end
def mas(name, id);  $mas      << [name, id]  end

# --- The Brewfile.rb body, inlined verbatim from
#     examples/brewfile/Brewfile.rb so the two workloads stay
#     byte-identical aside from the host-wrapper boilerplate. ---

tap "homebrew/cask"
tap "homebrew/cask-fonts"
tap "homebrew/services"
tap "neovim/neovim"
tap "shopify/shopify"

brew "git"
brew "ruby"
brew "node"
brew "go"
brew "rust"
brew "python@3.12"
brew "wget"
brew "curl"
brew "jq"
brew "ripgrep"
brew "fzf"
brew "fd"
brew "bat"
brew "htop"
brew "tmux"
brew "neovim"
brew "tree"
brew "gh"
brew "wasmtime"
brew "deno"
brew "bun"

cask "firefox"
cask "iterm2"
cask "visual-studio-code"
cask "docker"
cask "rectangle"
cask "raycast"
cask "obsidian"
cask "slack"
cask "zoom"
cask "spotify"

mas "Xcode", 497799835
mas "Magnet", 441258766

extras = ["1password", "linear"]
extras.each { |name| cask name }

class GroupedBrew
  def initialize(group)
    @group = group
  end

  def add(name)
    brew name
  end
end

dev = GroupedBrew.new("dev")
["maven", "gradle", "leiningen"].each { |t| dev.add(t) }

if "ENV_PROD" == "ENV_PROD"
  brew "redis"
  brew "postgresql@16"
end

# --- Summary printf, same shape as cruby_runner.rb so the two are
#     directly comparable when we run both under their respective
#     wasm shells.

# Summary uses `puts` (not `printf`) so it stays inside the Tier 1
# subset — printf-flavoured number formatting isn't on the rubyrs
# fast path. The shape of work is the same as cruby_runner's
# version: 4 dispatched method calls on Array#length + string
# interpolation.
puts "Collected Brewfile contents:"
puts "  #{$taps.length} taps"
puts "  #{$formulae.length} formulae (brew)"
puts "  #{$casks.length} casks"
puts "  #{$mas.length} mas apps"
