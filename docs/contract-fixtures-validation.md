# wx-cli 契约 golden：验收与交接

已完成：六份真实 stdout 样例、六项 CLI 契约测试、更新入口、README 契约说明及两轴审查返修。已验证：macOS 合成库路径、字节漂移检测、针对性与全量测试、clippy。未验证：Linux/MSVC 完整检查、daemon IPC、操作端集成。

日期：2026-10-22（Asia/Singapore）。执行方式：本机 Codex；主会话唯一写入、串行检查；两个原生子代理只读审查，不运行测试。

## 分支与范围

- 仓库：wx-cli fork，工作区 `.worktrees/wxcli-dev`。
- 分支：`chore/contract-fixtures`。
- 基线和当前 HEAD：`70c59eceffb455f46279b4565f48925a5ca61232`。
- `git rev-list --left-right --count HEAD...integration/personal-wechat-read-v2` 输出 `0 0`；无基线落后或冲突。
- 未执行 git add、commit、merge、push；无新提交号。文件留在本票工作区，新增文件待编排方纳入提交。
- 未改产品源码、Cargo.toml、Cargo.lock、hook 或其他仓库。

完整读取 `~/.agents/skills/implement/SKILL.md`、`~/.agents/skills/tdd/SKILL.md` 及附属 tests/mocking 指引；实现后完整读取 `~/.agents/skills/code-review/SKILL.md`。同时读取指定 `weicoz-agent-implement/SKILL.md` 的「实现会话检查约定」。本票用户指令禁止提交，覆盖 implement 的默认提交步骤。

## 逐条验收

| 验收项 | 状态及证据 |
|---|---|
| 身份样例 | 已验证：`tests/fixtures/contract/whoami.json:1`；`tests/contract_cli.rs:218` 从真实 whoami stdout 生成并比较，合成账号及显示名有独立断言 |
| 群列表包含不完整群 | 已验证：`tests/fixtures/contract/groups.json:1`；`tests/contract_cli.rs:226` 同时断言完整群 true/2 与缺 chat_room 记录群 false/0/[] |
| 普通文本、mentions、本人消息 | 已验证：`tests/fixtures/contract/incremental.json:1`；`tests/contract_cli.rs:271` 断言文本、64 位字符串 ID、`notify@all` 和 `is_self=true` |
| 引用发送者有值与 null | 已验证：同一增量样例有两条 type 49；`tests/contract_cli.rs:286` 断言 quote 完整对象，键为 `message_id` / `sender_wxid` |
| 系统消息无发送者 | 已验证：`tests/contract_cli.rs:294` 断言 type 10000、sender_wxid=null、is_self=null |
| has_more=true | 已验证：合成库 6 条，limit=5；`tests/contract_cli.rs:300` 断言 true，样例含前五条 |
| 已知群无消息表空批次 | 已验证：`tests/fixtures/contract/incremental-empty.json:1`；`tests/contract_cli.rs:258` 构造已知群但无 message 目录，断言 []/false/非空游标 |
| 未知会话错误 | 已验证：`tests/fixtures/contract/error-chat-not-found.json:1`；`tests/contract_cli.rs:241` 真实 incremental，退出码 1，code=CHAT_NOT_FOUND |
| 登录错误 | 已验证：`tests/fixtures/contract/error-login-required.json:1`；`tests/contract_cli.rs:249` 保留合成库、撤销假 lsof 占用，退出码 1，code=LOGIN_REQUIRED |
| stdout 原始字节、格式、末尾换行 | 已验证：`tests/contract_cli.rs:189` 直接写 `output.stdout`，不序列化或替换字段；`fixture-audit.log` 核对六份 JSON 与末尾 LF。whoami/groups 为 CLI 美化输出，incremental/错误为 CLI 紧凑输出 |
| 不一致失败并给差异 | 已验证：`tests/contract_cli.rs:205` 输出首个差异字节、长度和双方内容；`byte-drift.log:8` 仅删末尾 LF 即退出 101：`differs at byte 67 (expected 67 bytes, actual 68 bytes)`；随后原字节已恢复 |
| 更新方式及稳定生成 | 已验证：`tests/contract_cli.rs:195` 支持 UPDATE_CONTRACT_FIXTURES=1；`update-reproducible.log` 六项通过，重新构造库并生成的六份字节与此前全部相同 |
| 固定时间与游标 | 已验证：`tests/contract_cli.rs:116` 固定消息秒数 1700000000；`:130` 用 macOS fsetattrlist 固定合成库 birthtime，读取 metadata 断言相同；`fixture-audit.log` 核对游标中的固定代际。产品逻辑未变、stdout 未归一化 |
| 合成数据与本机隔离 | 已验证：`tests/contract_cli.rs:8` 等仅含合成账号、群名、消息；`:162` 明确 WX_CLI_HOME、账号/PID，移除 WX_* 继承选择，PATH 替代 pgrep/lsof；明文库不需要密钥，未读取真实库或密钥 |
| 清理与 daemon | 已验证：`:178` 断言未生成 daemon.pid；`:183` Drop 清理，断言失败路径亦执行。fixture-audit.log 记录 wx-contract-* 临时目录剩余 0。新增路径不启动 daemon；既有 daemon 用例在 ps 预检跳过，未启动进程 |
| README 与通知义务 | 已完成：`README.md:489` 写明“样例为契约的一部分，改输出必须同步更新样例并通知操作端”，列出六份文件、验证/更新命令与平台边界。本票未改变产品输出，不发送外部消息 |

## TDD 证据

接缝由票据预先约定：真实 CLI 子进程的 stdout；进程发现以本地脚本替代，SQLite 与 CLI 均真实执行。不使用真实微信库、内存取钥或设备。

逐片执行 whoami → groups → CHAT_NOT_FOUND → LOGIN_REQUIRED → empty → incremental。每片先写测试，运行时因对应 golden 缺失退出 101；再由 UPDATE_CONTRACT_FIXTURES=1 写入真实 stdout，关闭更新后字节比较通过。六次红灯均为缺文件，不是产品回归。

日志位于 `target/contract-evidence/`（忽略目录，不纳入提交）：

- 红灯：`red-whoami.log`、`red-groups.log`、`red-missing-chat.log`、`red-login.log`、`red-empty.log`、`red-incremental.log`。
- 生成：对应 `update-*.log`；全部样例重建稳定性：`update-reproducible.log`。
- 绿灯：对应 `green-*.log`；最终代码统一验证：`related-final.log`、`full-test.log`。
- 负向字节检查：`byte-drift.log`，故意删除 LF，finally 恢复文件；最终全量验证读取已恢复样例。

单片命令格式（实际测试名见测试文件及日志；均串行）：

```bash
cargo test -j 1 --test contract_cli <测试名> -- --exact --nocapture
UPDATE_CONTRACT_FIXTURES=1 cargo test -j 1 --test contract_cli <测试名> -- --exact --nocapture
```

## 检查命令与结果

| 完整命令 | 结果 |
|---|---|
| `cargo check -j 1` | 已验证：Rust 编辑/格式化后共 9 次，均退出 0；`check-whoami/groups/missing-chat/login/empty/incremental/formatted/review-fix/final.log` |
| `cargo test -j 1 --test contract_cli --test account_cli --test incremental_cli -- --test-threads=1 --nocapture` | 已验证：退出 0；harness 12 passed，实际执行通过 11 项，daemon 一项在预检跳过；`related-final.log` |
| `cargo test -j 1 -- --test-threads=1 --nocapture` | 已验证：退出 0；179 单元、4 account、6 contract、2 incremental，harness 合计 191 passed；实际执行通过 190 项，daemon 一项预检跳过；`full-test.log:199,204,210,222,230` |
| `cargo clippy --all-targets -j 1` | 已验证：退出 0；bin 39 / bin test 51 条既有警告（含 39 重复）；新增 contract_cli 无警告；`clippy.log` |
| `cargo check --target x86_64-unknown-linux-gnu -j 1` | 阻塞：退出 101，`error[E0463]: can't find crate for core`；未安装该 Rust target；`check-linux.log` |
| `cargo check --target x86_64-pc-windows-msvc -j 1` | 阻塞：退出 101，打开 Cargo 全局缓存 windows_x86_64_msvc-0.48.5.crate 时 `Operation not permitted (os error 1)`；`check-msvc.log` |
| `rustfmt --edition 2021 --check tests/contract_cli.rs` | 已验证：退出 0 |
| `git diff --check` | 已验证：退出 0 |

正常提交 hook 实际执行 **0 次**；完整 cargo test 实际执行 **1 次**。本票未获提交授权，且 core.hooksPath 未设置，公共 hooks 目录只有 sample，无启用提交 hook。按本票显式安排的“cargo test（先相关后全量）”手动执行一次全量，不重复触发提交；未改、删或绕过 hook。覆盖缺口为 daemon 的 ps/IPC 路径，不把 harness 的 passed 视为该路径验收。

## Code review

固定点：`70c59eceffb455f46279b4565f48925a5ca61232`。提交列表 `git log 70c59ec..HEAD --oneline` 为空。三点 diff 不含本票未提交及新增文件；实际审查 `git diff 70c59ec -- README.md`，并完整读取新增测试及六份 JSON。标准源为 AGENTS.md、README 契约、现有 CLI 测试；未发现其他 CONTRIBUTING、CODING_STANDARDS、GLOSSARY 或 ADR。

### Standards

1 项 P2，已完成返修并静态复核：原 pgrep 替身对所有名称返回 4242；Linux 的 `src/process.rs:91` 分别查 wechat/weixin，形成两个候选，经 `select_main_process` 返回 INTERNAL，使四项测试失败。`:26` 改为只命中 WeChat/wechat，weixin 退出 1，单一候选成立。未改产品进程选择逻辑；Linux 实跑未验证。

主会话补充：`:18` 先创建目录成功，再构造含 Drop 的 Fixture；避免创建失败时清理非本次创建的既有目录。审查复核确认该顺序正确。

### Spec

1 项 P1，与 Standards 指向同一 Linux 替身缺陷；已完成相同返修并静态复核。其余六份样例、场景、真实 stdout、固定时间、隔离、清理与更新契约齐全，无范围扩大。票据未要求所有平台生成；非空增量测试限 macOS，README 已明示。

Standards 原发现 1、剩余 0；Spec 原发现 1、剩余 0。两轴独立静态复核；最终代码的运行证据为上述针对性与全量日志。

## 改动文件与文案

- `tests/contract_cli.rs`：合成 SQLite、命令隔离、六场景、原字节比较与更新、临时库清理。
- `tests/fixtures/contract/whoami.json`。
- `tests/fixtures/contract/groups.json`。
- `tests/fixtures/contract/incremental.json`。
- `tests/fixtures/contract/incremental-empty.json`。
- `tests/fixtures/contract/error-chat-not-found.json`。
- `tests/fixtures/contract/error-login-required.json`。
- `README.md`：契约标准样例、命令与平台边界。
- `docs/contract-fixtures-validation.md`：本验收记录。

待定稿界面文案键与占位文字：无。合成显示名与消息只是测试数据；未新增产品错误文案或操作端 UI。

## 需编排方／端到端执行

1. 审查工作区，纳入新增文件后在本票分支提交；本会话无提交、推送、合入。
2. 在工具链及缓存权限完整的环境补 Linux/MSVC check；Linux 五项 golden 测试未实跑，Windows 此测试文件不执行。
3. 在允许 ps/IPC 的环境运行 `cargo test --test account_cli captured_cli_returns_while_two_account_daemons_keep_running -- --exact --nocapture`，记录测试启动 PID/监听 socket，结束核对进程退出。该环境缺口不计本票产品失败。
4. 操作端直接消费六份样例，运行其契约测试及实际 Windows 端到端；本票未操作外仓或真实设备。

未做／未验证：Git 写操作、外部消息发送、真实微信数据与密钥访问、Windows 真实运行、操作端集成；Linux/MSVC 完整检查及 daemon IPC 仍未验证。非空增量 golden 生成要求 macOS 支持设置文件创建时间。
