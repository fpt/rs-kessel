#!/bin/bash
# Claude Code hook: sends events to kessel-cli via Unix domain socket.
# Usage: Add to .claude/settings.json hooks configuration.
# Reads hook JSON from stdin, forwards to kessel-cli's Unix socket.
# Exits silently if kessel-cli is not running (socket doesn't exist).

SOCKET="/tmp/kessel-cli-$(id -u).sock"

# Check if socket exists
if [ ! -S "$SOCKET" ]; then
    exit 0
fi

# Read stdin (hook JSON) and inject cwd for session identification
INPUT=$(cat)
INPUT="${INPUT%\}},\"cwd\":\"$PWD\"}"

# Forward to Unix socket
if command -v socat &>/dev/null; then
    echo "$INPUT" | socat - UNIX-CONNECT:"$SOCKET" 2>/dev/null
elif command -v python3 &>/dev/null; then
    python3 -c "
import socket, sys
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
try:
    s.connect('$SOCKET')
    s.sendall(sys.stdin.buffer.read())
    s.sendall(b'\n')
    s.close()
except:
    pass
" <<< "$INPUT"
fi

exit 0
