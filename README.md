# ssh-client

原生 Rust SSH 终端客户端。一个安装目录，两个命令：`ssh-client` 和 `ssh`。
支持 Windows、Linux、macOS，x86_64 / ARM64；不依赖系统 `ssh` 程序。

> 新项目，尚未经过独立安全审计。不是 OpenSSH 全功能替代品。
> 请查看 [验证记录](docs/VALIDATION.md)，区分已跑过的测试与仅配置的 CI。

## 一行安装

以下地址需要仓库及首个 Release 已公开发布后才可用。

**Linux / macOS**（用户级，无需 sudo）：

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/neko233-com/ssh-client/main/scripts/install.sh | sh
```

**Windows PowerShell**（用户级，无需管理员）：

```powershell
& ([scriptblock]::Create((Invoke-WebRequest -UseBasicParsing https://raw.githubusercontent.com/neko233-com/ssh-client/main/scripts/install.ps1).Content))
```

网络脚本会执行代码；安全敏感环境请先下载审查脚本，再执行。正式部署建议固定
Release 版本，不要一直跟随 `main`。安装器会校验发行包的 SHA-256，但校验值与包
来自同一个 GitHub Release，不等于独立数字签名。

安装后重新打开终端，让持久化 PATH 生效。安装器使用独立的用户目录，不覆盖系统
OpenSSH 文件。POSIX PATH 优先级会改变 `ssh` 解析到的程序；Windows 新终端中系统 PATH 可能
仍优先于用户 PATH，此时用 `ssh-client` 或完整路径调用本项目。需要完整 OpenSSH 功能时应
显式使用系统程序路径，或卸载本项目后继续使用系统客户端。

```sh
ssh-client --version
ssh --version
ssh-client --help
```

## 连接示例

选项放在目标主机前；目标后面的内容作为远程 shell 命令：

```sh
# 首次连接：默认拒绝未知服务器，并显示指纹
ssh-client -i ~/.ssh/id_ed25519 user@example.com

# 通过可信渠道核对指纹后，显式登记新主机；绝不会自动接受已变更的密钥
ssh-client --accept-new -i ~/.ssh/id_ed25519 user@example.com

# 自定义端口、远程命令和批处理（不提示密码）
ssh-client -B -n -p 2222 -i ~/.ssh/id_ed25519 user@example.com 'uname -a'

# 交互式密码输入，不把密码放进命令行或进程列表
ssh-client --password user@example.com

# IPv6 / 强制或禁用 PTY
ssh-client -p 2222 'user@[::1]'
ssh-client -t user@example.com 'top'
ssh-client -T user@example.com 'printf hello'
```

`ssh` 与 `ssh-client` 是同一套实现。连接错误返回 255；远程命令的 0–255
退出码原样返回。远程 shell 负责解析命令字符串，引用规则类似 OpenSSH，不能把
不可信输入直接拼接成远程命令。

## 安全与兼容范围

- 原生 SSH 加密传输，私钥认证与交互密码认证。
- 交互 shell、PTY、终端 resize、原始终端模式恢复。
- 远程命令、stdin/stdout/stderr、EOF、退出码。
- 默认使用 `~/.ssh-client/known_hosts`。这是专用精确匹配数据库，不会修改
  `~/.ssh/known_hosts`。不支持散列、通配符、CA/certificate/marker 格式；拒绝
  不理解的记录，不会静默跳过验证。
- 默认私钥为 `~/.ssh/id_ed25519`，可用 `-i` 指定。
- 未实现 OpenSSH 配置文件、ssh-agent、ProxyJump、端口转发、SCP/SFTP、
  PKCS#11、FIDO、安全密钥、keyboard-interactive/MFA。不要假定对应选项可用。
- 安装只处理客户端，不安装/启用 SSH 服务端，不修改防火墙。

详见 [安全说明](SECURITY.md) 与 [测试规范](docs/TESTING.md)。

## 从源码构建

```sh
cargo build --locked --release --bins
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
```

构建需要 Rust 工具链及目标平台 C 编译工具（加密依赖使用 ring）。Windows 使用
MSVC Build Tools；Linux 使用 GCC/Clang；macOS 使用 Xcode Command Line Tools。

## 发布

推送与 Cargo.toml 版本相同的 `vX.Y.Z` 标签。六个平台的原生测试、安装器验证、
Release 构建、双命令 smoke test 全部成功后，工作流才发布 GitHub Release。
发行物是六个压缩包和 `SHA256SUMS`。任何平台缺失都会阻止发布。

当前 CI 不进行代码签名、公证或外部渗透测试。Linux GNU 包的最低 glibc 版本
取决于构建环境（Ubuntu 24.04），不承诺兼容所有旧发行版或 Alpine/musl。

License: MIT.

## 固定版本、安装目录与卸载

下载脚本并审查后，可使用以下参数：

```sh
sh scripts/install.sh --version v0.1.0
sh scripts/install.sh --prefix "$HOME/tools/ssh-client" --no-path
sh scripts/install.sh --uninstall
```

```powershell
& ./scripts/install.ps1 -Version v0.1.0
& ./scripts/install.ps1 -Prefix "$HOME\tools\ssh-client" -NoPath
& ./scripts/install.ps1 -Uninstall
```

卸载使用安装时相同的 `--prefix` / `-Prefix`。安装器只删除收据中校验和匹配的
自有文件，发现被替换/修改的文件会停止而不是强行删除。`--no-path` / `-NoPath`
不修改持久化 PATH；调用完整路径或自行设置当前会话 PATH。自定义 prefix 不接受
符号链接路径或 shell 特殊字符，以避免不安全的 profile 写入。

POSIX 默认目录：`~/.local/share/ssh-client/bin`，更新 `.profile`、`.bashrc`、
`.zshrc` 中带标记的块。符号链接 profile 会跳过；自行管理的其他 shell 配置需手动
添加 PATH。Windows 默认目录：`%LOCALAPPDATA%\ssh-client\bin`，更新用户级 PATH
和当前 PowerShell 会话。不会更改全局执行策略。
