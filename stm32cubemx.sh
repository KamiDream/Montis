#!/bin/sh
# STM32CubeMX launcher for non-reparenting Wayland compositors (niri, sway, ...).
#
# STM32CubeMX 6.x uses Java Swing/AWT. Without this variable AWT assumes a
# reparenting window manager: the toplevel window is resized by the compositor,
# but the Swing content pane keeps its initial size (~1430x780). The result is
# a clipped UI in small tiles and large blank margins in big windows.
#
# Usage:
#   ./stm32cubemx.sh            # start normally
#   ./stm32cubemx.sh file.iocx  # extra arguments are passed through
#
# To use it in place of the system launcher, install it as ~/.local/bin/stm32cubemx:
#   install -Dm755 stm32cubemx.sh ~/.local/bin/stm32cubemx

_JAVA_AWT_WM_NONREPARENTING=1
export _JAVA_AWT_WM_NONREPARENTING

APP=/usr/bin/stm32cubemx

if [ ! -x "$APP" ]; then
    echo "stm32cubemx: $APP not found (is the stm32cubemx package installed?)" >&2
    exit 127
fi

exec "$APP" "$@"
