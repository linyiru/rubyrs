# Minimal `rack/protection` umbrella for the rack_protection_smoke
# fixture — the real umbrella autoloads every middleware and
# composes them via Rack::Builder.new; we only need the three
# explicitly-required middlewares (FrameOptions, XSSHeader,
# PathTraversal) for this fixture, so the umbrella is just the
# module-namespace anchor + eager require of Base, which each
# middleware file requires via `require 'rack/protection'`.
# (Real autoload would defer this; rubyrs doesn't yet fire
# autoload on constant access, so eager require is the portable
# choice.)

module Rack
  module Protection
  end
end

require "rack/protection/base"
