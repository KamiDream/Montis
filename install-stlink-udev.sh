#!/bin/sh
# Install ST-LINK udev rules so a normal user can open the probe.
#
# Arch Linux uses the "uucp" group instead of "plugdev"; systemd's uaccess ACL
# is kept as well so it also works without group membership.
# Rule set taken from ST's stlink-udev-rules 1.0.3-3 bundle.
#
# Usage:
#   sudo sh install-stlink-udev.sh
#
# Then replug the ST-LINK (or run: sudo udevadm trigger --action=change \
# --subsystem-match=usb) and start the debug session again.

set -eu

if [ "$(id -u)" -ne 0 ]; then
    echo "需要 root 权限，请用: sudo sh $0" >&2
    exit 1
fi

RULES=/etc/udev/rules.d/99-stlink.rules

cat > "$RULES" <<'EOF'
# ST-LINK/V1, V2, V2-1 and V3 debug probes.
# Group "uucp" for Arch Linux; uaccess ACL for the active local session.
SUBSYSTEMS=="usb", ATTRS{idVendor}=="0483", ATTRS{idProduct}=="3744", \
    MODE="660", GROUP="uucp", TAG+="uaccess", ENV{ID_MM_DEVICE_IGNORE}="1", \
    SYMLINK+="stlinkv1_%n"

SUBSYSTEMS=="usb", ATTRS{idVendor}=="0483", ATTRS{idProduct}=="3748", \
    MODE="660", GROUP="uucp", TAG+="uaccess", ENV{ID_MM_DEVICE_IGNORE}="1", \
    SYMLINK+="stlinkv2_%n"

SUBSYSTEMS=="usb", ATTRS{idVendor}=="0483", ATTRS{idProduct}=="374b", \
    MODE="660", GROUP="uucp", TAG+="uaccess", ENV{ID_MM_DEVICE_IGNORE}="1", \
    SYMLINK+="stlinkv2-1_%n"

SUBSYSTEMS=="usb", ATTRS{idVendor}=="0483", ATTRS{idProduct}=="3752", \
    MODE="660", GROUP="uucp", TAG+="uaccess", ENV{ID_MM_DEVICE_IGNORE}="1", \
    SYMLINK+="stlinkv2-1_%n"

SUBSYSTEMS=="usb", ATTRS{idVendor}=="0483", ATTRS{idProduct}=="374d", \
    MODE="660", GROUP="uucp", TAG+="uaccess", ENV{ID_MM_DEVICE_IGNORE}="1", \
    SYMLINK+="stlinkv3loader_%n"

SUBSYSTEMS=="usb", ATTRS{idVendor}=="0483", ATTRS{idProduct}=="374e", \
    MODE="660", GROUP="uucp", TAG+="uaccess", ENV{ID_MM_DEVICE_IGNORE}="1", \
    SYMLINK+="stlinkv3_%n"

SUBSYSTEMS=="usb", ATTRS{idVendor}=="0483", ATTRS{idProduct}=="374f", \
    MODE="660", GROUP="uucp", TAG+="uaccess", ENV{ID_MM_DEVICE_IGNORE}="1", \
    SYMLINK+="stlinkv3_%n"

SUBSYSTEMS=="usb", ATTRS{idVendor}=="0483", ATTRS{idProduct}=="3753", \
    MODE="660", GROUP="uucp", TAG+="uaccess", ENV{ID_MM_DEVICE_IGNORE}="1", \
    SYMLINK+="stlinkv3_%n"

SUBSYSTEMS=="usb", ATTRS{idVendor}=="0483", ATTRS{idProduct}=="3754", \
    MODE="660", GROUP="uucp", TAG+="uaccess", ENV{ID_MM_DEVICE_IGNORE}="1", \
    SYMLINK+="stlinkv3_%n"

SUBSYSTEMS=="usb", ATTRS{idVendor}=="0483", ATTRS{idProduct}=="3755", \
    MODE="660", GROUP="uucp", TAG+="uaccess", ENV{ID_MM_DEVICE_IGNORE}="1", \
    SYMLINK+="stlinkv3loader_%n"

SUBSYSTEMS=="usb", ATTRS{idVendor}=="0483", ATTRS{idProduct}=="3757", \
    MODE="660", GROUP="uucp", TAG+="uaccess", ENV{ID_MM_DEVICE_IGNORE}="1", \
    SYMLINK+="stlinkv3_%n"
EOF

udevadm control --reload-rules
udevadm trigger --action=change --subsystem-match=usb

echo "已写入 $RULES 并重新加载 udev 规则。"
echo "请重新插拔 ST-LINK，然后在 VS Code 中重新开始调试。"
