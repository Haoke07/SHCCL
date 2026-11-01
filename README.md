# SHCCL

**Smart Home Control Craft Launcher** — 一个用 Rust 编写的 Minecraft 启动器。

SHCCL 采用 Cargo workspace 组织，核心逻辑与交互前端分离。当前阶段以 **CLI 优先**，账号系统支持 **离线模式** 与 **Microsoft 登录**（设备码流程 + token 自动刷新）。CLI、TUI 与 GUI 共享同一份配置和账号数据。

## 当前状态

| 组件 | 状态 | 说明 |
| --- | --- | --- |
| `shccl-core` | 可用 | 版本清单、版本 JSON、客户端/库/资源下载、natives 解压、离线账号、Microsoft 设备码登录与 token 刷新、Java 检测、启动命令构建与进程启动、版本级设置与版本隔离工作目录 |
| `shccl-cli` | 可用 | 安装、离线 / Microsoft 登录、账号选择、启动、全局与版本级配置、诊断 |
| `shccl-tui` | 可用 | 基于 ratatui/crossterm 的完整终端界面：仪表盘、版本安装/启动、离线 / Microsoft 账号、全局与版本级设置、日志与进度 |
| `shccl-gui` | 可用 | 基于 SDL3 的完整界面：仪表盘、版本安装/启动、离线 / Microsoft 账号、全局与版本级设置、日志与进度 |

> 离线账号仅用于离线游戏与离线服务器；Microsoft 账号可用于正版服务器，启动前会自动刷新过期 token。

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
- SDL3 与 SDL3_ttf（仅构建/运行 GUI 需要；也可以通过 Cargo feature 从源码构建）

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

# 使用 Microsoft 设备码登录（按提示在浏览器输入代码）
cargo run -p shccl-cli -- login microsoft

# 临时回退：旧版 Live Connect 公共 ID（无需注册应用，需手动粘贴回调 URL）
cargo run -p shccl-cli -- login microsoft --legacy

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
shccl login microsoft [--legacy]
shccl accounts
shccl select <USERNAME|UUID>
shccl launch [VERSION] [--account NAME] [--java PATH] [--memory MiB]
              [--game-dir DIR] [--width W --height H] [--dry-run]
              [-- GAME_ARGS...]
shccl config show [--version VERSION]
shccl config set-java <PATH> [--version VERSION]
shccl config clear-java [--version VERSION]
shccl config set-memory <MiB> [--version VERSION]
shccl config set-concurrency <1-64> [--version VERSION]
shccl config set-default-version <VERSION>
shccl config clear-default-version
shccl config set-game-dir <DIR> [--version VERSION]
shccl config clear-game-dir [--version VERSION]
shccl config reset --version VERSION
```

`config` 不带 `--version` 时修改启动器全局默认值；带 `--version` 时只修改该版本的覆盖项。`config show --version` 输出该版本实际生效的设置（包含继承自全局的字段和默认游戏目录）。

`launch` 在目标版本未安装时会自动执行安装。

## 配置

配置分为两层：

- 启动器全局默认值：`<root>/shccl.json`；
- 版本级覆盖：`<root>/versions/<version_id>/shccl.json`。

版本级文件只保存该版本显式设置的字段，未设置的字段继承全局默认值。默认情况下每个版本使用自己的 `versions/<version_id>` 作为工作/游戏目录（版本隔离）：`saves`、`mods`、`resourcepacks`、`options.txt` 等互不影响，而 `versions` 中的版本文件、`libraries`、`assets` 仍然共享。

如需让多个版本共用同一游戏目录，把它们的 `game_dir` 指向同一路径即可；其它设置（内存、Java、JVM/游戏参数等）仍然各自独立：

```bash
shccl config set-memory 8192 --version 1.21.4
shccl config set-game-dir /games/shared --version 1.21.4
shccl config set-game-dir /games/shared --version 1.20.1
shccl config show --version 1.21.4
shccl config reset --version 1.21.4
```

游戏目录解析顺序：

1. `launch --game-dir`；
2. 版本级配置的 `game_dir`；
3. 全局配置的 `game_dir`（可作为所有版本的共享目录）；
4. `versions/<version_id>`（默认的版本隔离目录）。

相对路径相对于 launcher root 解析。

全局默认值示例：

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

版本级覆盖示例（只保存与全局不同的字段）：

```json
{
  "memory_mb": 8192,
  "game_dir": "/games/shared"
}
```

`game_dir` 为空时不会再把 launcher root 当作所有版本的游戏目录，而是使用该版本自己的 `versions/<version_id>`。已有的 `saves`、`resourcepacks`、`options.txt` 如需继续复用，可以把全局或版本级 `game_dir` 指向原目录。

TUI 与 GUI 的 Settings 页面顶部有 `Target` 行，可在 Global defaults 和已安装版本之间切换；选中版本后编辑的设置写入 `versions/<id>/shccl.json`，其余版本不受影响。`Default version` 始终是全局设置。

账号保存在 `<root>/accounts.json`。在 Unix 系统上写入时会设置为 `0600` 权限。

离线 UUID 使用与 Java 相同的算法：

```text
UUID.nameUUIDFromBytes(("OfflinePlayer:" + name).getBytes(UTF_8))
```

因此与其它启动器、离线服务器和现有世界保持一致。

Microsoft 账号额外保存 Minecraft 访问令牌、OAuth 刷新令牌、过期时间与 XUID。访问令牌在启动前 5 分钟内过期时会自动刷新；刷新失败时会提示重新执行 `shccl login microsoft`。

Microsoft 登录需要启动器自己的 Azure 应用 client ID。它是公共客户端，不需要 secret。发布构建时应把 client ID 编进二进制，最终用户无需任何配置：

```bash
SHCCL_MICROSOFT_CLIENT_ID=<client-id> cargo build --release -p shccl-cli
```

TUI 与 GUI 的 Microsoft 登录也会读取同一个 client ID，发布时请一起注入：

```bash
SHCCL_MICROSOFT_CLIENT_ID=<client-id> cargo build --release -p shccl-tui -p shccl-gui
```

开发和测试时也可以在运行时覆盖：

```bash
SHCCL_MICROSOFT_CLIENT_ID=<client-id> cargo run -p shccl-cli -- login microsoft
```

注册 Azure 应用时注意：

- 账号类型选择支持个人 Microsoft 账号；
- 在“Authentication”中启用“Allow public client flows”；
- 设备码流程不需要重定向 URI，也不需要客户端密码；
- 新注册的应用还需在 <https://aka.ms/mce-reviewappid> 申请 Minecraft API 访问权限，否则 `api.minecraftservices.com` 会返回 403；
- scope 为 `XboxLive.signin offline_access`。

登录时使用的 client ID 会随账号一起保存，后续刷新 token 不依赖运行时环境。旧版共享 client ID `00000000402b5328` 不支持设备码流程（`AADSTS700016`），因此不再作为默认值。

### 临时使用旧版 Live Connect（`--legacy`）

在注册自己的 Azure 应用之前，可以用官方启动器的旧版公共 client ID 登录：

```bash
cargo run -p shccl-cli -- login microsoft --legacy
```

这条路径：

- 使用 `00000000402b5328` 和旧版 Live Connect 授权码流程；
- 打开浏览器登录后，需要把最终跳转的 `login.live.com/oauth20_desktop.srf?code=...` URL 粘贴回终端（Live Connect 拒绝 localhost 回调）；
- 账号会标记为 `legacy`，之后自动用 `login.live.com` 刷新 token；
- 用的是官方启动器的应用标识，不受官方支持，微软可能随时限制；仅建议在注册自己的 Azure 应用前临时使用。

默认的 `shccl login microsoft` 仍然要求构建时注入自己的 client ID，走设备码流程。

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

TUI 与 CLI/GUI 功能同步：全局设置和账号共享同一份 `shccl.json`、`accounts.json`，版本级设置写入各自的 `versions/<id>/shccl.json`。

| 页面 | 对应 CLI | 功能 |
| --- | --- | --- |
| Dashboard | `doctor` | Root、配置文件、Java 检测、账号、已安装版本、内存与默认版本 |
| Versions | `versions` / `install` / `launch` | 浏览已安装版本、加载远端清单、过滤、安装、修复/校验、启动、预览启动命令、设为默认 |
| Accounts | `login offline` / `login microsoft` / `accounts` / `select` | 添加并选中离线账号、Microsoft 设备码登录、切换当前账号 |
| Settings | `config ...` | 选择全局默认值或某个已安装版本，编辑 Java 路径、内存、下载并发、游戏目录，并查看配置 JSON；默认版本为全局设置 |
| Logs | - | 安装进度、启动命令、游戏 stdout/stderr |

操作方式：

- `1`–`5` 切换页面，`Tab` / `Shift+Tab` 循环切换；`↑`/`↓` 或 `j`/`k` 移动选择，`PgUp`/`PgDn` 翻页，`Home`/`End` 跳到首尾。
- `Enter` 执行当前页面的主要操作（启动/安装/选择账号/编辑设置）；Versions 页面中 `←`/`→` 切换 Installed/Remote，`i` 安装指定版本、`l` 加载远端清单、`f` 过滤、`d` 设为默认、`p` 预览启动命令、`v` 校验并修复。
- Accounts 页面 `a` 添加离线账号，`m` 使用 Microsoft 设备码登录（弹窗显示验证网址与代码，`Esc` 取消）；Settings 页面 `Enter` 编辑、`c` 清除可清除项、`t` 切换全局/版本目标、`s` 以 JSON 输出配置到 Logs；Logs 页面 `c` 清空日志。
- `r` 刷新本地状态并重新检测 Java；`?` / `F1` 打开快捷键帮助；`q`/`Esc` 退出（弹窗打开时 `Esc` 关闭弹窗）。
- 输入框支持 `Ctrl+A` 全选，`←`/`→`、`Home`/`End` 移动光标，`Enter` 保存，`Esc` 取消。
- 鼠标滚轮可滚动列表与日志，点击页签、版本页签和列表行可切换页面或选择条目。

启动游戏时使用 `capture_output`，游戏日志会实时显示在 Logs 页面；启动未安装的版本时会像 CLI 一样先自动安装。安装与启动期间状态栏显示阶段和文件进度。

## GUI（SDL3）

```bash
cargo run -p shccl-gui
cargo run -p shccl-gui -- --root /path/to/.minecraft
```

GUI 与 CLI 功能同步：全局设置和账号共享同一份 `shccl.json`、`accounts.json`，版本级设置写入各自的 `versions/<id>/shccl.json`。

| 页面 | 对应 CLI | 功能 |
| --- | --- | --- |
| Dashboard | `doctor` | Root、配置文件、Java 检测、账号、已安装版本、内存与默认版本 |
| Versions | `versions` / `install` / `launch` | 浏览已安装版本、加载远端清单、过滤、安装、修复/校验、启动、预览启动命令、设为默认 |
| Accounts | `login offline` / `login microsoft` / `accounts` / `select` | 添加并选中离线账号、Microsoft 设备码登录、切换当前账号 |
| Settings | `config ...` | 选择全局默认值或某个已安装版本，编辑 Java 路径、内存、下载并发、游戏目录，并查看配置 JSON；默认版本为全局设置 |
| Logs | - | 安装进度、启动命令、游戏 stdout/stderr |

操作方式：

- 鼠标点击侧边栏、列表行和按钮；滚轮滚动列表与日志。
- `1`–`5` 切换页面，`Tab` 循环切换；`↑`/`↓` 或 `j`/`k` 移动选择。
- `Enter` 执行当前操作（安装/启动/选择账号）；Versions 页面中 `I` 安装指定版本、`L` 加载远端清单、`F` 过滤、`D` 设为默认、`P` 预览启动命令。
- Accounts 页面 `A` 添加离线账号，`M` 或点击 `Microsoft sign-in` 使用设备码登录（弹窗显示验证网址与代码，`Esc` 或 Cancel 取消）；Settings 页面点击 `Target` 行的 `Change`（或按 `T`）切换全局/版本目标；Logs 页面 `C` 清空日志。
- `R` 刷新本地状态并重新检测 Java；`Q`/`Esc` 退出（弹窗打开时 `Esc` 关闭弹窗）。
- 输入框支持 `Ctrl+A` 全选，`←`/`→`、`Home`/`End` 移动光标，`Enter` 保存。

启动游戏时使用 `capture_output`，游戏日志会实时显示在 Logs 页面；启动未安装的版本时会像 CLI 一样先自动安装。

界面文字使用系统已安装的等宽字体渲染：启动时优先通过 fontconfig（`fc-match monospace`）获取当前桌面偏好的等宽字体，否则在系统字体目录中挑选等宽字体文件，并按 12px 行高缩放以适配布局。字体按设备像素光栅化后 1:1 绘制（不使用 SDL 的 renderer scale），因此在高分屏或 2 倍缩放下不会因放大而模糊。若系统中找不到任何等宽字体，GUI 会在启动时报错退出。

如果系统没有 SDL3，可启用从源码构建：

```toml
sdl3 = { version = "0.20", features = ["main", "ttf", "build-from-source-static"] }
```

初次构建会更慢，并需要相应的 C 工具链（SDL3 与 SDL3_ttf 都会从源码构建）。

## 安全与隐私

- 离线账号只保存用户名与 UUID；Microsoft 账号会保存访问令牌与刷新令牌，`accounts.json` 在 Unix 上权限为 `0600`。
- Microsoft 登录默认使用设备码流程，不需要在本机监听回调端口，也不会保存账号密码；`--legacy` 回退使用旧版 Live Connect，需要手动粘贴回调 URL。
- 离线模式下的访问令牌为占位值 `0`；Microsoft 模式使用刷新后的真实令牌。
- 启动的游戏在 CLI 中继承终端的 stdin/stdout/stderr；TUI/GUI 使用 `capture_output` 将日志转发到界面，不读写游戏 stdin。

## 后续计划

- Fabric / Forge / NeoForge 等加载器安装
- Java runtime 自动下载与版本选择

## License

MIT
