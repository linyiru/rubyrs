#!/usr/bin/env bash
# verify.sh — run the SAME app.rb on CRuby and rubyrs, hit the same routes,
# and prove the responses are identical.
#
# Usage:
#   poc/sinatra/verify.sh                 # build rubyrs if needed, run both
#   RUBYRS_BIN=/path/to/rubyrs poc/sinatra/verify.sh
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
APP="$HERE/app.rb"
PORT="${PORT:-9292}"
RUBYRS_BIN="${RUBYRS_BIN:-$ROOT/target/debug/rubyrs}"

if [ ! -x "$RUBYRS_BIN" ]; then
  echo "Building rubyrs (_http_server,_fiber)…"
  ( cd "$ROOT" && cargo build -p rubyrs --features _http_server,_fiber ) || exit 1
fi

# Probe one interpreter: start server, curl the routes, print a normalized
# transcript (status + body per route) to stdout, then stop the server.
probe() {
  local label="$1"; shift
  # Free the port first: a previous run's server (app.rb's default
  # duration is long) could still be squatting on it, which would make
  # this probe silently hit the wrong server.
  local squatter
  squatter=$(lsof -ti "tcp:$PORT" 2>/dev/null)
  [ -n "$squatter" ] && kill $squatter 2>/dev/null && sleep 0.3
  "$@" "$APP" >/tmp/sin_${label}.log 2>&1 &
  local pid=$!
  # wait for the port to open
  for _ in $(seq 1 50); do
    curl -s -o /dev/null "http://127.0.0.1:$PORT/" && break
    sleep 0.1
  done

  # Each entry: "<id>". The per-id curl args live in the case below so we
  # can vary method / body / headers per route.
  local routes=(
    "GET /"
    "GET /hello/world"
    "GET /hello/%3Cb%3E"
    "GET /search?q=hello+world&limit=5"
    "GET /say/cats/to/dogs"
    "GET /admin"
    "GET /old"
    "GET /teapot"
    "POST /echo"
    "POST /form"
    "GET /whoami"
    "GET /prefs"
    "GET /feature"
    "GET /feature?skip=yes"
    "PUT /resource/42"
    "GET /validate"
    "GET /stream"
    "GET /no-such-route"
  )
  for route in "${routes[@]}"; do
    local method="${route%% *}" path="${route#* }"
    local code
    case "$method $path" in
      "POST /echo")
        # text/plain so Rack doesn't consume the body as form params on the
        # CRuby side — keeps request.body.read intact on both runtimes.
        code=$(curl -s -D /tmp/sin_hdr -o /tmp/sin_body -w "%{http_code}" -X POST \
          -H 'Content-Type: text/plain' --data 'ping from the same app.rb' \
          "http://127.0.0.1:$PORT$path") ;;
      "POST /form")
        # default content-type is application/x-www-form-urlencoded
        code=$(curl -s -D /tmp/sin_hdr -o /tmp/sin_body -w "%{http_code}" -X POST \
          --data 'name=Ruby&city=Tokyo' \
          "http://127.0.0.1:$PORT$path") ;;
      "GET /whoami")
        code=$(curl -s -D /tmp/sin_hdr -o /tmp/sin_body -w "%{http_code}" \
          -A 'poc-agent/1.0' "http://127.0.0.1:$PORT$path") ;;
      "GET /prefs")
        code=$(curl -s -D /tmp/sin_hdr -o /tmp/sin_body -w "%{http_code}" \
          --cookie 'theme=dark' "http://127.0.0.1:$PORT$path") ;;
      "PUT "*)
        code=$(curl -s -D /tmp/sin_hdr -o /tmp/sin_body -w "%{http_code}" -X PUT \
          "http://127.0.0.1:$PORT$path") ;;
      *)
        # No -L: we want to SEE the 302, not follow it.
        code=$(curl -s -D /tmp/sin_hdr -o /tmp/sin_body -w "%{http_code}" -X "$method" "http://127.0.0.1:$PORT$path") ;;
    esac
    echo "### $method $path -> $code"
    # Surface the Location header for redirects (normalized, parity-checked).
    local loc
    loc=$(grep -i '^location:' /tmp/sin_hdr | tr -d '\r' | sed 's/^[Ll]ocation: */Location: /')
    [ -n "$loc" ] && echo "$loc"
    echo "--body--"
    cat /tmp/sin_body
    echo "--end--"
  done

  kill "$pid" 2>/dev/null
  wait "$pid" 2>/dev/null
}

echo "==================== CRuby + real Sinatra ===================="
probe cruby ruby | tee /tmp/sin_cruby.txt
echo
echo "==================== rubyrs micro-Sinatra ===================="
probe rubyrs "$RUBYRS_BIN" | tee /tmp/sin_rubyrs.txt
echo

echo "==================== DIFF ===================="
# Normalize the ONE intentional difference: each runtime self-reports its
# own name via SERVER_BACKEND, which is the proof that two different engines
# ran the same source. Everything else must match byte-for-byte.
norm() { sed -E 's/(CRuby \+ Sinatra [0-9.]+|rubyrs micro-Sinatra \(_http_server battery\))/<this-runtime>/g' "$1"; }
norm /tmp/sin_cruby.txt  > /tmp/sin_cruby_n.txt
norm /tmp/sin_rubyrs.txt > /tmp/sin_rubyrs_n.txt

if diff -u /tmp/sin_cruby_n.txt /tmp/sin_rubyrs_n.txt; then
  echo
  echo "✅ IDENTICAL (modulo the self-reported runtime name): the same app.rb"
  echo "   produced byte-identical responses on CRuby+Sinatra and rubyrs."
else
  echo "❌ responses differ beyond the runtime label (see diff above)"
  exit 1
fi
