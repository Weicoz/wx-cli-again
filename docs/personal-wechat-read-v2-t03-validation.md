# 票 03：按 ID 游标增量读取验收记录

日期：2026-10-21。工作区：`prw2/t03`，基线 `59e206f0e25a75bd28398b382793e202d3816f5b`。仅工作区交付；未执行 add、commit、merge、push。控制端票据、PRD、实测记录均只读参考。

## 范围

新增 `wx --wxid <账号> [--pid <主进程>] incremental <群wxid> --cursor <不透明游标> --limit 200`。固定 JSON，退出码成功 0、执行期错误 1。显式空字符串为首次读取；后续原样回传 next_cursor。省略 cursor 时，使用账号与会话隔离的持久化检查点。

每条消息含 server_id、svrid（同值十进制字符串）、local_id、shard、chat_wxid、sender_wxid、is_self、timestamp、type、text。发送者无法确认时 sender_wxid/is_self 为 null。正文保留 XML，不新增 @ 与引用字段。完整字段与错误表见 README「按 ID 游标增量读取」。

## 逐条验收

| 验收项 | 实现与验证证据 |
|---|---|
| 不透明游标输入/输出 | `src/incremental.rs:32` 的 Cursor::decode/encode；版本、账号、会话表、分片代际、local_id 与批次序列。`tests/incremental_cli.rs:3` CLI 合成库测试直接回传 next_cursor，不解析内容 |
| 稳定消息 ID 与正文身份字段 | `src/incremental.rs:467`（same_second_and_late_messages_advance_by_local_id） 逐字段断言 JSON；9007199254740993 保持十进制字符串，无 JS 精度损失 |
| 同秒多条、晚到旧时间 | 同一测试输出同秒两条；后续 local_id=3、timestamp=90 继续输出，无时间戳截断 |
| 多分片与新分片 | `src/incremental.rs:513`（new_shards_continue_and_disappearing_empty_tables_are_rotation）：旧分片新增与新分片 local_id=1 同时读取；独立表位置不冲突 |
| 重放一致与重启续读 | `src/incremental.rs:489`（replay_survives_new_inserts_and_process_state_reopen）、`tests/incremental_cli.rs:3` 子进程重启测试；全部批次持久化，空批次也有新 next_cursor（`src/incremental.rs:319`、`:330`） |
| 账号独立 | `tests/incremental_cli.rs:3` 双账号相同会话/相同本地 ID 分别返回 101 与 201；交叉游标返回 CURSOR_ACCOUNT_MISMATCH；错误 PID 返回 LOGIN_REQUIRED |
| 代际变化不静默重置 | `src/incremental.rs:646`（truncation_rebuild_and_file_removal_never_reset_cursor）、空表消失、消息目录消失；返回 DB_ROTATED 或 CURSOR_OUT_OF_RANGE，保留检查点 |
| 源库错误不交付半批 | `src/incremental.rs:680`（unreadable_new_shard_returns_no_partial_batch_or_checkpoint）；先缺钥失败，再修复源库，首次两分片消息仍全部输出 |
| 错误码区分 | invalid/account/chat 游标、CHAT_NOT_FOUND、CURSOR_OUT_OF_RANGE、DB_ROTATED、VERSION_UNSUPPORTED、KEY_FAILED、MESSAGE_NOT_READY 各有样本 |
| Windows 相关逻辑 | `src/incremental.rs:704`（windows_key_separators_encrypted_wal_and_zstd_use_synthetic_data） 在 macOS 运行，验证反斜杠密钥映射、SQLCipher、WAL、zstd；状态事务走 SQLite，无 Windows rename 空窗 |
| 状态损坏停止 | `src/incremental.rs:770` 的截空、丢 checkpoints、丢 batches、错误版本及 `src/incremental.rs:748` 的非 SQLite 内容均报错；不得自动初始化既有文件 |

## 检查

日志位于本 worktree `target/t03-*.log`，不提交构建产物。最终一轮检查按序执行。

| 完整命令 | 结果及日志 |
|---|---|
| `cargo check` | 已验证，退出码 0；`target/t03-check-final.log` |
| `cargo test incremental::tests` | 已验证，11 条本票单元测试；`target/t03-review-green.log` |
| `cargo test --test incremental_cli` | 已验证，2 条 CLI 合成库测试；`target/t03-cli-final.log` |
| `cargo test` | 已验证，173 单元 + 5 条实际执行集成；另 1 项环境预检跳过（harness 仍记为成功）；`target/t03-cargo-test.log` |
| `cargo clippy --all-targets` | 已验证，退出码 0；既有 bin 39 / test 51 警告（含重复），新增文件无警告；`target/t03-clippy.log` |
| `cargo test --test account_cli captured_cli_returns_while_two_account_daemons_keep_running -- --nocapture` | 预检跳过：`SKIP daemon IPC integration: sandbox denies ps: Operation not permitted (os error 1)`；`target/t03-daemon-preflight.log`。不得算作 daemon 实测 |
| `cargo check --target x86_64-unknown-linux-gnu` | 失败：`error[E0463]: can't find crate for core`；缺 Rust target；`target/t03-check-linux.log` |
| `cargo check --target x86_64-pc-windows-msvc` | 失败：Cargo 缓存写入 `Operation not permitted (os error 1)`，依赖 windows_x86_64_msvc/0.48.5；`target/t03-check-msvc.log` |
| `git diff --check` | 已验证，无空白错误 |

Windows GNU release 最终命令：

```bash
CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar cargo build --release --target x86_64-pc-windows-gnu -j 4
```

已验证：最终命令退出码 0，``Finished `release` profile [optimized] target(s) in 3m 43s``。5 个既有 Windows dead-code 警告；日志 `target/t03-windows-gnu-build.log`。

产物：`target/x86_64-pc-windows-gnu/release/wx.exe`（11807744 bytes）；`file` 核验为 PE32+ console x86-64、MS Windows。SHA-256：`d2dab321724659c3afd3de66f01aea7e35296014bd7b1c873924b0343f270c8e`。本次只核验格式与构建，不运行 Windows 二进制。

## Code review

按 code-review 分 Standards / Spec 两轴并行只读审查。基线为上述 HEAD；工作区差异加未跟踪新增文件全文，无新增提交。

1. Standards P1：既有状态库截空/丢表时会静默初始化。按首次创建/既有状态分流，检查版本与完整 schema；补截空、丢表与错误版本回归。
2. Spec P1：空批次未缓存，重放会随新增消息改变。改为全部批次持久化，next_cursor 增加序列；补「空批次重放仍空、新游标读取新增」回归。

两项均有修复前失败记录 `target/t03-review-red.log`。修复后 `target/t03-review-green.log` 11 项通过；Standards / Spec 两轴静态复审均确认 P1 关闭，未发现新阻断项。额外收紧 Unix 状态库为 0600，并在读取与缓存重放交付前再次检查源库代际；替换加密库优先报 DB_ROTATED，不误报 KEY_FAILED。

## 改动文件

| 文件 | 内容 |
|---|---|
| `src/main.rs:9` | 注册 incremental 模块 |
| `src/cli/mod.rs:206`、`:491` | incremental 参数、账号选择与 JSON 错误出口 |
| `src/cli/account.rs:32`、`:50` | 复用登录占用、本人核对和只读明文/SQLCipher 打开逻辑 |
| `src/incremental.rs:210` | ID 游标读取与事务持久化；`:467` 起合成样本测试 |
| `tests/incremental_cli.rs:3`、`:99` | 双账号/PID/进程重启/错误契约、不选账号拒绝读取 |
| `tests/account_cli.rs:213` | 既有 daemon 测试 ps 权限预检，受限环境不再启动无法清理的 daemon |
| `README.md:480` | 最终命令、字段、分页/重放/持久化语义与错误码 |
| `docs/personal-wechat-read-v2-t03-validation.md` | 本验收记录与编排方交接 |

TDD：`target/t03-red-01.log` 至 `t03-red-06.log` 分别记录 ID 读取、插入后重放、CLI 接入、空表轮转、待分配 server_id、目录消失的失败；对应 `green` 日志通过。审查修复另有 `target/t03-review-red.log` / `t03-review-green.log`。最后一轮全量结果覆盖所有最终代码。

## Windows 编排方复测

1. 用本次 wx.exe 与真实微信 4.x 测试账号，核对 --wxid / --pid、本人身份与群 sender_wxid/is_self。不得把 macOS 合成库测试视为 Windows 实测。
2. 同秒发多条、延迟入库，limit=1 连续续读；server_id 不丢精度。未分配服务端 ID 时停止，分配后恢复。
3. 子进程捕获 stdout/stderr，保存游标后杀掉读取进程再启动，显式/隐式两条续读路径分别验收。空、非空批次原游标重放结果必须完全一致。
4. 新增 message_N 分片、缺新分片密钥、旧分片删除/替换/重建；要求新增可续读，无法延续时明确错误，状态不清空。真实微信轮转时机本次未观察。
5. 普通用户与管理员权限分别检查文件创建时间、SQLCipher/WAL、账号状态目录 ACL、SQLite journal/事务、状态损坏错误。
6. 重跑票 02 daemon 管道捕获测试；本次新增命令直接读源库，不依赖 daemon。

## 未做与限制

未读取本机真实微信数据库、真实密钥或微信进程内存；所有自动化样本均临时合成。未改 @ 与引用结构、未改旧 new-messages 时间戳命令、未修改 Cargo.toml/Cargo.lock、未提交推送、未做 Windows 真机复测。

隐式检查点在交付 stdout 前持久化，不代表下游 ack；操作端必须保存并重传显式游标，以恢复 stdout 丢失。全部批次缓存随轮询累计增长，保留缓存才能跨进程精确重放。

首次全量测试曾启动两个合成库 daemon，随后沙箱禁止 ps 与信号，清理失败：PID 78086、78117；样本目录 `/var/folders/55/xwg7x1wj6m93xp67ncz01_3m0000gn/T/wx-daemons-cli-78076`。编排方在沙箱外先确认 PID 与样本 daemon.pid/二进制路径匹配，再停止并清理该测试目录；不得按旧 PID 直接终止其他进程。已给既有测试补 ps 预检，后续受限测试在启动 daemon 前明确跳过。
