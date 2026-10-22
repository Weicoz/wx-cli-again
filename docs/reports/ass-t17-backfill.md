# 售后统计票 17 第 1 段：wx-cli 按起点回读

本报告仅覆盖 wx-cli，其他仓库、部署与服务调度不在本轮范围。测试只使用合成 SQLite 库与进程探测替身，不连接真实微信、不发送消息。

## 分支与基线

- 仓库：wx-cli-again；分支：`ass/t17-wxcli`。
- 指定基线与起始 HEAD：`d7d42f5ca2bdcb0b7c3e058290641997c1e93456`。
- rd checkout 未提供本地 `main` 引用，未 fetch，无法判断远端 main 后续是否前移；无起始未提交改动，无冲突。
- 只在当前 worktree 修改；本轮具备 rd 提交授权，但跨平台检查未通过，按仓库规则未提交；不 push、不合并、不切换分支。

## 操作端接入

最终调用入口、参数、JSON 结构、错误码与重试边界见 [补读契约](../backfill.md)。

```sh
wx --wxid wxid_example --pid 1234 backfill '123@chatroom' --since 1700000000 --until 1702592000 --cursor '' --limit 200
```

首次可省略 cursor；续页保持账号、群和时间范围并原样传回 next_cursor。Unix 秒半开区间 `[since, until)`，done=true 后停止。响应顶层含 account_wxid、chat_wxid、messages、next_cursor、has_more、done；消息复用增量结构，svrid 是十进制字符串。补读游标携带用途及时间范围，独立于增量检查点。

契约样例（操作端后续原样复制至其 `tests/fixtures/wx-cli-contract/`）：

- `tests/fixtures/contract/backfill-first.json`
- `tests/fixtures/contract/backfill-next.json`
- `tests/fixtures/contract/backfill-done.json`

## 执行依据与接缝

完整读取 implement、tdd（含 tests.md、mocking.md）、weicoz-agent-implement 技能。实际安装的 weicoz-agent-implement/SKILL.md 没有名为「实现会话检查约定」的章节；采用其中「测试只跑该跑的」及本轮显式检查约定：先相关、后一次全量，默认串行，保留正常 hook。未启动嵌套代理。

预先约定接缝是用户指定的真实 CLI 合成库测试与逐字节契约测试，不测试内部私有方法。TDD 的失败、修复与回归日志保存在 `target/t17-evidence/`，不提交构建产物。

## 需编排方／端到端执行

- 在目标 Linux、Windows MSVC 工具链环境完成跨平台 check，运行相同合成库回归。
- 操作端按 backfillId 去重与停止、补读/增量串行轮次、增量节奏验证；将本仓样例原样复制到操作端。
- 三仓 reader-config、messages-report、reader-status、控制端 PostgreSQL 幂等入库与禁止触发模型链路联调。
- 若安排测试栈，由编排方部署并复测；本会话不部署、不写部署目录、不重启服务。

## 文案

无 UI 变更，无新增本地化文案键。CLI 帮助与错误 message 是可定稿占位文案；操作端按错误 code 对接。新增 code 为 TIME_RANGE_INVALID、CURSOR_RANGE_MISMATCH、CURSOR_KIND_MISMATCH；完整文字以源码和 docs/backfill.md 为准。


## 逐条验收证据

| 验收项 | 结果与证据 |
|---|---|
| 新增按起点回读子命令 | 已实现 `backfill`；README 与 docs/backfill.md 记录最终命令、参数、JSON、错误码 |
| 群 `[since, until)` 半开区间 | `backfill_reads_half_open_range_in_local_id_order_without_advancing_incremental` 用边界前、since、until、边界后和乱序时间验证；输出 local_id 顺序为 2、4、5，排除 local_id=3 的 until 记录 |
| 首页、续页、结束与 done | 同上验证 2 条/1 条/空页；`backfill_pages_match_contract_with_incremental_message_shapes` 验证 5 条首页、1 条续页和空结束页；`backfill_discovers_shards_and_rejects_lost_boundaries` 验证恰满 limit 的末页直接 done=true |
| 输出与增量消息结构相同 | 契约测试逐条比较补读首页 messages 与既有 incremental.json 的 messages；文本、本人、系统、64 位 svrid、mentions、quote、raw 均相同 |
| 复用查询、顺序、分片和代际 | `src/incremental.rs` 共享 read_page/open_shards/validate_position/verify_generations；没有复制消息解析器。多分片 CLI 测试验证 0/1/2 分片顺序、新分片发现、边界删除 CURSOR_OUT_OF_RANGE、已知分片消失 DB_ROTATED |
| 补读与增量独立 | 首次补读后 incremental.db 不存在；后续增量隐式读取先返回 local_id=1，补读后再增量返回 2；补读不访问增量状态库 |
| 两种游标误用明确报错 | `backfill_rejects_wrong_cursor_kind_range_and_invalid_bounds` 双向验证 CURSOR_KIND_MISMATCH；变更范围报 CURSOR_RANGE_MISMATCH；损坏游标报 CURSOR_INVALID |
| 非法范围、未知群、空群 | 同上验证相等/反向/负数/i64 最大时间范围报 TIME_RANGE_INVALID；未知群的 backfill stdout 与既有 error-chat-not-found.json 原样相等；已知空群 done=true、messages=[]、有效新游标 |
| 三份真实 CLI 样例、逐字节比较 | backfill-first.json / backfill-next.json / backfill-done.json 由 Fixture 合成库运行真实 wx 子进程直接写 stdout；golden 比较原始字节、末尾 LF；fixture-audit.log 记录长度及 SHA-256 |
| 原有契约与增量语义 | 原六份 fixture 对基线逐字节一致；全量包含原 17 项 incremental 单元测试、2 项 incremental CLI 和全部原契约测试 |
| wx-cli cargo test 通过 | 一次全量退出 0：179 单元 + 4 account + 10 contract + 2 incremental，harness 共 195 passed；其中 daemon 1 项预检跳过，实际通过 194 项 |
| 不连接真实微信、不发送消息 | 新测试仅使用 Fixture 临时合成 SQLite 与 pgrep/lsof 脚本替身；真实 CLI 子进程均断言无 daemon.pid；产品命令复用只读源连接 |
| 本票提交 | 未完成：两个跨平台 cargo check 因缺 target 失败，AGENTS.md 禁止失败时提交。当前只有未提交差异，无新增提交号 |

## TDD 与检查记录

所有 Cargo 命令串行执行，`-j 1`，测试 `--test-threads=1`。未改 Cargo.toml、Cargo.lock、检查配置或 hook。首次依赖构建由 rd cargo 包装器在任务内缓存；结束将其移入忽略的 target/rd-private，未提交依赖。

| 命令或阶段 | 结果 / 日志（target/t17-evidence/） |
|---|---|
| `cargo check -j 1` | Rust 编辑后立即检查共 8 次，均退出 0；check-test-1/2/3、check-impl-1/2、check-review-fix、check-formatted.log（formatted 首次在格式工具路径失败后记录，成功格式化后覆盖为最终结果） |
| 第一片：CLI 区间分页与状态独立 | red-range.log：退出 101，原 CLI 不认识 backfill；green-range.log：实现后退出 0，1 passed |
| 第二片：游标隔离与范围 | red-errors.log：退出 101，错误接受增量游标；green-errors.log：补齐校验后退出 0，1 passed |
| 第三片：契约样例 | red-contract.log：退出 101，缺 backfill-first.json；`UPDATE_CONTRACT_FIXTURES=1 cargo test -j 1 --test contract_cli backfill_pages_match_contract_with_incremental_message_shapes -- --exact --nocapture` 退出 0，generate-contract.log；原有 incremental.json 虽经更新路径输出，但字节完全一致 |
| `cargo test -j 1 --test contract_cli -- --test-threads=1 --nocapture` | contract-green.log：9 passed；审查补覆盖后 contract-final.log：10 passed；格式化后的最终覆盖由一次全量承担 |
| `cargo test -j 1 -- --test-threads=1 --nocapture` | 仅 1 次全量，退出 0；full-test.log。daemon SKIP 位于第 205 行 |
| `cargo clippy --all-targets -j 1` | 退出 0，clippy.log；bin 39 / bin test 51 条既有警告（含 39 条重复）；修改文件无警告 |
| `cargo check -j 1 --target x86_64-unknown-linux-gnu` | 退出 101；check-linux.log：E0463，缺 core / target 未安装，尚未检验到项目代码 |
| `cargo check -j 1 --target x86_64-pc-windows-msvc` | 退出 101；check-windows.log：E0463，缺 core / target 未安装，尚未检验到项目代码 |
| `git diff --check`、样例 JSON/LF/基线字节审计 | 通过；fixture-audit.log |

正常提交 hook 实际运行 **0 次**；完整测试运行 **1 次**。core.hooksPath 未设置，公共 hooks 目录只有 `.sample`，没有启用提交 hook；未修改、禁用或绕过 hook，也未尝试提交。跨平台缺口不能用 macOS 成功替代。

首次直接 `rustfmt` 失败（默认 rustup 未配置）；使用 rd 的 `RUSTUP_HOME=/Users/jy/remote-dispatch/users/weicoz/toolchains/rustup rustfmt --edition 2021 src/incremental.rs tests/contract_cli.rs` 成功，之后立即 cargo check 通过。初始资源：1 分钟负载 3.57、可用空间约 30 GiB；沙箱禁止 sysctl swap 与 top，无法报告换页和单进程内存；全过程无并发重检查。

## Code review：Standards

实现后完整读取 code-review/SKILL.md。按本轮禁止嵌套代理的要求由本会话分别执行两轴审查，不启动技能默认的并行子代理。固定点为 d7d42f5，审查命令 `git diff d7d42f5 -- src/cli/mod.rs src/incremental.rs tests/contract_cli.rs`，另读 README、新文档与三份新样例。因尚未提交，使用工作区 diff；`git log d7d42f5..HEAD --oneline` 为空。审查时差异已非空。

发现 1 项判断性 Data Clumps：backfill 函数携带独立 since、until 原始参数；已封装 TimeWindow 与其校验构造函数，减少参数，保持窗口绑定。未发现其余需要修改的命名、重复查询/解析或过度抽象问题。规则检查未通过项是两个目标工具链缺失，已保留日志并阻止提交。

## Code review：Spec

规格来源为本轮完整票据摘录与跨仓约定，仅审 wx-cli 条目。仓库缺 docs/agents/issue-tracker.md，本轮未初始化工作流或访问外部票据，以用户提供摘录为验收依据。

发现 1 项覆盖缺口：初稿未直接用 backfill CLI 验证新增分片、恰满末页、游标边界删除和分片消失。已增加 backfill_discovers_shards_and_rejects_lost_boundaries；并补空群、未知群回归断言。10 项相关测试和后续全量均通过。未发现未实现的 wx-cli 产品行为；跨平台编译、提交与外部联调仍未完成。

审查小计：Standards 1 项设计判断已处理；Spec 1 项覆盖缺口已处理；未解决事项为两平台编译环境和由此阻止的提交。

## 改动文件与交付状态

- src/cli/mod.rs：backfill 子命令、固定 JSON 错误出口与分发。
- src/incremental.rs：TimeWindow、独立用途/范围游标校验、共享 read_page、无状态 backfill。
- tests/contract_cli.rs：四个补读场景测试、未知/空群断言，保留既有字节比对机制。
- tests/fixtures/contract/backfill-first.json、backfill-next.json、backfill-done.json：真实 stdout。
- README.md、docs/backfill.md：接入契约。
- docs/reports/ass-t17-backfill.md：本报告。

详细日志另归档到本次 rd job 的 reports/t17-evidence/，构建目录不提交。未新增 UI、未修改其他仓库、未安装外部目录工具链、未部署。补读相同游标在源库变动后不保证字节重放，契约已明确；本票没有要求增量状态库的批次重放能力。

后续由编排方在具备 Linux/MSVC target 的环境完成两条跨平台 check，并在允许 ps/IPC 的环境补跑 daemon 测试；审查本票差异后以中文说明提交到 ass/t17-wxcli。无服务部署步骤；三仓联调按前述端到端清单执行。本会话结果为 partial，不以 DONE 或退出码替代验收。


## 第 2 轮续派：target 安装及跨平台复检

本轮结论：**blocked**。只补工具链安装尝试、两条串行跨平台检查和本节报告，无 Rust/测试/契约样例代码改动。未执行 git add/commit/push/merge，分支仍为 ass/t17-wxcli，HEAD 仍为 d7d42f5ca2bdcb0b7c3e058290641997c1e93456，无新增提交号。

执行安装命令（指定 rd 私有目录，未改系统或用户全局 rustup 配置、未更换源）：

```sh
RUSTUP_HOME=/Users/jy/remote-dispatch/users/weicoz/toolchains/rustup rustup target add x86_64-unknown-linux-gnu x86_64-pc-windows-msvc
```

退出码 **1**，完整原始输出（round2/target-add.log）：

```text
info: downloading 2 components
info: rolling back changes
error: component download failed for rust-std-x86_64-unknown-linux-gnu: error opening file for download: cleaning up cached downloads: No such file or directory (os error 2)
```

随后私有工具链的 `rustup target list --installed` 仍只有 aarch64-apple-darwin。未尝试其他源、全局配置或替代安装位置。按照续派要求继续串行执行两条检查，并复用任务内 target/rd-private/cargo-home 缓存（以 CARGO_HOME 指定），RUSTUP_HOME 保持上述 rd 私有目录。

| 命令 | 退出码与结果 | 原始日志 |
|---|---|---|
| `cargo check -j 1 --target x86_64-unknown-linux-gnu` | 101；E0463，can't find crate for core；libc 编译失败，target 未安装 | target/t17-evidence/round2/check-linux.log |
| `cargo check -j 1 --target x86_64-pc-windows-msvc` | 101；E0463，can't find crate for core；windows-link 编译失败，target 未安装 | target/t17-evidence/round2/check-windows.log |

本轮尚未到项目 Rust 或 C 依赖交叉编译器检查阶段，不能判断两者是否可通过；此次失败不能归因为本票 Rust 代码，也不能归因为缺 C 编译器。没有修改产品代码，未重跑相关测试或全量 cargo test；复用第 1 轮结果（全量仅执行过一次）。提交 hook 实际累计仍为 0 次，未绕过检查和提交门槛。

原始日志、退出码与完整待审补丁归档到本次 rd job 的 reports/t17-evidence/round2/。需编排方解决私有 rustup 下载/缓存文件错误后，再安装这两个 target，重跑两条检查；全部通过才可正常提交。当前报告及前轮实现继续保留在本票工作区，本轮最终状态覆盖前轮 partial 为 blocked。

## 编排方补检与提交（2026-10-22）

- m6 rd 私有工具链经普通 SSH 安装 `x86_64-unknown-linux-gnu`、`x86_64-pc-windows-msvc` target 成功（沙箱内安装失败）。两条交叉 check 仍退出 101，失败点为 C 依赖构建脚本：Linux 为 `zstd-sys`、`openssl-sys`，MSVC 为 `zstd-sys` 的 cc 交叉编译；未进入本仓 Rust 代码。与 t03、t04b、t04c 记录的宿主条件阻塞相同。
- 替代检查：编排方本机（具备 mingw-w64）在本工作区执行 `cargo check -j 2 --target x86_64-pc-windows-gnu`，退出 0；5 条警告均在未改动的 `src/scanner/mod.rs`。本票改动中的 `cfg(unix)` 分支在 Windows 目标下编译通过。
- 原生 `cargo check` 与一次全量 `cargo test`（194 项通过）沿用 m6 第 1 轮结果，改动内容由编排方逐文件原样取回，未再修改代码。
- 编排方审查源码 diff：补读复用 `read_page`，只加 `create_time` 半开窗口条件；不打开增量检查点库；游标携带用途与窗口，混用报错。未发现阻塞问题。
- 未验证：Linux 与 Windows MSVC 完整 check、Windows 真实运行。按用户授权不 push。
