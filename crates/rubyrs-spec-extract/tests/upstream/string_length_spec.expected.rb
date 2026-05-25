
# rubyrs-spec-extract v0.3: 1 pattern(s) left for hand polish.
# Each entry names the upstream line + reason. Address each
# (comment out, inline, or wait for a later extractor version)
# before the file is consumable by the micro-runner.
#
#   - L6: `it_behaves_like` — shared-example name not found in the supplied --shared registry (or none supplied); pass the matching `shared/...` file via `--shared <path>` to inline, or hand-translate

describe "String#length" do
  it_behaves_like :string_length, :length
end
