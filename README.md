# SHCCL

**Smart Home Control Craft Launcher** — 一个用 Rust 编写的 Minecraft 启动器。

SHCCL 采用 Cargo workspace 组织，核心逻辑与交互前端分离。当前阶段以 **CLI 优先**，账号系统先支持 **离线模式**，Microsoft 登录、模组加载器与完整的 TUI/GUI 功能作为后续扩展点。

## 当前状态

| 组件 | 状态 | 说明 |
| --- | --- | --- |
| `shccl-core` | 可用 | 版本清单、版本 JSON、客户端/库/资源下载、natives 解压、离线账号、Java 检测、启动命令构建与进程启动 |
| `shccl-cli` | 可用 | 安装、离线登录、账号选择、启动、配置、诊断 |
| `shccl-tui` | 骨架 | 基于 ratatui/crossterm 的状态面板，可浏览已安装版本与当前账号 |
| `shccl-gui` | 骨架 | 基于 SDL3 的窗口与调试字体渲染，展示状态与已安装版本 |

> 离线账号仅用于离线游戏与离线服务器。Microsoft 登录尚未实现。

## Workspace 结构

```text
.
├── Cargo.toml
└── crates
    ├── shccl-core   # 下载 / 离线认证 / 启动逻辑，不依赖任何 UI
    ├── shccl-cli    # 二进制：shccl
    ├── shccl-tui    # 二进制：shccl-tui
    └── shccl-gui    # 二进制：shccl-gui（SDL3）
```

`shccl-core` 通过 `ProgressSink` 回调向任意前端报告阶段、文件进度和游戏日志；前端不需要依赖核心的异步运行时细节。

## 环境要求

- Rust 1.88+（edition 2024，使用 `if let` 链等 1.88 稳定特性）
- Java 21+（新版 Minecraft 所需；旧版本可用 Java 8/17，启动器会读取版本 JSON 的 `javaVersion.majorVersion` 做最低版本检查）
- SDL3（仅构建/运行 GUI 需要；也可以通过 Cargo feature 从源码构建）

默认目录：

- Linux: `$XDG_DATA_HOME/shccl` 或 `~/.local/share/shccl`
- macOS: `~/Library/Application Support/SHCCL`
- Windows: `%APPDATA%\SHCCL`

可通过 `--root <DIR>` 或环境变量 `SHCCL_ROOT` 指定现有 `.minecraft` 目录。

## 快速开始（CLI）

```bash
# 构建 CLI
cargo build --release -p shccl-cli

# 查看路径、Java、账号、已安装版本
cargo run -p shccl-cli -- doctor

# 创建并选中离线账号
cargo run -p shccl-cli -- login offline Steve

# 查看远端版本
cargo run -p shccl-cli -- versions --remote --limit 10

# 安装（客户端、libraries、natives、assets）
cargo run -p shccl-cli -- install 1.21.4

# 设置默认版本后直接启动
cargo run -p shccl-cli -- config set-default-version 1.21.4
cargo run -p shccl-cli -- launch

# 不实际启动，只打印最终命令
cargo run -p shccl-cli -- launch 1.21.4 --dry-run

# 传递游戏参数
cargo run -p shccl-cli -- launch 1.21.4 -- --fullscreen
```

### CLI 命令

```text
shccl doctor
shccl versions [--remote] [--limit N]
shccl install <VERSION>
shccl login offline <NAME>
shccl accounts
shccl select <USERNAME|UUID>
shccl launch [VERSION] [--account NAME] [--java PATH] [--memory MiB]
              [--game-dir DIR] [--width W --height H] [--dry-run]
              [-- GAME_ARGS...]
shccl config show
shccl config set-java <PATH>
shccl config clear-java
shccl config set-memory <MiB>
shccl config set-concurrency <1-64>
shccl config set-default-version <VERSION>
shccl config clear-default-version
shccl config set-game-dir <DIR>
shccl config clear-game-dir
```

`launch` 在目标版本未安装时会自动执行安装。

## 配置

配置保存在 `<root>/shccl.json`：

```json
{
  "java_path": null,
  "memory_mb": 4096,
  "jvm_args": [],
  "game_args": [],
  "default_version": "1.21.4",
  "game_dir": null,
  "download_concurrency": 12
}
```

`game_dir` 为空时使用 launcher root 作为游戏目录（与标准 `.minecraft` 一致），已有的 `saves`、`resourcepacks`、`options.txt` 可以直接复用。相对路径会相对于 root 解析。

离线账号保存在 `<root>/accounts.json`。在 Unix 系统上写入时会设置为 `0600` 权限。

离线 UUID 使用与 Java 相同的算法：

```text
UUID.nameUUIDFromBytes(("OfflinePlayer:" + name).getBytes(UTF_8))
```

因此与其它启动器、离线服务器和现有世界保持一致。

## 核心流程

```text
version_manifest_v2.json
        │
        ▼
版本 JSON（支持 inheritsFrom 合并）
        │
        ├── client.jar
        ├── libraries（按 rules / OS 过滤）
        │      └── natives
        └── asset index → assets/objects/<前两位>/<hash>
                              └── pre-1.6 / legacy virtual assets
        │
        ▼
构建 JVM/game 参数（占位符替换、内存、classpath、java.library.path）
        │
        ▼
启动 Java 进程
```

下载器会：

- 使用 SHA-1 + 文件大小校验已有文件；
- 并发下载（默认 12）；
- 写入 `.part` 后原子替换；
- 失败重试 3 次；
- 通过 `ProgressSink` 报告进度。

安装过程中会写入 `versions/<id>/.shccl-incomplete`；成功后才移除。中断的安装会被 CLI 识别并自动重新安装/续传，而不会误判为可启动版本。

native 处理同时兼容两种格式：

- 1.19 之前：从 `downloads.classifiers` 下载并解压到 `versions/<id>/natives`；
- 1.19 及之后：native jar 作为独立 library 加入 classpath，由 LWJGL 在运行时解压。

## 启动器版本注入

与 HMCL 一致，启动时会向游戏注入启动器信息：

- JVM 系统属性 `-Dminecraft.launcher.brand=SHCCL` 和 `-Dminecraft.launcher.version=<版本>`，供游戏、模组与崩溃报告读取；
- 版本 JSON 的 `${version_type}` 会被替换为 `SHCCL <版本>`，即游戏参数 `--versionType "SHCCL <版本>"`。在标题画面会拼接 `/<versionType>` 的版本（如 1.14.4、1.20）上，左下角会显示 `Minecraft <游戏版本>/SHCCL <版本>`；新版本即使不渲染该后缀，参数仍会照常传入；
- `${launcher_name}`、`${launcher_version}` 占位符也会被替换为对应值。

注意：`${version_type}` 被覆盖后，原来的 `release` / `snapshot` / 加载器类型不再出现在该参数中，这与 HMCL 的行为一致。

## TUI

```bash
cargo run -p shccl-tui
cargo run -p shccl-tui -- --root /path/to/.minecraft
```

当前提供只读状态面板：`j/k` 或方向键移动，`r` 刷新，`q`/`Esc` 退出。安装、登录和启动暂时通过 CLI 完成。

## GUI（SDL3）

```bash
cargo run -p shccl-gui
cargo run -p shccl-gui -- --root /path/to/.minecraft
```

SDL3 窗口使用内置 debug 字体渲染，`R` 刷新，`Q`/`Esc` 退出。

如果系统没有 SDL3，可启用从源码构建：

```toml
sdl3 = { version = "0.20", features = ["main", "build-from-source-static"] }
```

初次构建会更慢，并需要相应的 C 工具链。

## 安全与隐私

- 当前只保存离线账号，不涉及 Microsoft token。
- 所有启动参数中的访问令牌在离线模式下为占位值 `0`。
- 启动的游戏继承 CLI 的 stdin/stdout/stderr；TUI/GUI 后续可使用 `capture_output` 将日志转发到界面。

## 后续计划

- Microsoft 设备码登录与 token 刷新
- Fabric / Forge / NeoForge 等加载器安装
- Java runtime 自动下载与版本选择
- 完整 TUI：安装、账号、日志、启动
- 完整 SDL3 GUI：可点击版本列表、进度条、设置页
- 多实例隔离与实例级配置

## License

MIT
