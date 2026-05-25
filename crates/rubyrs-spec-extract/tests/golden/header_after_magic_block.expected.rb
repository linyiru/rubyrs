#!/usr/bin/env ruby
# encoding: utf-8
# frozen_string_literal: false

# rubyrs-spec-extract v0.3: 1 pattern(s) left for hand polish.
# Each entry names the upstream line + reason. Address each
# (comment out, inline, or wait for a later extractor version)
# before the file is consumable by the micro-runner.
#
#   - L7: `before` — only the bare `before :each do ... end` form is lifted (no extra args, all sibling `it`s must have bodies); other forms like `before :all` or `before :each, :foo` pass through and need hand polish

# Regular comment — body starts here.
describe "skip log placement" do
  before :all do
    @x = 1
  end

  it "needs hand work" do
    @x
  end
end
