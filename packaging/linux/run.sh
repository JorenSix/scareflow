#!/bin/sh
# Starts the show editor on the desktop session, also when launched over ssh.
cd "$(dirname "$0")"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
export WAYLAND_DISPLAY="${WAYLAND_DISPLAY:-wayland-0}"
exec ./scareflow-showui showui.toml
