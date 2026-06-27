# YAML.safe_load_file / unsafe_load_file (aliases of load_file) parse a YAML
# file from disk. Bridgetown's _data loader uses safe_load_file for site
# metadata; without it the data files silently fail to load.
require "yaml"
path = "/tmp/rubyrs_yaml_sl_fixture.yml"
File.write(path, "title: My Site\nauthor: Ada\ncount: 3\ntags:\n  - a\n  - b\n")
data = YAML.safe_load_file(path)
p data["title"]            # "My Site"
p data["author"]           # "Ada"
p data["count"]            # 3
p data["tags"]             # ["a", "b"]
p YAML.respond_to?(:safe_load_file)    # true
p YAML.safe_load_file(path) == YAML.load_file(path)  # true
File.delete(path)
