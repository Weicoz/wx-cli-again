# 票 02：交付与验证记录

日期：2026-10-21。分支：`prw2/t02`。基准：`integration/personal-wechat-read-v2` / `077a54cbfe679bda963cd038d8440422907fc797`。

工作状态：已完成本票代码与本机验证。Windows 实机行为未验证，按约定交编排方在 zero-s-pc 只读复测。未改控制端票据或规格，未写 Git 索引，未提交、合并或推送。

## 逐条验收

| 验收项 | 实现及证据 | 验证状态 |
|---|---|---|
| 按指定账号/进程取钥 | `src/scanner/windows.rs:24` 使用共享选择器；`src/cli/init.rs:113` 写密钥前核对本人；Windows 图片密钥亦复用选择器 | 已验证：纯逻辑单测与 GNU 编译。Windows 取钥未验证 |
| Windows 只选主进程 | `src/process.rs:12` 排除 `--type=` / `--type`、不可读命令行；支持显式 PID；账号按数据库占用 PID 交集选择，拒绝歧义 | 已验证：`src/process.rs:235` 等 4 个测试。真实 CIM/Restart Manager 未验证 |
| pipe、配置、密钥、缓存与读取位置隔离 | `src/config.rs:154`、`:191`、`:204`；server/client/ping 共用 pipe 映射；账号配置不回退到 cwd/exe/共用配置，密钥与解密目录受限于本账号目录 | 已验证：`src/config.rs:608` 映射测试、两份合成库 CLI 测试。Windows 双账号同时运行未验证 |
| new-messages 状态隔离 | `src/cli/new_messages.rs:8` 改用账号 `cli_dir()/last_check.json`；watch、shard-meta、PID、日志均已有统一目录接缝 | 已验证：路径映射单测及差异复核；只改状态路径，未改游标格式或推进逻辑 |
| 身份及错误分类 | `src/cli/account.rs:202`、`:24`；直接只读源库，每次核对进程占用及本人联系人；身份必须唯一，且与显式 `--wxid` 严格一致 | 已验证：`tests/account_cli.rs:4`、`:24`、`:105`。Windows 登录、退出及换号未验证 |
| 群列表仅输出群、群名、成员数 | `src/cli/account.rs:125` 仅查询以 `@chatroom` 结尾的联系人；从群成员表取完整成员，不以发言人估算数量，不读取消息 | 已验证：两份未建消息表的合成 SQLite 库，CLI 输出与预期 JSON 完全相等 |
| 群成员 wxid 与群内显示名 | 复用 `src/daemon/query.rs:1761` 昵称解析；成员显示名优先群昵称，再备注、昵称、wxid | 已验证：`tests/account_cli.rs:24` 同时含联系人备注与不同的群昵称，输出群昵称 |
| 两个账号目录分别读取不串读 | `tests/account_cli.rs:24` 为两账号创建独立配置及明文 SQLite，分别核对身份、群和成员 | 已验证：两个账号输出不同且等于各自预期；后缀重叠账号另有拒绝回归 |
| doctor Windows 进程及 IPC 检测 | `src/cli/doctor.rs:70` 用共享平台检测器；`:286` 实际 ping daemon，不以 Windows 上不存在的 sock 文件判断 | 已验证：共享逻辑单测与 GNU 编译；Windows doctor 输出未验证 |
| 首次自动启动 daemon 不继承输出句柄 | `src/cli/transport.rs:201` 明确 `CreateProcessW(..., bInheritHandles=false, ...)`；`src/daemon/mod.rs:63` 在 daemon 内打开日志 | 已验证：GNU 编译。`tests/account_cli.rs:207` 保留捕获输出及双 daemon 集成测试，本机因沙箱禁止 socket bind 运行时跳过；Windows 真实句柄行为未验证 |
| Windows GNU release 交叉编译 | 下方完整命令退出码 0，生成 x86-64 PE 文件 | 已验证。Windows 执行未验证 |

## 最终 CLI / JSON 契约

多账号调用每次传 `--wxid <原始账号 wxid>`；可选全局 `--pid <主进程 PID>`，支持放在子命令前或后。PID 必须属于目标数据库的主进程，不是辅助进程。

```bash
wx --wxid wxid_example [--pid 1234] whoami --json
wx --wxid wxid_example [--pid 1234] groups --json
```

上方方括号表示可选参数，不是命令文本的一部分。`WX_ACCOUNT`、`WX_PID` 为环境变量入口，CLI 参数优先；`WX_CLI_HOME` 可覆盖 `.wx-cli` 根目录。

`whoami --json` 成功，退出码 0：

```json
{"wxid":"wxid_example","displayName":"本人昵称"}
```

`groups --json` 成功，退出码 0，stdout 为数组：

```json
[{"wxid":"123@chatroom","displayName":"客服群","memberCount":2,"members":[{"wxid":"wxid_a","displayName":"群昵称甲"},{"wxid":"wxid_b","displayName":"联系人乙"}]}]
```

群和成员分别按 wxid 排序。无群输出 `[]`。成员列表为空时输出 `memberCount: 0` 和 `members: []`。无法取得完整成员结构时失败，不返回推测的成员数或消息聚合列表。

上述两命令业务失败，退出码 1，stdout：

```json
{"error":{"code":"LOGIN_REQUIRED","message":"错误说明"}}
```

| code | 含义 |
|---|---|
| `LOGIN_REQUIRED` | 未初始化、缺数据库、目标账号无占用数据库的主进程 |
| `KEY_FAILED` | 密钥缺失、文件损坏或密钥不匹配 |
| `VERSION_UNSUPPORTED` | 联系人/群表结构不支持，或不能从本人联系人唯一确认 wxid |
| `INTERNAL` | 配置或本人 wxid 与所选账号不符、无法查询进程元数据、多主进程歧义、其他内部错误 |

与暂定身份契约无差异。新增选择参数 `--wxid`、`--pid`；本票固定群命令名与上述数组字段，供票 05 对齐。

身份识别要求联系人表包含本人，且数据库目录为 `<wxid>/db_storage` 或 `<wxid>_<4位hex>/db_storage`。本人匹配不唯一或不等于所选账号时拒绝。指定账号模式不迁移或复用旧共用配置与读取状态。Windows pipe 名同时变更，升级前停止旧 daemon。

## 检查结果

全部 Cargo 检查串行执行。

| 完整命令 | 最终结果 |
|---|---|
| `cargo check` | 已验证，退出码 0；每轮 Rust 改动后执行 |
| `cargo test process::tests` | 已验证首批 3 个进程测试；新增第 4 个错误分类测试最终在全量中通过 |
| `cargo test config::tests` | 已验证，6 个通过 |
| `cargo test --test account_cli -- --nocapture` | 退出码 0；3 个 CLI 集成已验证，1 个 IPC 集成运行时跳过 |
| `cargo test` | 退出码 0；162 个单元测试通过，集成 harness 报 4 个成功，其中 1 个实际因沙箱跳过 |
| `cargo clippy --all-targets` | 已验证，退出码 0；既有 bin 39 / test 51 警告（包含重复），本次新增文件无警告。日志：`target/t02-clippy.log` |
| `git diff --check` | 已验证，退出码 0 |
| `cargo check --target x86_64-unknown-linux-gnu` | 失败：`error[E0463]: can't find crate for core`，本机缺此 target，Linux 编译未验证 |
| `cargo check --target x86_64-pc-windows-msvc` | 失败：缺缓存的 `windows_x86_64_msvc/0.48.5`，下载报 `Could not resolve host: static.crates.io`；MSVC 编译未验证 |

Windows GNU 完整命令：

```bash
CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar cargo build --release --target x86_64-pc-windows-gnu -j 4
```

最终退出码 0：`Finished release profile [optimized] target(s) in 2m 34s`（复用首轮编完的原生依赖）。5 个既有 Windows dead-code 警告。首轮发现 Windows 扫描器在关闭句柄后对未解包 Result 调用 len，已修复，再构建通过。

产物：`target/x86_64-pc-windows-gnu/release/wx.exe`，约 11 MiB。`file` 核验为 `PE32+ executable (console) x86-64`。

SHA-256：`3116862ca11711433211671d5ba137c606846a4062a88efc639b38a48c0558d4`。

IPC 测试明确输出：`SKIP daemon IPC integration: sandbox denies socket bind: Operation not permitted (os error 1)`。不得把 harness 的成功计数解释为已验证真实 daemon / Windows 句柄。

## code-review 与修复

使用 code-review 两个只读子代理，分别审 Standards / Spec，基准比较工作区及新增文件，未以空的 HEAD 三点 diff 漏掉未提交改动。

- Standards：0 项发现。
- Spec：1 项 P1。目录 `wxid_a_abcd` 可被解释为账号 `wxid_a` 的后缀目录，原实现未严格核对本人 wxid，可能串读。已保留唯一匹配门禁，再严格比较 `--wxid`；门禁覆盖身份、群、IPC 查询、daemon 启动、init 早退及写入前、直接语音导出。
- 回归：后缀重叠配置下 whoami/groups/contacts/init/media voice 全部拒绝，无 daemon PID、无语音导出文件。复核确认 P1 已关闭。
- 另外补齐：Windows 进程 JSON 空值/单对象/数组解析；不可读命令行归 INTERNAL，避免误报未登录。相关测试均有修复前失败、修复后通过证据。

## 改动文件

| 文件 | 用途 |
|---|---|
| `Cargo.toml` | 仅加 Windows API feature，未改版本或依赖版本 |
| `README.md` | 多账号与 CLI JSON 契约 |
| `src/main.rs` | 注册进程模块 |
| `src/config.rs` | 账号选择、独立路径/pipe、目录校验、映射测试 |
| `src/process.rs` | 新增进程元数据解析、主进程筛选、数据库占用映射及单测 |
| `src/scanner/windows.rs` | Windows 账号主进程选择与句柄清理 |
| `src/scanner/macos.rs`、`src/scanner/linux.rs` | 显式账号/PID 选择接缝 |
| `src/attachment/image_key/windows.rs` | 图片密钥复用同一账号进程选择器 |
| `src/cli/mod.rs` | 全局选择参数、whoami/groups 命令及 JSON 错误出口 |
| `src/cli/account.rs` | 新增只读身份、完整群目录和统一错误 |
| `src/cli/init.rs` | 指定账号配置、取钥后与早退前身份门禁 |
| `src/cli/media.rs` | 直接导出路径补身份门禁 |
| `src/cli/doctor.rs` | 平台进程检测与真实 IPC ping |
| `src/cli/new_messages.rs` | 仅替换状态文件目录，不改游标 |
| `src/cli/transport.rs` | 账号 IPC、身份门禁、Windows 禁止句柄继承 |
| `src/daemon/mod.rs` | Windows 自行打开日志及启动身份门禁 |
| `src/daemon/server.rs` | 账号 pipe 名 |
| `src/daemon/query.rs` | 仅将既有群昵称解析函数暴露为 pub(crate)，未改消息查询 |
| `tests/account_cli.rs` | 合成库、CLI 契约、隔离、错误和 IPC 集成测试 |
| `docs/personal-wechat-read-v2-t02-validation.md` | 本验收与交接记录 |

## 编排方 Windows 只读复测

1. 在 zero-s-pc 测试号验证 CIM/Restart Manager 能在实际 SSH/普通用户权限下找到持有 contact/session DB 的主进程；辅助 PID、错误账号 PID 必须拒绝。普通用户权限不足应单独记录，不与未登录混淆。
2. 两账号同时运行：分别传 --wxid，检查 config/keys/cache/state/PID/log/pipe 均独立；一个 daemon 停止不影响另一个。首次取钥前核对所选账号及 PID；本机未执行任何取钥或进程内存命令。
3. whoami 对照测试号本人；退出或换号后再次查询必须停止旧身份读取。确认该微信版本本人联系人及目录格式满足身份契约；缺钥/错钥、未知结构分别返回对应 JSON 错误。
4. groups 对照群与 wx members，检查成员数、成员 wxid、群昵称；不得含消息，不得只列发言者。
5. 停止测试账号 daemon 后，用操作端子进程管道首次调用 contacts/sessions，确认 stdout/stderr EOF、调用及时结束且 daemon 保持运行；随后调用立即返回，日志位于相应账号目录。
6. doctor 对照实际主进程 PID 和 pipe 状态。Linux target、MSVC 依赖/工具链可用时补各自检查。

未做：真实微信数据库或密钥读取、进程内存扫描、远端实测、安装或部署、Git 提交/推送。未验证：上述 Windows 实机项，以及受环境阻塞的 Linux/MSVC 检查。
