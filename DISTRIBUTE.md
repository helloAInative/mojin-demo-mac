# 摸金小王子 · 分发

当前 `build.sh` 使用 **ad-hoc 签名**，本机可运行。对外分发需要 Apple 开发者账号。

## 公证（Notarize）

```bash
# 1) Developer ID Application 证书签名
codesign --force --options runtime --sign "Developer ID Application: YOUR NAME (TEAMID)" \
  "/Applications/摸金小王子.app"

# 2) 打包
ditto -c -k --keepParent "/Applications/摸金小王子.app" /tmp/mojin.zip

# 3) 公证
xcrun notarytool submit /tmp/mojin.zip --apple-id YOU@email --team-id TEAMID --password APP_SPECIFIC --wait
xcrun stapler staple "/Applications/摸金小王子.app"
```

## Sparkle 自动更新

1. 集成 [Sparkle](https://sparkle-project.org)
2. 托管 `appcast.xml`（含签名包 URL 与版本号）
3. 本仓库未内嵌 Sparkle，避免无证书时空包体积

## TestFlight

菜单栏 App 一般走 **Developer ID 直发**，不走 App Store / TestFlight。若要上架需改 `LSUIElement` 并补沙盒。
