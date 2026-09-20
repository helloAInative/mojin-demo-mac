# 贡献指南

感谢你参与摸金小王子。

## 开始之前

1. 先搜索现有 Issue，确认问题尚未被讨论。
2. 较大的功能请先创建 Issue，说明使用场景、交互和数据来源。
3. 行情源改动请确认服务条款、稳定性和数据字段含义。

## 本地开发

环境要求和启动方式见 [README.md](README.md)。建议先运行：

```bash
cd mojinprince-server
cargo fmt --check
cargo test --offline
./scripts/verify.sh
cd ..
BUILD_ONLY=1 ./build.sh
```

## 代码约定

- Swift 使用现有 SwiftUI 风格，避免在视图中新增第三方依赖。
- Rust 提交前运行 `cargo fmt` 和 `cargo clippy --all-targets -- -D warnings`。
- 行情网关返回结构化错误，HTTP 状态码与错误类型必须一致。
- 新配置需要有安全默认值，并在 README 中说明。
- 不提交 API Token、证书、数据库、日志、构建产物或个人持仓数据。

## Pull Request

PR 描述应包含改动原因、用户可见行为、关键实现选择、运行过的测试和已知限制。一个 PR 尽量只解决一个主题；修改功能状态时同步更新路线图文档。

## 提交信息

推荐使用简短的祈使句，例如：

```text
Add gateway connection diagnostics
Fix Beijing exchange code normalization
```
