# A real-shape Brewfile-style declaration script. Methods are provided by
# the Rust host via Runtime::register_fn (see examples/brewfile.rs).

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

# A small bit of imperative Ruby in among the declarative DSL, to show
# that this is a full runtime, not a config parser.
extras = ["1password", "linear"]
extras.each { |name| cask name }

# Class-defined helpers also work in the same script (Tier 1 + P2-C
# language completeness shows here).
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
