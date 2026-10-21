<div align="center">

# wx-cli

**从命令行查询本地微信数据**

[![License: Apache-2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey.svg)](#安装)
[![Rust](https://img.shields.io/badge/built%20with-Rust-orange.svg)](https://www.rust-lang.org)

会话 · 聊天记录 · 搜索 · 联系人 · 群成员 · 群昵称 · 收藏 · 统计 · 导出

</div>

---

## AI Agent Skill

通过 [skills CLI](https://github.com/vercel-labs/skills) 一键安装到 Claude Code、Cursor、Codex 等 agent：

```bash
npx skills add botiverse/wx-cli
```

或全局安装：

```bash
npx skills add botiverse/wx-cli -g
```

安装后 agent 会自动读取 `SKILL.md`，了解如何安装和调用 wx-cli。

源码与发布仓库：[botiverse/wx-cli](https://github.com/botiverse/wx-cli)。

---

## 特性

- **零依赖安装** — 单一 Rust 二进制，一行命令装完
- **毫秒级响应** — 后台 daemon 持久缓存解密数据库，mtime 不变则复用
- **AI 友好** — `history` / `search` / `sessions` / `new-messages` / `stats` / `attachments` 默认返回 `{..., meta}` wrapper，agent 能直接消费 freshness / source 信息
- **完全本地** — 数据不出本机，实时解密，无需全量预解密

---

## 安装

> **当前仓库 [botiverse/wx-cli](https://github.com/botiverse/wx-cli) 为 private。**  
> 匿名 `curl` / 公开 npm 旧包（`@jackwener/wx-cli@0.3.0`）**拿不到**本仓库最新二进制。  
> 有仓库读权限时，请用下面的 **源码构建**（推荐）。

### 从源码构建（推荐）

```bash
git clone git@github.com:botiverse/wx-cli.git && cd wx-cli
cargo build --release
# 安装到用户 PATH（覆盖旧版）
mkdir -p ~/.local/bin
cp target/release/wx ~/.local/bin/wx
wx --version   # 应显示当前 Cargo.toml 版本，如 0.6.3
```

Windows：

```powershell
git clone git@github.com:botiverse/wx-cli.git
cd wx-cli
cargo build --release
# 将 target\release\wx.exe 放到 PATH 目录
```

### 已有 clone 时升级

```bash
git pull
cargo build --release
cp target/release/wx ~/.local/bin/wx
```

<details>
<summary>其他方式（需仓库权限 / 发布配置）</summary>

**GitHub Release 预编译包**（仓库 private 时仅协作者可见）

从 [Releases](https://github.com/botiverse/wx-cli/releases) 下载：

| 平台 | 文件 |
|------|------|
| macOS Apple Silicon | `wx-macos-arm64` |
| macOS Intel | `wx-macos-x86_64` |
| Linux x86_64 | `wx-linux-x86_64` |
| Linux arm64 | `wx-linux-arm64` |
| Windows x86_64 | `wx-windows-x86_64.exe` |

```bash
chmod +x wx-macos-arm64 && mv wx-macos-arm64 ~/.local/bin/wx
```

**一键脚本**（raw 链接在 private 仓库下对匿名用户 404；有权限时可用 `gh` 下载 release asset）

```bash
# 需已登录 gh 且对 botiverse/wx-cli 有读权限
gh release download -R botiverse/wx-cli -p 'wx-macos-arm64' -O ~/.local/bin/wx
chmod +x ~/.local/bin/wx
```

`install.sh` / `install.ps1` 仍维护在仓库内，仓库公开或 raw 可访问后可再启用：

```bash
curl -fsSL https://raw.githubusercontent.com/botiverse/wx-cli/main/install.sh | bash
```

**npm**

历史包名 `@jackwener/wx-cli` 仍存在于 npm，但公开 registry 上的版本可能严重滞后，**不要**当作当前主安装路径。

</details>

---

## 快速开始

### 新用户要做什么 / 不要做什么

| | macOS 新用户 |
|--|--|
| **需要** | 微信 4.x 已安装并**登录**；从**本机 GUI Terminal**（Terminal.app / iTerm 等，不要用 SSH）执行 `sudo wx init` |
| **不需要** | **关闭 SIP** |
| **不需要** | 预先 `codesign` / ad-hoc **重签微信**（默认路径不会改 WeChat.app） |
| **init 之后** | 日常 `wx sessions` / `history` 等**无需 sudo**，也**不要求微信一直开着** |

### 初始化（只需一次）

保持微信运行并已登录，然后：

**macOS**

```bash
# 必须：本机 GUI Terminal + sudo（系统可能提示授予「开发者工具」权限，请允许）
sudo wx init

# 若提示缺分片密钥 / meta.unknown_shards 非空：加长 hook，等待期间在微信里点开相关聊天
sudo wx key extract --hook-seconds 90
```

`wx init` 两阶段取密钥（**都不依赖关 SIP**）：

1. 进程内存扫描（`x'key+salt'` + salt 邻接）
2. LLDB hook `CCCryptorCreate`，补齐尚未加载进内存的 per-DB AES key

说明：

- 官方 **Hardened Runtime** 包：本机 Terminal + `sudo` 即可；若失败，到「系统设置 → 隐私与安全性 → 开发者工具」里勾选你的终端。
- 部分官网 4.x 本身已是 **ad-hoc**：用户态 LLDB 往往也能工作。
- **不要**默认对 WeChat 做 ad-hoc 重签：会打乱 TCC 权限、公众号/截图可能异常。仅当 SSH 等无 GUI 场景下 `task_for_pid` 仍失败时，才考虑有副作用的重签；详见 [macOS 权限指南](docs/macos-permission-guide.md)。

**Linux**

```bash
sudo wx init
```

**Windows**（以管理员身份运行 PowerShell）

```powershell
wx init
```

### 验证

```bash
wx --version
wx doctor          # 密钥 / 分片 / SQLCipher 健康检查
wx sessions
```

能看到最近会话且 `wx doctor` 关键项通过即表示正常。daemon 在首次查询时自动启动。

若 `doctor` 提示关键分片缺密钥，或 `meta.unknown_shards` 非空：

```bash
sudo wx key extract --hook-seconds 90
# 等待期间在微信里打开相关聊天（冷分片可能需触发加载）
wx doctor
```

---

## 命令

### 诊断与密钥

```bash
wx doctor                                 # 环境 / 密钥 / 分片检查
wx doctor --fix --json                   # JSON + 修复建议
wx key list                               # 已有密钥与缺失覆盖
wx key extract --hook-seconds 90          # 建议：sudo wx key extract --hook-seconds 90
wx key set message/message_1.db <64hex>   # 手动写入并校验
```

补密钥统一用 **`sudo wx key extract --hook-seconds 90`**（不要用裸的 `wx init --force` 当主路径）。  
取钥**不需要关闭 SIP**；需本机 GUI Terminal +（Hardened Runtime 包）sudo。

### 消息

```bash
wx sessions                                      # 最近 20 个会话
wx unread                                        # 有未读消息的会话
wx unread --filter private,group                 # 只看真人未读（过滤公众号/折叠入口）
wx new-messages                                  # 上次检查后的新消息（增量）
wx history "张三"                                # 最近 50 条记录
wx history "张三" -n 2000                        # 拉更多历史消息
wx history "AI群" --since 2026-04-01 --until 2026-04-15
wx search "关键词"                               # 全库搜索
wx search "关键词" -n 500                        # 放宽搜索结果条数
wx search "会议" --in "工作群" --since 2026-01-01
```

`history` / `search` / `export` 都支持 `-n` / `--limit` 指定条数。默认值只是为了避免一次性输出过多消息，不是硬上限。

会话/消息输出里都带 `chat_type` 字段，取值为 `private` / `group` / `official_account` / `folded`。`official_account` 涵盖公众号、订阅号、服务号及 `mphelper` / `qqsafe` 等系统通知；`folded` 对应微信里的"订阅号折叠"和"折叠群聊"两个聚合入口。

群聊里的 `last_sender`、`sender` 和 `stats` 的 `top_senders` 会优先使用群昵称（群名片）。如果本地数据库里没有对应群昵称，则回退到联系人备注、微信昵称或 username。

`history` / `search` / `new-messages` / `attachments` 以及 `stats.top_senders`，在群聊上下文里还会附带稳定身份三件套：

- `sender_username`：稳定 wxid，用来区分两个昵称同名的成员
- `sender_contact_display`：通讯录里的显示名（备注 > 昵称 > wxid 兜底）
- `sender_group_nickname`：群名片本身（同 `sender` 的来源，方便机器读取时不必再解析）

解析不到 wxid 时（id2u 没命中且老格式 `wxid_xxx:\n...` 前缀也不存在）这三字段不会输出，避免伪造空字段污染下游过滤。

`history` / `search` / `sessions` / `unread` / `new-messages` / `stats` / `attachments` 现在都会附带 `meta`：

- `status`: `ok` / `possibly_stale` / `possibly_stale_unknown_shards` / `windowed`
- `unknown_shards`: 磁盘上存在、但 daemon 当前没有 key 的 `message_N.db` 分片；非空时应先跑 `sudo wx key extract --hook-seconds 90`
- `chat_latest_timestamp` / `chat_latest_db`: 当前命中数据里最新一条消息的时间和分片来源
- `session_last_timestamp`: `session.db` 里 WeChat 自己记录的最新时间；如果明显领先于 `chat_latest_timestamp`，说明结果可能漏了消息

默认情况下，人类用户会在 stderr 看到可执行的 warning；agent / 脚本可直接读 stdout 里的 `meta`。传 `--with-meta` 会额外返回 `per_shard_latest` / `cache_mode_per_shard`，传隐藏 flag `--debug-source` 还会带真实 `shard_paths`。

引用消息会在 `history` / `search` / `new-messages` 输出中显示当前回复和被引用原文：

```text
[引用] 当前回复
  ↳ 发送者: 被引用内容
```

`--type link` / `--type file` 会包含微信 appmsg 里的链接、文件、合并聊天记录和引用消息等变体；搜索时也会匹配解压后可见的引用原文。

### 朋友圈（SNS）

三个独立命令，区分"通知"和"帖子"：

```bash
wx sns-notifications                             # 点赞/评论通知（默认仅未读）
wx sns-notifications --include-read -n 100       # 含已读

wx sns-feed                                      # 近 20 条朋友圈（时间线）
wx sns-feed --user "张三"                        # 限定作者
wx sns-feed --since 2026-04-01 -n 100            # 按时间

wx sns-search "关键词"                           # 全文搜索朋友圈正文
wx sns-search "婚礼" --user "李四" --since 2023-01-01
```

- **sns-notifications** 返回互动通知：`type`（`like`/`comment`）、`from_nickname`、`content`（评论正文）、`feed_preview` + `feed_author`（对应原帖）
- **sns-feed** / **sns-search** 返回朋友圈帖子：`author`、`content`（正文）、`media`、`media_count`、`location`、`timestamp`；`media` 字段含每张图的 url/thumb/key/token/md5/enc_idx/size，供下游做图片代理或离线渲染。`media_count = media.len()`，按 DOM 解析的合法 `<media>` 子节点计数（malformed XML 返回 0）

朋友圈数据只覆盖你本地刷到过的帖子（微信 app 按需下载）。

### 公众号文章

公众号文章推送存在独立的 `biz_message_*.db` 分片，用 `biz-articles` 单独查：

```bash
wx biz-articles                                   # 最近 50 篇
wx biz-articles -n 200                            # 更多
wx biz-articles --account "返朴"                  # 限定公众号（名称模糊匹配）
wx biz-articles --since 2026-05-01 --until 2026-05-10
wx biz-articles --unread                          # 仅有未读的公众号，每号取最新 1 篇
wx biz-articles --json | jq '.[].url'             # 下游消费 URL
```

每条返回：`account` / `account_username` / `title` / `url` / `digest` / `cover_url` / `time` / `timestamp` / `recv_time_str`。多图文推送会展开成多行。

### 附件提取（图片）

聊天里的附件本体存在 `xwechat_files/<wxid>/msg/attach/...` 下的 `.dat` 文件，需要按消息所在 `message_resource.db` 的 md5 + 平台相关 image key 解码才能拿到原图。

```bash
# 1) 列出会话里的图片附件，先拿到不透明的 attachment_id
wx attachments "张三"
wx attachments "AI群" --kind image -n 100
wx attachments "AI群" --since 2026-04-01 --until 2026-04-15

# 2) 把单个 attachment_id 解密写出去（扩展名建议保留 .jpg / .mp4 等）
wx extract <attachment_id> -o ~/Desktop/photo.jpg
wx extract <attachment_id> -o /tmp/x.jpg --overwrite
```

`attachments` 输出每条带：`attachment_id` / `kind` / `type` / `local_id` / `timestamp` / `time`，群聊里还有 `sender` 以及稳定身份三件套 `sender_username` / `sender_contact_display` / `sender_group_nickname`（语义同 `history` / `search` / `new-messages`：`sender_username` 是 wxid，用于两个同名成员之间的稳定区分；解析不到 wxid 时这三字段不输出）。当前 `kind` 固定为 `image`；命令名保留成 `attachments` 是为了后续扩到其他附件类型时不 break CLI。

`extract` 输出报告里带：`md5` / `dat_path` / `dat_size` / `output` / `output_size` / `format`（实际识别出的图片格式：jpg / png / gif / webp / hevc 等）/ `decoder`（实际选用的解码器：`legacy_xor` / `v1_aes` / `v2`）。

支持的解码档位：
- **legacy XOR**：早期单字节 XOR，无 magic（按文件首字节探测格式自动反推）
- **V1 fixed-AES**（`07 08 V1 08 07`）：AES-128-ECB + 固定 key `cfcd208495d565ef`
- **V2 AES + XOR**（`07 08 V2 08 07`）：AES-128-ECB + raw + XOR；AES key 平台派生

V2 image key 提取：
- **macOS**：`kvcomm` cache（`key_<uin>_*.statistic` 文件名取 uin → `md5(str(uin) + wxid)[:16]`）+ brute-force fallback（`md5(str(uin))[:4] == wxid_suffix` 枚举 2^24）；xor_key = `uin & 0xff`，**不是硬编码 0x88**
- **Windows**：扫 `Weixin.exe` 内存匹配 `[A-Za-z0-9]{32|16}` 候选，按 V2 template ciphertext-block 反验
- **Linux**：上游空白，遇到 V2 .dat 会报 unsupported

### 联系人 & 群组

```bash
wx contacts                  # 联系人列表
wx contacts --query "李"     # 按名字搜索
wx members "AI交流群"        # 群成员列表
```

`wx members --json` 返回的成员字段包括：

- `username`：微信内部 username
- `display`：用于展示的名称，优先使用群昵称
- `contact_display`：联系人备注或微信昵称
- `group_nickname`：群昵称；本地没有记录时为空字符串
- `is_owner`：是否群主

### 收藏 & 统计

```bash
wx favorites                          # 全部收藏
wx favorites --type image             # 按类型筛选（text/image/article/card/video）
wx favorites --query "关键词"         # 搜索收藏内容
wx stats "AI群"                       # 聊天统计
wx stats "AI群" --since 2026-01-01   # 指定时间范围
```

### 导出

```bash
wx export "张三" --format markdown -o chat.md
wx export "张三" -n 2000 --format markdown -o chat.md
wx export "AI群" --since 2026-01-01 --format json
```

### 输出格式

默认输出 YAML；`--json` 可切换为 JSON。对 agent 而言，`history` / `search` / `sessions` / `new-messages` / `stats` / `attachments` 的 stdout 现在是 wrapper，而不是裸数组：

```bash
wx sessions --json
wx search "关键词" --json | jq '.results[0].content'
wx new-messages --json
wx history "张三" --json | jq '.meta'
wx history "张三" --json --with-meta | jq '.meta.cache_mode_per_shard'
```

### Daemon 管理

```bash
wx daemon status
wx daemon stop
wx daemon logs --follow
```

---

## 架构

```
wx (CLI) ──Unix socket──▶ wx-daemon (后台进程)
                              │
                    ┌─────────┴──────────┐
               DBCache               联系人缓存
           (mtime 感知复用)
```

daemon 首次解密后将数据库和 mtime 持久化到 `~/.wx-cli/cache/`。重启后 mtime 未变则直接复用，无需重解密。

```
~/.wx-cli/
├── config.json       # 配置
├── all_keys.json     # 数据库密钥
├── daemon.sock       # Unix socket
├── daemon.pid / .log
└── cache/
    ├── _mtimes.json  # mtime 索引
    └── *.db          # 解密后的数据库
```

---

## 原理

微信 4.x 使用 SQLCipher 4 加密本地数据库（AES-256-CBC + HMAC-SHA512，页级 raw key）。每个 DB 有独立的 32-byte AES key；密钥在微信进程打开 DB 时出现在内存中。

- **首次**：`sudo wx init` — 内存扫描（`x'key+salt'` + salt 邻接）+ 可选 LLDB hook，写入 `~/.wx-cli/all_keys.json`
- **补齐 / 新分片**：`sudo wx key extract --hook-seconds 90`（与 `init --force` 等价，是产品推荐命令）

之后 daemon **优先 SQLCipher 在线打开**加密库（无需全量预解密）；必要时才解密到 `~/.wx-cli/cache/` 并按 mtime / WAL 增量更新。

macOS 取钥依赖本机 Terminal 的 `task_for_pid` / 调试能力，**与是否关闭 SIP 无关**（SIP 保护的是系统组件，不是「能不能读微信内存」的开关）。

---

## 常见问题

| 问题 | 处理 |
|------|------|
| `meta.unknown_shards` / doctor 缺关键分片 | `sudo wx key extract --hook-seconds 90`，等待时打开相关聊天 |
| `wx --version` 偏旧 | `git pull && cargo build --release && cp target/release/wx ~/.local/bin/wx` |
| 是否必须关 SIP？ | **否** |
| 是否必须 ad-hoc 重签微信？ | **默认否**；仅 SSH/无 GUI 且 attach 失败时考虑 |
| daemon 无响应 | `wx daemon stop` 后任意查询会自动重启 |

---

## 致谢

本项目受 [ylytdeng/wechat-decrypt](https://github.com/ylytdeng/wechat-decrypt) 启发，在其基础上进行了重新设计与实现。感谢原作者的研究与探索。

---

## 免责声明

本工具仅用于学习和研究目的，用于解密**自己的**微信数据。请遵守相关法律法规，不得用于未经授权的数据访问。

## 多账号、身份与群列表（fork 契约）

同机多账号，每次调用传 `--wxid <账号原始 wxid>`。可选 `--pid <主进程 PID>`；PID 必须属于该账号数据库的占用主进程，不能指向 `--type=` 辅助进程。Windows 用 CIM 查询命令行、Restart Manager 只读查询数据库占用；不调用关闭或重启应用的 API。macOS/Linux 的显式账号选择使用 `pgrep` 和 `lsof`。

```bash
wx --wxid wxid_example init
wx --wxid wxid_example --pid 1234 key extract
wx --wxid wxid_example whoami --json
wx --wxid wxid_example groups --json
wx --wxid wxid_example doctor --json
```

账号目录为 `~/.wx-cli/accounts/<SHA-256(wxid)>/`，含 `config.json`、`all_keys.json`、`cache/`、`decrypted/`、`last_check.json`、`watch_state.json`、`shard-meta.json`、daemon PID 和日志。哈希保留账号大小写差异，避免 Windows 目录串号。`WX_CLI_HOME` 可指定 `.wx-cli` 根目录；`WX_ACCOUNT`、`WX_PID` 为参数的环境变量等价入口，CLI 参数优先。

指定账号时，不回退到工作目录、可执行文件目录或旧的共用配置；密钥与解密目录必须留在该账号目录。`db_dir` 必须对应 `<wxid>/db_storage` 或 `<wxid>_<4位十六进制后缀>/db_storage`。首次 `init` 仅在该账号目录中自动检测；无法检测时，在账号 `config.json` 中填写准确的 `db_dir` 后再取钥。旧的共用配置不会自动迁移。

Windows 管道名为 `wx-cli-<SHA-256(账号配置目录路径)>`，完整路径加 `\\.\pipe\` 前缀。server、client、ping 共用同一映射。Unix 显式账号 socket 位于临时目录，以配置目录哈希命名，避免路径过长。无账号参数时保留旧配置查找；升级前停止旧版 daemon，避免版本混用。

`whoami --json` 成功（stdout，退出码 0），字段与暂定契约一致：

```json
{"wxid":"wxid_example","displayName":"本人昵称"}
```

`groups --json` 成功（stdout 数组，退出码 0）：

```json
[{"wxid":"123@chatroom","displayName":"客服群","memberCount":2,"members":[{"wxid":"wxid_a","displayName":"群昵称甲"},{"wxid":"wxid_b","displayName":"联系人乙"}]}]
```

群按 wxid 排序，成员按 wxid 排序。成员显示名依次取群昵称、联系人备注、联系人昵称、wxid。只从联系人与群成员表读取完整成员；不扫描消息、不以发言者推算成员数。成员结构不支持或群记录缺失时返回错误，不报告不完整列表。空群返回 `memberCount: 0`、`members: []`；无群返回 `[]`。

两条命令每次直接核对数据库占用与本人联系人，随后只读打开源库，不使用旧 daemon 缓存推断当前登录身份。联系人表必须包含本人，且账号目录必须能唯一匹配本人 wxid。无法确认则失败。明文 SQLite 可直接只读查询，加密库使用该账号已有密钥；两条命令均不会自动扫描进程内存取钥。

两条命令失败时退出码 1，stdout 固定结构：

```json
{"error":{"code":"LOGIN_REQUIRED","message":"错误说明"}}
```

- `LOGIN_REQUIRED`：未初始化、缺数据库、目标账号无占用数据库的主进程。
- `KEY_FAILED`：缺少密钥、密钥文件损坏或密钥不匹配。
- `VERSION_UNSUPPORTED`：联系人/群结构不支持，或无法唯一确认本人。
- `INTERNAL`：配置不匹配、系统查询失败、多主进程歧义或其他内部错误。

Windows 自动启动 daemon 使用 `CreateProcessW(bInheritHandles=false)`；daemon 在自身进程内打开日志，避免调用方捕获 stdout/stderr 时等待 daemon 关闭句柄。


## 按 ID 游标增量读取（fork 契约，票 03/04）

```bash
# 首次：空游标，从该会话各分片的首个 local_id 开始
wx --wxid wxid_example --pid 1234 incremental '123@chatroom' --cursor '' --limit 200
# 下一批：原样传回 next_cursor；不要解码或修改
wx --wxid wxid_example incremental '123@chatroom' --cursor '<next_cursor>' --limit 200
# 省略 --cursor：从该账号、该会话的持久化检查点续读
wx --wxid wxid_example incremental '123@chatroom' --limit 200
```

固定输出 JSON，无需 `--json`。必须用 `--wxid` 或 `WX_ACCOUNT` 选择账号；可用 `--pid` / `WX_PID` 指定主进程。每次复用 whoami 的进程占用与本人身份检查，直接只读打开源库及 WAL，不依赖 daemon、不扫描进程内存、不自动取钥。会话参数为精确群 wxid，不支持昵称模糊匹配。`--limit` 默认 200，范围 1–10000。

成功：退出码 0，stdout 为单个对象：

```json
{
  "account_wxid": "wxid_example",
  "chat_wxid": "123@chatroom",
  "messages": [{
    "server_id": "9007199254740993",
    "svrid": "9007199254740993",
    "local_id": 42,
    "shard": "message/message_0.db",
    "chat_wxid": "123@chatroom",
    "sender_wxid": "wxid_sender",
    "is_self": false,
    "timestamp": 1792454400,
    "type": 1,
    "text": "消息正文",
    "mentions": [],
    "quote": null,
    "raw": {
      "message_content": "wxid_sender:\n消息正文",
      "source": "<msgsource/>"
    }
  }],
  "next_cursor": "<opaque-token>",
  "has_more": false
}
```

`server_id` 与 `svrid` 为相同的十进制字符串，避免 JavaScript 丢失 64 位精度。`local_id` 为分片、会话表内自增 ID；`type` 为 local_type 低 32 位数值，`timestamp` 为 Unix 秒。`text` 为 TEXT/BLOB 解码及 WCDB zstd 解压后的正文，移除已识别的群发送者前缀；XML 保留原文。发送者先取该分片 Name2Id，缺失时取群正文前缀；无法确认时 `sender_wxid` 与 `is_self` 为 null，不猜本人。server_id 未分配（0/null）时返回 MESSAGE_NOT_READY，且不推进，等待入库完成后重试。

每条消息附带以下结构化字段（下划线命名）：

- `mentions`：从 `source` 的 `msgsource/atuserlist` 读取成员 wxid，支持 CDATA、逗号分隔，去除首尾空白与空项；`@所有人` 保留原始标识 `notify@all`，不展开成员、不按昵称推断。无列表、source 缺失或 XML 解析失败时为 `[]`。
- `quote`：仅消息类型 49、`appmsg/type=57` 的引用消息返回对象，例如 `{"message_id":"9007199254740993","sender_wxid":"wxid_service"}`。`message_id` 来自 `refermsg/svrid`，保留十进制字符串；`sender_wxid` 优先取 `refermsg/chatusr`。chatusr 缺失或为空时，按 svrid 在当前账号、当前群的全部本地消息分片回查（含游标之前及本批之外的消息），复用 Name2Id / 正文前缀识别发送者。不使用 fromusr 或 displayname 猜测；查不到时 `sender_wxid` 为 null。引用子字段缺失或无效时为 null；非引用、缺 refermsg 或 XML 解析失败时 `quote` 为 null。
- `raw`：`{"message_content":"消息记录原文","source":"source XML"}`。两者均为 TEXT/BLOB 的 UTF-8 文本，WCDB 压缩标记为 4 时先做 zstd 解压。`message_content` 保留群发送者前缀与 XML，不做正文格式化；source 保留 XML 原文，即使 XML 无法解析。source 列不存在、值为 NULL 或解码失败时 `raw.source` 为 null；缺压缩标记列时按未压缩读取。

新增字段的缺失、XML 解析或 source 解压失败只置空对应字段，不中断批次。原有正文解码、数据库与游标错误契约保持不变。持久化批次原样重放；升级前已保存的批次仍按原 JSON 返回，只有新读取的批次包含新增字段。

按分片相对路径字典序、各表 local_id 升序分页；不保证跨分片发送时间顺序。晚到消息、同秒消息均按入库 ID 读取。游标包含版本、账号、会话表、各分片代际与已读 ID。每次从磁盘发现新 message_N.db；新分片从首行读起，不依赖旧密钥索引。任一分片缺钥、查询失败均整体失败，不交付残缺批次。已有分片或表消失、代际锚点改变时报 DB_ROTATED；不会清空游标或自动从头读。

联系人表中存在的精确群 wxid（`@chatroom`），在所有消息分片均无该群会话表时返回空批次：`messages: []`、`has_more: false`；首次尚无消息目录或分片也遵守此规则。空批次照常保存并生成新的 `next_cursor`，同一输入游标与 limit 稳定重放原批次。继续传回 `next_cursor` 后，新出现的会话表或分片从首个 local_id 读取，不漏读、不重复交付已推进的消息。联系人中也不存在的会话仍返回 CHAT_NOT_FOUND；已有分片或表消失仍返回 DB_ROTATED。

状态位于该账号目录 `incremental.db`，与旧 `new-messages` 的时间戳状态独立。SQLite 事务保存检查点与全部批次，避免 Windows 覆盖 rename 的空窗。显式 `--cursor` 不推进隐式检查点；省略参数才保存并推进检查点。既有状态库缺初始化版本、缺表、截空或内容损坏均返回错误，不回退为空。全部批次按输入游标和 limit 持久化，重放返回相同消息、新游标及 has_more，后续插入不改变已返回批次。每次新读取增加游标序列；空批次也必须回传新的 next_cursor 才能继续探测新增消息。重放仍核对当前账号、源库代际与边界，失效时返回错误。固定 limit 使用同一游标链。首次空游标重放也遵守上述规则。

操作端必须自行持久化收到的 next_cursor，并在失败时重传原输入游标。隐式检查点仅提供进程重启续读，不代表下游确认；若调用方丢失 stdout，需用上次显式游标恢复。批次记录含正文，只保存在本账号状态库（Unix 文件权限 0600，Windows 继承账号目录 ACL）；空批次也占状态空间，随轮询累计增长，保留它才能跨进程重放。

失败：退出码 1，stdout 为 `{"error":{"code":"...","message":"..."}}`。

| code | 条件 |
|---|---|
| ACCOUNT_REQUIRED | 未显式选择账号 |
| CURSOR_INVALID | 游标格式、版本或字段非法 |
| CURSOR_ACCOUNT_MISMATCH | 游标属于其他账号 |
| CURSOR_CHAT_MISMATCH | 游标属于其他会话 |
| CURSOR_OUT_OF_RANGE | 已读 ID 越界或边界记录消失 |
| DB_ROTATED | 已有分片/表消失、源库代际或边界 server_id 改变 |
| CHAT_NOT_FOUND | 各分片均无该会话表，且联系人中不存在该精确群 wxid |
| MESSAGE_NOT_READY | server_id 尚未分配 |
| LOGIN_REQUIRED / KEY_FAILED / VERSION_UNSUPPORTED / INTERNAL | 沿用票 02；结构不支持或正文损坏也返回 VERSION_UNSUPPORTED |

参数语法错误由 clap 输出 stderr、退出码 2；执行期错误按上表输出 JSON。不能延续时保留原状态，由操作端报错并人工核实源库后恢复。
