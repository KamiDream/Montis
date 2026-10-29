#!/usr/bin/env sh
# 生成单文件 AppImage：dist/Montis-<版本>-<架构>.AppImage
#
#   ./packaging/appimage.sh
#
# 依赖：
#   * mksquashfs（Arch: sudo pacman -S squashfs-tools；Debian/Ubuntu: squashfs-tools）
#   * packaging/runtime-x86_64（随仓库提供的 AppImage type-2 runtime）
#
# 可用环境变量覆盖：
#   MKSQUASHFS=/path/to/mksquashfs
#   APPIMAGE_RUNTIME=/path/to/runtime
set -eu

cd "$(dirname "$0")/.."

VERSION="$(grep -m1 '^version = ' Cargo.toml | cut -d'"' -f2)"
ARCH="$(uname -m)"
NAME="Montis-${VERSION}-${ARCH}.AppImage"

RUNTIME="${APPIMAGE_RUNTIME:-packaging/runtime-x86_64}"
MKSQUASHFS="${MKSQUASHFS:-$(command -v mksquashfs || true)}"
APPDIR="dist/Montis.AppDir"
IMAGE="dist/Montis-${VERSION}-${ARCH}.squashfs"

if [ -z "${MKSQUASHFS}" ] || [ ! -x "${MKSQUASHFS}" ]; then
    echo "缺少 mksquashfs。Arch: sudo pacman -S squashfs-tools" >&2
    echo "Debian/Ubuntu: sudo apt install squashfs-tools" >&2
    exit 1
fi

if [ ! -f "${RUNTIME}" ]; then
    echo "缺少 AppImage runtime：${RUNTIME}" >&2
    echo "可以用 APPIMAGE_RUNTIME=/path/to/runtime 指定。" >&2
    exit 1
fi

if [ "${ARCH}" != "x86_64" ]; then
    echo "当前只内置了 x86_64 runtime，无法为 ${ARCH} 生成 AppImage。" >&2
    exit 1
fi

cargo build --release -p nm-app --locked

rm -rf "${APPDIR}"
mkdir -p \
    "${APPDIR}/usr/bin" \
    "${APPDIR}/usr/share/applications" \
    "${APPDIR}/usr/share/icons/hicolor/scalable/apps" \
    "${APPDIR}/usr/share/icons/hicolor/256x256/apps" \
    "${APPDIR}/usr/share/doc/melody"

install -m755 target/release/melody "${APPDIR}/usr/bin/melody"
install -m644 packaging/melody.desktop "${APPDIR}/melody.desktop"
install -m644 packaging/melody.desktop \
    "${APPDIR}/usr/share/applications/melody.desktop"
install -m644 crates/nm-app/assets/logo.svg \
    "${APPDIR}/usr/share/icons/hicolor/scalable/apps/melody.svg"
install -m644 packaging/melody.png \
    "${APPDIR}/usr/share/icons/hicolor/256x256/apps/melody.png"
# AppImage 约定：根目录下放与 Icon= 同名的图标，并用 .DirIcon 作为缩略图。
install -m644 packaging/melody.png "${APPDIR}/melody.png"
install -m644 packaging/melody.png "${APPDIR}/.DirIcon"
install -m644 README.md LICENSE "${APPDIR}/usr/share/doc/melody/"

cat > "${APPDIR}/AppRun" <<'EOF'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
exec "${HERE}/usr/bin/melody" "$@"
EOF
chmod 755 "${APPDIR}/AppRun"

rm -f "${IMAGE}" "dist/${NAME}"
"${MKSQUASHFS}" "${APPDIR}" "${IMAGE}" \
    -root-owned -noappend -comp gzip -b 131072 -no-progress >/dev/null

cat "${RUNTIME}" "${IMAGE}" > "dist/${NAME}"
chmod 755 "dist/${NAME}"
rm -f "${IMAGE}"
rm -rf "${APPDIR}"

sha256sum "dist/${NAME}" > "dist/${NAME}.sha256"

echo "已生成：dist/${NAME}"
echo "校验和：dist/${NAME}.sha256"
echo "试运行：./dist/${NAME}"
