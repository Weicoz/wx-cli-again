# 客户图片票 08：取钥轮询与失败阶段码

基线为 `c40e3e2`（票据指定的 `ticket/customer-images-01` 提交）；交付分支为 `ticket/customer-images-08-key-harvest`。仅修改本 wx-cli-again worktree，不 push，不接触真实微信、客户图片、控制端或其他仓库。未加依赖，Cargo.toml / Cargo.lock 未修改。

## 改动

- `src/cli/mod.rs`：新增 `image key-harvest` 和 `resolve --wait-key-seconds`。
- `src/attachment/image_key/windows.rs`：唯一数据库持有主进程的有界扫描，RO/ER 页与 65 字节重叠；复用现有进程选择规则，不枚举子进程。
- `src/image/harvest.rs`：可注入 Scanner / Clock、三类候选与窗口统计、跨轮哈希去重、脱敏报告。
- `src/image/key_cache.rs`：每轮账号锁、轮询、新候选逐个完整验证、仅成功后写 DPAPI 历史缓存；缓存复用不重写，同一 resolve 的资源候选共享精确等待截止时间。
- `src/image/mod.rs`、`src/image/codec.rs`：共用消息/账号检查，失败阶段码；WXGF 转码可注入合成替身。
- `src/image/harvest_tests.rs`、`tests/image_cli.rs`：合成缓冲区、运行时 PNG、时钟/扫描/安全存储替身及真实 CLI 检查。
- `docs/image-resolve.md`：命令、输出字段、运维操作、真机待验证项。

## 检查记录

以下均为本轮实际执行命令，完整 stdout/stderr 随 Remote Dispatch artifact 的 `image08-evidence/` 提供。未覆盖的检查不视作通过。

| 命令 | 结果 |
| --- | --- |
| `cargo check` | 通过，exit 0；每批 Rust 修改后均执行，最终结果通过 |
| `cargo test` | exit 0；harness 报告 233 passed、0 failed、1 ignored；其中既有 daemon IPC 测试内部 SKIP，实际执行通过 232 项 |
| `cargo test image:: -- --test-threads=1` | 开发期图片单元测试通过；最终包含新增阶段/精确时间窗口检查的完整版本由上述 cargo test 覆盖 |
| `cargo test --test account_cli captured_cli_returns_while_two_account_daemons_keep_running -- --nocapture` | exit 0，但输出 `SKIP daemon IPC integration: sandbox denies ps: Operation not permitted (os error 1)`；不算 IPC 实际通过 |
| `cargo clippy -- -D warnings` | exit 101，39 项既有生产代码告警升为错误；本轮新增代码无告警 |
| `CARGO_TARGET_DIR="$PWD/target" cargo clippy --manifest-path target/image08-baseline/Cargo.toml -- -D warnings` | 基线快照同样 exit 101、39 项错误；快照由 `git archive c40e3e2` 在当前 worktree 的忽略目录中生成 |
| `cargo clippy --all-targets`（仓库原有检查） | exit 0；生产代码 39 项告警，测试目标共 51 项（含 39 项重复），均为既有代码告警 |
| `cargo check --target x86_64-pc-windows-gnu` | **未验证**：exit 101，Rust GNU Windows target 未安装，报 E0463 缺 core |
| `cargo check --target x86_64-pc-windows-msvc` | **未验证**：exit 101，原生依赖编译缺 Windows SDK/C 头文件（stdlib.h / string.h）及相应工具链 |
| `cargo check --target x86_64-unknown-linux-gnu` | **未验证**：exit 101，缺 x86_64-linux-gnu-gcc，OpenSSL / zstd 原生依赖构建失败 |
| `git diff --check` | 通过 |

保留严格 clippy、三个跨平台目标和一次开发期 Duration 参数类型不匹配的失败日志；后者已修复，最终 cargo check / cargo test 通过。提交按普通 git commit 执行，不关闭 hook；仓库 hooks 目录只有 sample 文件。

## 合成覆盖与边界

合成测试覆盖第 k 轮才出现候选、精确超时/剩余毫秒预算、锁争用与重试、ASCII32/ASCII16/UTF-16LE32、奇数偏移、重叠/跨轮去重、窗口不进入反验、首块 BMP 短魔数误报后继续至可完整解码候选、所有命中失败不写缓存、成功只写一次且跨安全存储重开复用。候选与图片均在测试运行时生成，无真实密钥/身份/聊天正文夹具；只检查数字和阶段码。

阶段测试覆盖缺 AES 首块（aes_head）、AES 体/长度不符（aes_body）、PKCS7、XOR 尾部、格式识别、WXGF 转码替身失败、标准图片解码及旧 decoder 未细分异常（other）。未命中的候选不会被误报为完整资源解码失败。

本票**未在 Windows 真机验证**，未运行原生 DPAPI/ACL 或真实进程内存扫描。待现场验证：开图时轮询取钥、缓存关图复用、不同 CLI 进程及微信重启后的缓存复用（跨进程重启复用）、真实 DPAPI/ACL/账号隔离/锁争用和真实 V2/WXGF 完整解码。既有真实 WXGF 测试需外部生成 HEVC 与随包 codec，本轮未运行，保持原有 ignore。

## 契约影响

`resolve` 默认 `--wait-key-seconds=0`，保留至多一次 3 秒扫描、既有成功字段、退出码和错误码。新 stage 仅为失败时可选增补字段，六份既有 golden 样例不变；按字段读取的消费者无破坏性变化，严格拒绝未知字段的失败解析器需要允许 stage。非零等待显式延长请求时间，控制端若使用此参数应自行设置相应进程预算，本票不修改控制端。

原需求同时要求 key-harvest 仅五字段及超时错误码/retryable。本轮采用已提出的兼容解释：成功严格五字段；失败增补 errorCode / retryable，适用时再增补 stage。超时 `IMAGE_KEY_HARVEST_TIMEOUT`、retryable=true，不输出候选、哈希、身份、路径或异常正文。此解释需由发起方验收确认，不影响既有 resolve 成功契约。
