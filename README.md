# gpt-switch

`gpt-switch` 是一个用于在本机保存和切换多个 Codex ChatGPT 账号的命令行工具。

它不会调用远程登出接口，而是保存并原子替换本地的 `~/.codex/auth.json`。切换账号前，工具会先同步当前账号的最新凭据，尽量避免丢失刷新后的 token。

## 功能

- 给当前 Codex 账号设置别名并保存
- 查看已保存账号及当前账号
- 在已保存账号之间切换
- 删除不再需要的账号
- 移除当前本地凭据，为登录新账号做准备
- 写入前校验认证文件，并通过临时文件原子替换
- 在 Unix 系统上将保存的凭据权限设置为 `0600`

## 安装

### 直接安装二进制

普通用户不需要下载源码或安装 Rust。请从项目的 Releases 页面下载与操作系统和 CPU 架构匹配的文件：

| 平台 | 目标名称 |
| --- | --- |
| macOS Apple Silicon | `aarch64-apple-darwin` |
| macOS Intel | `x86_64-apple-darwin` |
| Linux x64 | `x86_64-unknown-linux-gnu` |

解压后安装到 `PATH`：

```bash
chmod +x gpt-switch
sudo mv gpt-switch /usr/local/bin/gpt-switch
```

验证安装：

```bash
gpt-switch --version
gpt-switch --help
```

发布者可以使用以下命令生成当前平台的优化版本：

```bash
cargo build --release
```

生成的文件位于 `target/release/gpt-switch`。发布时只需要分发这个可执行文件，无需分发整个 `target` 目录。不同操作系统或 CPU 架构需要分别构建，建议在 Releases 中标注目标名称并提供 SHA-256 校验值。

### 从源码安装

从源码安装需要 Rust 工具链：

```bash
cargo install --path .
```

也可以直接在项目目录运行：

```bash
cargo run -- --help
```

## 使用方法

查看当前账号：

```bash
gpt-switch current
```

保存当前账号：

```bash
gpt-switch add work
```

账号别名只能包含字母、数字、`-` 和 `_`。

查看已保存账号：

```bash
gpt-switch list
```

切换账号：

> 执行切换前，请先完全退出 Codex App 和正在运行的 Codex CLI，避免运行中的进程重新写回旧凭据。

```bash
gpt-switch use personal
```

切换完成后重新打开 Codex。

删除账号：

```bash
gpt-switch remove work
```

当前正在使用的账号不能直接删除，请先切换到其他账号。

## 添加新账号

第一次使用时，先保存当前账号：

```bash
gpt-switch add work
```

然后按以下流程操作：

1. 完全退出 Codex App 和正在运行的 Codex CLI。
2. 执行 `gpt-switch login-new`。
3. 重新打开 Codex，登录新的 ChatGPT 账号。
4. 执行 `gpt-switch current`，确认当前邮箱。
5. 执行 `gpt-switch add personal`，保存新账号。

之后可以通过别名切换：

```bash
gpt-switch use work
gpt-switch use personal
```

## 数据目录

默认使用以下文件：

```text
~/.codex/auth.json
~/.codex/gpt-switch/accounts/<alias>/auth.json
~/.codex/gpt-switch/login-backup.json
```

其中：

- `auth.json` 是 Codex 当前使用的活动凭据。
- `accounts/<alias>/auth.json` 是各账号的凭据快照。
- `login-backup.json` 是执行 `login-new` 前额外保存的最近一次备份。

## 安全说明

- 保存的 `auth.json` 包含完整登录凭据，请勿分享、上传或加入版本管理。
- 凭据以本地文件形式保存，没有额外加密；安全性依赖操作系统账号和文件权限。
- 工具只解析 JWT 中的邮箱用于本地账号识别，不校验 JWT 签名，也不会将其用于服务端认证或权限判断。
- `login-new` 只移除本地活动文件，不会调用 `codex logout`，因此不会主动使远程 token 失效。

## 当前限制

- 目前固定使用 `~/.codex`，尚未支持自定义 `CODEX_HOME`。
- 需要在 Codex 完全退出时进行切换，当前版本不会自动检测运行中的 Codex 进程。
- 主要支持包含 `id_token` 和邮箱信息的 ChatGPT 登录凭据，不适用于纯 API Key 账号切换。
- 当前没有进程锁，不建议同时运行多个 `gpt-switch` 命令。

## 开发与验证

```bash
cargo fmt --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

单元测试使用系统临时目录，不会读取或修改真实的 Codex 登录文件。
