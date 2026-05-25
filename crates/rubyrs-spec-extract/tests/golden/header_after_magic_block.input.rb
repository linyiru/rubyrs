#!/usr/bin/env ruby
# encoding: utf-8
# frozen_string_literal: false

# Regular comment — body starts here.
describe "skip log placement" do
  before :all do
    @x = 1
  end

  it "needs hand work" do
    @x
  end
end
