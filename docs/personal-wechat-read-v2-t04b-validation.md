# T04b：已知群无消息表增量读取验收记录

日期：2026-10-21。工作区：`/Users/weicoz/File/git/wx-cli-again/.worktrees/t04b`。分支：`prw2/t04b`。基线与当前 HEAD：`d3aa7edd61c5c36ff0fb29046df5227a7284312f`。对 `integration/personal-wechat-read-v2` 的 ahead/behind 为 `0/0`，无冲突。仅交付工作区差异；未执行 git add、commit、merge、push。

执行方式：本机 Codex，按 implement → TDD → code-review。两名只读子代理分别审查 Standards 与 Spec；主会话独占写入与检查。无远端派单、部署或真实微信数据访问。

## 需求与根因

本票无独立票据文件，以用户本轮「本票专属约定」为准。背景为控制集成工作区 `.scratch/personal-wechat-read-suggestion/research/windows-wx-cli-v2-test-2026-10-21.md` 的「发现」，父规格为同目录 `PRD.md` 的「wx-cli fork 改造」与 Testing Decisions。

原始调用链：`src/cli/mod.rs:491` → `incremental::poll` → `open_shards`。基线 `d3aa7ed` 的 `src/incremental.rs:103` 在消息目录缺失时返回 CHAT_NOT_FOUND；`:170` 在所有分片没有会话表时返回 CHAT_NOT_FOUND。两条路径均未查询联系人，把「没有消息表」等同「会话不存在」。CLI 虽验证本人联系人，却没有验证目标群是否已知。

修复：仅在所有分片均无会话表时，查询该账号只读联系人库，精确匹配 `@chatroom` wxid。已知群复用原空批次、序列、检查点与重放逻辑；未知会话保持 CHAT_NOT_FOUND。初始无消息目录/分片同样返回空批次。既有分片或表消失仍为 DB_ROTATED。

## 逐条验收

| 验收项 | 状态与证据 |
|---|---|
| 确认误判路径与原因 | 已验证：上述基线位置；`target/t04b-evidence/red.log` 在已知群返回 `{"error":{"code":"CHAT_NOT_FOUND","message":"各消息分片均无该会话表"}}`，退出码 101 |
| 已知群无消息表返回空批次 | 已验证：`src/cli/account.rs:92` 精确联系人查询，`src/incremental.rs:169` 无表分流；`tests/incremental_cli.rs:144` 断言账号、会话、messages=[]、has_more=false 与非空 next_cursor |
| 同游标稳定重放、空批次推进 | 已验证：`tests/incremental_cli.rs:151`、`:169`；建表前后原输入均重放原批次，续传 next_cursor 生成不同空游标。复用 `src/incremental.rs:343` 的持久化重放与 `:444` 的序列推进 |
| 后建表从头读取，不丢不重 | 已验证：`tests/incremental_cli.rs:164` 创建消息表并写入 301、302；limit=1 依次得到 301/local_id=1、302，末次为空；`:177` 起断言分页与 has_more |
| 初始无目录/分片，后出现分片 | 已验证：`tests/incremental_cli.rs:208` 起使用独立已知群，依次测试无目录、空目录、首次分片；旧空批次稳定重放，隐式检查点读到 401/local_id=1，后续为空 |
| 未知会话仍 CHAT_NOT_FOUND | 已验证：`tests/incremental_cli.rs:141` 同群在另一账号不存在；`:228` 完全未知群也返回相同错误码 |
| 原轮转与边界检查保留 | 已验证：17 项 `incremental::tests::` 全部通过，含 missing_message_directory_distinguishes_first_read_from_rotation、new_shards_continue_and_disappearing_empty_tables_are_rotation、truncation_rebuild_and_file_removal_never_reset_cursor |
| README 契约同步 | 已完成：`README.md:534` 写明已知群空批次、无目录/分片、新表续读、重放与轮转；`:550` 更新 CHAT_NOT_FOUND 条件 |
| 合成样本与进程清理 | 已验证：所有样本位于测试临时目录；CLI 使用 WX_CLI_HOME 与假的 pgrep/lsof，仅访问合成 SQLite。新增路径不启动 daemon，`:251` 断言无 daemon.pid；两次红灯遗留合成目录已核实并删除，见 cleanup.log |

## TDD 与检查

检查串行执行，每个 Cargo 命令最多 `-j 2`。未改 Cargo.toml/Cargo.lock。原仓与本工作区 Cargo.lock 经 cmp 相同，Windows GNU 构建复用复制至本工作区的 release 缓存；没有写主仓缓存。

日志目录：本工作区 `target/t04b-evidence/`（忽略，不提交）。

| 完整命令 | 结果与日志 |
|---|---|
| `cargo test -j 2 --test incremental_cli incremental_json_selects_account_pid_and_resumes_after_restart -- --exact --nocapture` | TDD 首轮失败：已知群报 CHAT_NOT_FOUND；`red.log`。补联系人判断后通过；`green.log` |
| 同上命令，补无目录场景 | TDD 第二轮失败：`账号没有消息分片`；`red-no-directory.log`。修复后通过；`green-no-directory.log` |
| `cargo check -j 2` | 已验证，5 次均退出 0；最终 `check-formatted.log`。每次 Rust 编辑后立即执行 |
| `cargo test -j 2 incremental::tests:: -- --test-threads=1` | 已验证，17 项通过；`incremental-unit.log` |
| `cargo test -j 2 --test incremental_cli --test account_cli -- --test-threads=1` | 已验证：incremental CLI 2 项、account CLI 3 项实际运行通过；另 1 项 daemon 预检跳过。harness 共记 6 项成功；`cli-regression.log` |
| `cargo test -j 2 --test account_cli captured_cli_returns_while_two_account_daemons_keep_running -- --exact --nocapture` | 跳过：`SKIP daemon IPC integration: sandbox denies ps: Operation not permitted (os error 1)`；未启动 daemon，不计端到端成功。`daemon-preflight.log` |
| `cargo clippy --all-targets -j 2` | 已验证，退出 0；bin 39、bin test 51 条既有警告（含 39 条重复）。本次修改的文件没有警告；`clippy.log` |
| `cargo check --target x86_64-unknown-linux-gnu -j 2` | 阻塞，退出 101：`error[E0463]: can't find crate for core`，缺 Rust target；`check-linux.log` |
| `cargo check --target x86_64-pc-windows-msvc -j 2` | 阻塞，退出 101：写 Cargo 缓存 windows_x86_64_msvc-0.48.5.crate 时 `Operation not permitted (os error 1)`；`check-msvc.log` |
| `git diff --check` | 已验证，无空白错误 |

Windows GNU 构建完整命令：

```bash
CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar cargo build --release --target x86_64-pc-windows-gnu -j 2
```

已验证：退出 0，`Finished release profile [optimized] target(s) in 1m 18s`，5 条既有 Windows dead-code 警告；`windows-gnu-build.log`。产物 `target/x86_64-pc-windows-gnu/release/wx.exe`，11823104 bytes；`file` 为 PE32+ console x86-64、MS Windows。SHA-256：`8682881fd309b1dbb0d34b7cf5eea91b5c5b07d9a36c836e4b645232c935e62e`。未在 Windows 运行该产物。

正常提交 hook 实际执行 **0 次**，完整 `cargo test` 实际执行 **0 次**。已检查 core.hooksPath（未设置）与公共 Git hooks 目录（仅 sample，无启用 hook），未修改或绕过 hook。按「实现会话检查约定」及本轮禁止提交的限制，完整套件覆盖缺口交编排方统一安排，不能把针对性测试冒充全量结果。

## Code review

固定点：`d3aa7edd61c5c36ff0fb29046df5227a7284312f`。本轮无提交；审查 `git diff d3aa7edd61c5c36ff0fb29046df5227a7284312f -- README.md src/cli/account.rs src/incremental.rs tests/incremental_cli.rs` 的未提交差异及相关调用链。

Standards：0 项阻断，0 项可选发现。联系人查询复用只读打开、参数绑定与既有错误映射；游标、持久化、代际检查保留；测试沿用 CLI JSON 接缝，无新依赖或额外抽象。

Spec：0 项发现。已知群无表分流、空批次重放、后建表从头分页、未知会话、初始缺目录/分片、README 均符合本票；CLI 直读不启动 daemon。无待修复审查项。

两轴审查均为静态审查，未把审查结论当运行验证。

## 改动文件与文案

| 文件 | 改动 |
|---|---|
| `src/cli/account.rs:92` | 精确已知群查询，复用账号只读联系人库 |
| `src/incremental.rs:103` | 初始无目录进入空分片流程；无表时检查已知群；合成单元 fixture 增加空联系人库 |
| `tests/incremental_cli.rs:40` | 复用双账号 CLI fixture，增加无表群、空批次重放、后建表分页、未知群、缺目录及首次分片场景 |
| `README.md:534` | 同步 incremental 空批次与 CHAT_NOT_FOUND 契约 |
| `docs/personal-wechat-read-v2-t04b-validation.md` | 本验收与交接记录 |

待定稿文案键：无。未新增界面、JSON 字段、错误码或错误文案；README 为契约说明。

## 需编排方／端到端执行

1. 在本票分支审查工作区，统一执行一次完整 `cargo test`，再按授权提交与合入。若配置了正常提交 hook，保留并复用它的完整检查结果。
2. 在具备工具链的环境补 Linux 与 Windows MSVC check；上述两项为宿主条件阻塞，不计本票产品缺陷。
3. 在 Windows 微信测试号复测本次 wx.exe：groups 中已知无表群返回空批次；原游标重放稳定；群首次入库后沿 next_cursor 从首条读取；未知群仍 CHAT_NOT_FOUND。真实库、密钥、设备均由编排方处理。
4. 在允许 ps 与监听 IPC 的环境执行 daemon 集成测试，记录 PID/端口并在结束后确认退出。本轮该项在启动前跳过。

未做／未验证：完整套件、Linux/MSVC 完成检查、Windows 运行态、真实微信数据库/密钥/进程、daemon 端到端；未提交、推送或合入。显式 next_cursor 与隐式检查点语义沿用原契约；调用方必须持续保存并回传新游标。
