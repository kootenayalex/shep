#!/bin/sh
# A throwaway shep server + bridge for live checks of the companion, the CLI
# and `shep mcp`, without touching the launchd `dev.shep.server` that owns the
# real agents. Debug builds use the `shep-dev` config/state dirs, so nothing
# here reaches `~/.config/shep`.
#
#   scripts/dev-stack.sh up      build (if needed), start server + bridge, link the overseer plugin
#   scripts/dev-stack.sh status  pids, socket, `shep status`
#   scripts/dev-stack.sh pair    print the pairing QR/values for the companion
#   scripts/dev-stack.sh down    stop both and remove the sockets
#
# The AVD reaches the bridge at ws://10.0.2.2:7432/. Override with
# SHEP_DEV_STACK_DIR (default /tmp/shep-dev — keep it short: macOS caps unix
# socket paths at 104 chars and shep derives a `-client` sibling) and
# SHEP_DEV_BIND (default 127.0.0.1:7432).
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
dir=${SHEP_DEV_STACK_DIR:-/tmp/shep-dev}
bind=${SHEP_DEV_BIND:-127.0.0.1:7432}
bin="$root/target/debug/shep"
sock="$dir/api.sock"

# The nested-shep guard and the client-socket derivation both read SHEP_*/HERDR_*
# from the environment; a dev stack must start from a clean slate.
clean_env() {
    for v in $(env | sed -n 's/^\(SHEP_[A-Z_]*\|HERDR_[A-Z_]*\)=.*/\1/p'); do
        unset "$v"
    done
}

alive() { [ -f "$1" ] && kill -0 "$(cat "$1")" 2>/dev/null; }

seed_config() {
    # Debug builds read ~/.config/shep-dev; the board's chat needs a headless runtime.
    cfg="$HOME/.config/shep-dev/config.toml"
    mkdir -p "$(dirname "$cfg")"
    if ! grep -q '^\[plugins\.overseer\]' "$cfg" 2>/dev/null; then
        printf '\n[plugins.overseer]\nruntime = "claude"\n' >>"$cfg"
        echo "seeded [plugins.overseer] runtime = \"claude\" in $cfg"
    fi
}

up() {
    mkdir -p "$dir"
    [ -x "$bin" ] || (cd "$root" && cargo build --bin shep)
    seed_config
    clean_env
    if alive "$dir/server.pid"; then
        echo "server already up (pid $(cat "$dir/server.pid"))"
    else
        rm -f "$sock" "$sock-client"
        (cd "$root" && SHEP_SOCKET_PATH="$sock" nohup "$bin" server >"$dir/server.log" 2>&1 &
         echo $! >"$dir/server.pid")
        i=0
        until [ -S "$sock" ] || [ $i -ge 50 ]; do sleep 0.2; i=$((i+1)); done
        [ -S "$sock" ] || { echo "server did not bind $sock; see $dir/server.log" >&2; exit 1; }
        echo "server up (pid $(cat "$dir/server.pid"), socket $sock)"
    fi
    if alive "$dir/bridge.pid"; then
        echo "bridge already up (pid $(cat "$dir/bridge.pid"))"
    else
        # `--bind` must be the first argument: the bridge dispatches on argv[1].
        (cd "$root" && SHEP_SOCKET_PATH="$sock" nohup "$bin" bridge --bind "$bind" --socket "$sock" >"$dir/bridge.log" 2>&1 &
         echo $! >"$dir/bridge.pid")
        sleep 0.5
        alive "$dir/bridge.pid" || { echo "bridge exited; see $dir/bridge.log" >&2; exit 1; }
        echo "bridge up (pid $(cat "$dir/bridge.pid"), ws://$bind/)"
    fi
    SHEP_SOCKET_PATH="$sock" "$bin" plugin link "$root/plugins/overseer" >/dev/null 2>&1 \
        && echo "overseer plugin linked" || echo "overseer plugin already linked (or link refused; see \`shep plugin list\`)"
    echo "AVD: ws://10.0.2.2:${bind##*:}/   pair: scripts/dev-stack.sh pair"
}

status() {
    clean_env
    for p in server bridge; do
        if alive "$dir/$p.pid"; then echo "$p: up (pid $(cat "$dir/$p.pid"))"; else echo "$p: down"; fi
    done
    [ -S "$sock" ] && echo "socket: $sock" || echo "socket: absent"
    if [ -S "$sock" ]; then SHEP_SOCKET_PATH="$sock" "$bin" status 2>&1 | head -5; fi
}

pair() {
    clean_env
    SHEP_SOCKET_PATH="$sock" "$bin" bridge pair --host "$bind" --no-wait
}

down() {
    for p in bridge server; do
        if alive "$dir/$p.pid"; then
            pid=$(cat "$dir/$p.pid")
            kill "$pid" 2>/dev/null || true
            i=0
            while kill -0 "$pid" 2>/dev/null && [ $i -lt 50 ]; do sleep 0.1; i=$((i+1)); done
            if kill -0 "$pid" 2>/dev/null; then kill -9 "$pid" 2>/dev/null || true; echo "$p killed (ignored SIGTERM)"; else echo "$p stopped"; fi
        fi
        rm -f "$dir/$p.pid"
    done
    rm -f "$sock" "$sock-client"
}

case "${1:-}" in
    up) up ;;
    status) status ;;
    pair) pair ;;
    down) down ;;
    *) sed -n '2,15p' "$0" >&2; exit 2 ;;
esac
