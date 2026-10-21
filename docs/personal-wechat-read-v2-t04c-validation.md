# T04c：单群缺 chat_room 记录时保留群列表

已完成：实现、README 契约、合成 CLI 回归、两轴代码审查。已验证：相关测试、本机 check、clippy、Windows GNU release 构建。未验证：完整测试、Linux/MSVC 完成检查、Windows 真实运行及操作端适配。

日期：2026-10-21。仓库：wx-cli fork；工作区 `.worktrees/wxcli-dev`；分支 `prw2/t04c`。基线与当前 HEAD 均为 `e5457c02430e5a59500dc1a570fc150e6944f1d3`。`git rev-list --left-right --count HEAD...integration/personal-wechat-read-v2` 为 `0 0`；无基线落后或冲突。未执行 git add、commit、merge、push，未写其他分支或操作端仓库。

执行方式：本机 Codex；主会话独占写入与检查。完整读取用户指定 implement、tdd、code-review 技能，及 `weicoz-agent-implement/SKILL.md` 的「实现会话检查约定」。按 implement → TDD → code-review 执行；两个只读子代理分别审查 Standards 与 Spec，不运行测试、不写入。

## 原始失败路径

本票无独立票据文件，以用户「本票专属约定」为需求源。背景指针为操作端集成工作区 `research/windows-wx-cli-v2-test-2026-10-21.md`；具体缺记录现象由本票补充。

基线 `e5457c0`：`src/cli/mod.rs:490` → `cmd_groups` → `groups`。`src/cli/account.rs:157` 依次查询三种群名列；`:165` 用 `.ok()` 吞掉所有 SQL 错误；`:167-172` 把「无行」和读取失败一起改成 `VERSION_UNSUPPORTED`，经 `?` 中断整个列表。原始行号保存在 `target/t04c-evidence/baseline-error-path.log`。

当前实现：`src/cli/account.rs:163` 只在候选列不存在时继续兼容探测；`:184-187` 用 `OptionalExtension` 区分无行与读取错误。无记录的群输出空成员，`:220` 固定输出 `membersComplete`。`:179` 预先校验成员查询结构，避免把缺表/缺列伪装成数据尚未完整。`:85-97` 同时识别 rusqlite 的 `SqliteFailure` 与 `SqlInputError`；实际读取错误沿既有默认映射返回 `INTERNAL`。

## 逐条验收

| 验收项 | 状态与证据 |
|---|---|
| 定位原错误判定路径 | 已验证：上述基线行号；`red.log` 复现原 `VERSION_UNSUPPORTED` 与「群缺少 chat_room 记录，无法确认完整成员列表」 |
| 缺记录群保留 wxid、显示名 | 已验证：`tests/account_cli.rs:61,100-106`；缺记录群排在完整群前，断言列表成功、备注名保留、members=[]、membersComplete=false |
| 其余群成员与完整标记不受影响 | 已验证：`tests/account_cli.rs:99-106`；完整群仍有两个成员、群昵称、正确排序与 membersComplete=true |
| 记录补齐后恢复完整标记 | 已验证：`tests/account_cli.rs:108-125`；先补空群记录得到 true/0/[]，再补成员得到 true/1/本人；原完整群保持一致 |
| 保留版本兼容与空列表 | 已验证：`tests/account_cli.rs:36-39` 覆盖 username/chat_room_name/name 三种列名；`:127-129` 覆盖无群 [] |
| 真实结构错误仍报版本错误 | 已验证：`tests/account_cli.rs:190-199` 覆盖缺 chat_room 表、无受支持群名列、缺成员表、缺必需成员列，均为 VERSION_UNSUPPORTED |
| 数据读取错误使用正确类别 | 已验证：`tests/account_cli.rs:186-189` 非整数 room id 返回 INTERNAL；`:201-204` 损坏明文库返回 INTERNAL；既有登录、取钥、身份错误检查通过 |
| README 同步完整 JSON 契约 | 已完成：`README.md:456-471`；五个固定群字段、两个固定成员字段、计数与完整标记语义、错误边界均明确 |
| 合成数据与 daemon 生命周期 | 已验证：样本仅在临时目录；WX_CLI_HOME 与假的 pgrep/lsof 隔离真实微信；`:130` 断言无 daemon.pid。既有 daemon 测试在 ps 预检时跳过，未启动 daemon。两次红灯及一次旧缓存失败遗留的三个本次合成目录已核实并删除，见 cleanup.log |

## 操作端新契约

成功：退出码 0，stdout 顶层数组。群对象键集合固定为 `wxid`、`displayName`、`memberCount`、`members`、`membersComplete`；成员对象键集合固定为 `wxid`、`displayName`。字符串不变，memberCount 为非负整数，membersComplete 为必填布尔值。

```json
[
  {
    "wxid": "001@chatroom",
    "displayName": "新群备注",
    "memberCount": 0,
    "members": [],
    "membersComplete": false
  },
  {
    "wxid": "101@chatroom",
    "displayName": "客服群",
    "memberCount": 1,
    "members": [
      {"wxid": "wxid_a", "displayName": "群昵称甲"}
    ],
    "membersComplete": true
  }
]
```

`memberCount == members.length`。false 表示暂缺群记录，0 不代表真实成员总数。true 表示本地群记录存在且成员查询成功，空成员也可为 true。不扫描消息或猜测成员；没有 room id 时返回空数组。无群仍输出 `[]`。

失败：退出码 1，stdout 为 `{"error":{"code":"错误码","message":"错误说明"}}`。缺行不再触发错误；表/必需列不支持仍为 VERSION_UNSUPPORTED，实际数据库读取失败为 INTERNAL。既有账号、身份、密钥门禁保留。

**编排方必须同步操作端 parseGroups 的严格键集合**，并校验新增布尔字段；本票未改外仓。

## 检查命令与结果

日志目录：`target/t04c-evidence/`（忽略，不提交）。检查命令串行执行；普通 Cargo 命令使用 `-j 2`；Windows release 按用户指定使用 `-j 4`，未与其他检查并行。构建前可用空间约 11.6 GB，10 核、1 分钟负载 5.64，内存空闲比例 33%。未改 Cargo.toml、Cargo.lock 或 hook。

| 完整命令 | 结果与日志 |
|---|---|
| `cargo test -j 2 --test account_cli selected_profile_reads_its_own_identity_and_groups_without_messages -- --exact --nocapture` | 首轮红灯退出 101，原缺记录 VERSION_UNSUPPORTED：red.log；首次修复后第二次红灯暴露 SqlInputError 缺列未处理：red-sql-input.log；补齐错误分支后退出 0：green.log |
| `cargo check -j 2` | 已验证，5 次均退出 0；每次 Rust 编辑后立即执行。check-red-test.log、check-implementation.log、check-sql-input.log、check-regression-tests.log、check-formatted.log |
| `cargo test -j 2 --test account_cli -- --test-threads=1 --nocapture` | 已验证：3 项实际运行通过；1 项 daemon 预检跳过，harness 记 4 passed。account-cli.log 明确记录 `SKIP daemon IPC integration: sandbox denies ps: Operation not permitted (os error 1)`，不得算 daemon 验收 |
| `cargo test -j 2 --test incremental_cli -- --test-threads=1 --nocapture` | 首次失败为继承缓存内嵌旧 worktree 的 CARGO_BIN_EXE_wx 路径，启动程序时报 No such file or directory；incremental-cli-stale-cache.log。仅刷新 tests/incremental_cli.rs 的 mtime 强制重编，内容无差异；随后 2 项通过，incremental-cli.log |
| `cargo test -j 2 incremental::tests:: -- --test-threads=1` | 已验证，17 项通过；incremental-unit.log。含共享联系人判断、轮转与结构错误回归 |
| `cargo clippy --all-targets -j 2` | 已验证，退出 0；bin 39 / bin test 51 条既有警告（含 39 条重复），本票修改文件无警告；clippy.log |
| `cargo check --target x86_64-unknown-linux-gnu -j 2` | 阻塞，退出 101：`error[E0463]: can't find crate for core`，缺 Rust target；check-linux.log |
| `cargo check --target x86_64-pc-windows-msvc -j 2` | 阻塞，退出 101：打开 Cargo 缓存 windows_x86_64_msvc-0.48.5.crate 时 `Operation not permitted (os error 1)`；check-msvc.log |
| `rustfmt --edition 2021 --check src/cli/account.rs tests/account_cli.rs` | 首次指出格式差异；格式化后已验证，退出 0；rustfmt-check.log、rustfmt-final.log。open_database 两处仅格式化，无行为变化 |
| `git diff --check` | 已验证，退出 0，无空白错误；diff-check.log |

Windows GNU 构建完整命令：

```bash
CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar cargo build --release --target x86_64-pc-windows-gnu -j 4
```

已验证：退出 0，`Finished release profile [optimized] target(s) in 50.67s`，5 条既有 Windows dead-code 警告；windows-gnu-build.log。产物 `target/x86_64-pc-windows-gnu/release/wx.exe`，11823616 bytes，PE32+ console x86-64 / MS Windows。SHA-256：`fc70ba2420c0fe33551542ef7beebfe033a7fab29df16b32f9cde9344092435e`；windows-artifact.log。未在 Windows 运行该产物。

正常提交 hook 实际执行 **0 次**，完整 `cargo test` 实际执行 **0 次**。core.hooksPath 未设置；公共 Git hooks 目录无启用的 pre-commit/prepare-commit-msg/commit-msg/post-commit。未删除、修改或绕过 hook。按「实现会话检查约定」与本轮禁止提交限制，完整套件交编排方统一执行，不能以针对性测试代替。

## Code review

固定点：`e5457c02430e5a59500dc1a570fc150e6944f1d3`。`git log e5457c02430e5a59500dc1a570fc150e6944f1d3..HEAD --oneline` 为空。使用 `git diff e5457c02430e5a59500dc1a570fc150e6944f1d3 -- README.md src/cli/account.rs tests/account_cli.rs` 审查工作区；三点分支 diff 无法包含未提交改动，因此未用它代替。

### Standards

0 项发现。缺列回退与读取错误分开处理；五字段契约、完整性语义一致；合成 SQLite 测试覆盖缺记录、其他群、恢复、三种列名与错误分类；新增路径不启动 daemon。调用方 `src/cli/mod.rs:490` 与错误出口保持兼容；无需处理的 smell。审查后仅执行 rustfmt，主会话复核格式化差异无行为变化。

### Spec

0 项发现，无需求遗漏或范围扩张。缺记录群输出 false/空成员，完整群保持；实际读取错误上抛，结构错误归 VERSION_UNSUPPORTED；README 与合成测试覆盖本票要求。外仓 parseGroups 与 Windows 真实端到端交编排方完成。

Standards 0、Spec 0；无待修复审查项。两轴均为静态审查，运行验证另以上述检查为准。

## 改动文件与待定稿文案

| 文件 | 改动 |
|---|---|
| `src/cli/account.rs:85,163,220` | 正确识别结构错误、区分缺行与读取错误、输出成员完整标记 |
| `tests/account_cli.rs:36,100,186` | 复用现有 CLI 合成库，补缺记录/恢复/列兼容/错误分类场景 |
| `README.md:456` | 更新 groups 的固定 JSON 形状、完整性与错误语义 |
| `docs/personal-wechat-read-v2-t04c-validation.md` | 本验收及交接记录 |

待定稿界面文案键与占位文字：无。新增 API 字段仅 `membersComplete`，类型 boolean，必填；错误文案沿用现有文本。README 为契约说明，操作端呈现由编排方定稿。

## 需编排方／端到端执行

1. 审查本票工作区，统一跑一次完整 `cargo test`；保留正常提交 hook，记录其实际执行次数。随后按编排方授权在本票分支提交、合入；本会话无新提交号。
2. 同步操作端 parseGroups 的五字段严格校验与布尔类型校验，验证完整群、缺记录群、补齐后恢复和空列表。
3. 在具备工具链及缓存写权限的环境补 Linux、Windows MSVC check。两项为宿主条件阻塞，不计产品缺陷。
4. Windows 真实测试号用上述 wx.exe 复测新建群：短暂缺记录时整个列表成功，该群 false；写全后恢复 true，其余群不变。真实微信库、密钥与设备由编排方处理。
5. 在允许 ps/IPC 监听的环境补 daemon 集成测试，记录启动 PID/端口，结束后确认本次 daemon 退出。本轮该项在启动前跳过。

未做／未验证：完整套件、Linux/MSVC 完成检查、Windows 真实运行、操作端适配、daemon 端到端；未访问真实微信数据库或密钥；未提交、推送、合入或部署。契约新增必填字段，操作端严格解析器同步前不得将其视为集成验收通过。
