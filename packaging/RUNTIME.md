# AppImage runtime

`runtime-x86_64` 是 AppImage type-2 的启动器（runtime），用于把 `AppRun` +
SquashFS 镜像拼成单文件 `.AppImage`。

- 来源：AppImageKit / appimagetool 的标准 runtime（构建号 `effcebc`）
- 架构：x86_64
- 许可证：MIT（见 <https://github.com/AppImage/AppImageKit>）
- 文件大小：约 184 KB

`packaging/appimage.sh` 会把它直接拼到 SquashFS 前面。如果以后需要更新 runtime，
可以用 `APPIMAGE_RUNTIME=/path/to/new/runtime ./packaging/appimage.sh` 覆盖，
或直接替换本文件。
