# macOS 输入权限与开发包签名

权限界面读取运行中 daemon 的 CoreGraphics 预检结果。系统设置里的开关即使显示为开启，也可能属于旧应用签名；这种情况下当前进程仍无法捕获或注入输入。界面应显示“当前版本未生效”，不能把开关状态或旧授权当作实际可用状态。

每次重新构建后用同一个 Apple Development 或 Developer ID 身份签名应用包，包括同包 daemon。不要用 ad-hoc 签名做需要持久保留输入权限的开发包。推荐从仓库根目录运行统一打包命令，退出旧应用和 daemon 后安装到固定路径：

```sh
APPLE_SIGNING_IDENTITY='Apple Development: YOUR NAME (TEAMID)' \
  bash scripts/package-macos-desktop.sh --install
open R-ShareMouse.app
```

该命令会构建前端、daemon、CLI 和 Tauri 应用，以相同证书签名 app 与同包二进制，然后将上一版应用移到废纸篓。未签名或 ad-hoc 的开发包不能作为日常更新包。另一台 Mac 可以使用自己的证书，但它之后的更新必须保持同一证书身份、bundle ID 和安装路径。若要两台机器共用同一发布包，应使用共同的发布证书。

首次切换到固定签名时，需要在“系统设置 → 隐私与安全性 → 输入监控”和“辅助功能”中给**当前** `R-ShareMouse.app` 授权，并完全退出应用和 daemon 后重新打开。如果已有同名旧记录且开关开启却仍未生效，请核对应用路径和签名，移除旧记录并重新添加当前应用。最后在应用里点“重新检测”；两项必须都显示“当前版本可用”，输入后端也必须就绪。

只读检查示例：

```sh
codesign -dv --verbose=4 /path/to/R-ShareMouse.app
codesign -dv --verbose=4 /path/to/R-ShareMouse.app/Contents/MacOS/rshare-daemon
codesign --verify --deep --strict /path/to/R-ShareMouse.app
```

签名证书身份、应用 bundle ID、安装路径或二进制位置变化后，都应重新核验实际权限。CLI 或终端进程的授权状态不代表 GUI 启动的 daemon 已获授权。
