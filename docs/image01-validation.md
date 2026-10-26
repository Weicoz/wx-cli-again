# 客户图片票 01 实现回报

基线：`0c470bb`。本轮只改当前 wx-cli-again worktree，不切分支、不 push、不发送微信消息、不连接或修改 zero-s-pc，不修改 ai-wechat-action 或控制端。未提交 `.scratch/wip/`、依赖缓存、明文图片、密钥或客户正文。版本号保留 0.6.3，以本票 Git 提交锁定源码版本。

## 实现与逐项验收

| 票据项 | 开发验证与边界 |
| --- | --- |
| image resolve / JSON v1 / exit 0、2 | 已实现，真实 CLI 子进程验证，参数和进程/数据库故障单独处理；ACCOUNT_MISMATCH 不可重试，操作端须停账号任务 |
| 精确账号、群、长字符串服务端 ID 与资源关联 | 已验证合成库路径；不使用昵称或 local_id 最新行回退，跨分片重复消息保守拒绝 |
| 比较成功解码的真实像素面积、大图损坏回退 | 已验证；同面积按 full_path、hd、bubble、thumbnail；Bubble 无确定映射故排除，未用文件大小冒充尺寸 |
| DPAPI CurrentUser + ACL / 账号与目录隔离 / 历史版本与一次重扫 | 已实现 Windows 路径；非 Windows 用安全存储替身验证用户不符、重开进程、作用域隔离、缓存目标反验、一次重扫及最多四个版本。MSVC 目标连同专属测试编译通过；原生 DPAPI/ACL/进程内存取钥未运行 |
| 截断、不支持、key 不符分开处理 | 已验证 V2 段不足不扫描，已验证头的体损坏不重扫，未知格式/转码失败有单独码 |
| WXGF 拆封、真实转码与标准图校验 | 已用运行时生成的黑色 16×16 HEVC 验证真实完整切片，非转码替身；输出重新真实解码并核对 PNG 尺寸/大小/SHA-256。多分区真实动画/遮罩与实际微信格式覆盖未验证，不能承诺全格式支持 |
| 锁版解码依赖随交付携带 | 已构建 macOS 完整 release 包，含 FFmpeg 8.0.1、校验 manifest、完整对应源代码、LGPL 许可；对该包执行全部 13 项图片 CLI 测试。Windows/Linux 各架构 Release workflow 已接入打包，未触发 CI 或发布 |
| 图片失败不停止文本 / 旧命令兼容 | 新路径不触发 daemon、不改 incremental 状态；现有 contract/account/incremental 回归通过。既有 daemon IPC 验收因沙箱 ps 权限被内部跳过，不算实际执行通过 |
| 六类合成契约样例与 README | 已完成，详见下表；外仓同步由编排方完成 |
| 无界面/UIA | 已遵守 |

输出字段完整对齐 PRD：`contractVersion/accountWxid/chatId/messageId/status/resourceKind/outputPath/mimeType/width/height/byteLength/sha256/convertedFrom/errorCode/retryable`。ready 的文件字段真实验证；unavailable 的资源输出字段为 null。六份样例仅归一化随机绝对输出路径，其他字段来自实际 CLI，不含图片字节。

| 样例路径 | 含义 |
| --- | --- |
| tests/fixtures/contract/image-regular.json | 普通 PNG，按面积选择 hd |
| tests/fixtures/contract/image-thumbnail-fallback.json | 损坏大图回退 thumbnail |
| tests/fixtures/contract/image-wxgf.json | 真正拆封并转码的 WXGF 标准 PNG |
| tests/fixtures/contract/image-key-unavailable.json | IMAGE_KEY_UNAVAILABLE，retryable=false |
| tests/fixtures/contract/image-resource-missing.json | LOCAL_RESOURCE_MISSING，retryable=true |
| tests/fixtures/contract/image-account-mismatch.json | ACCOUNT_MISMATCH，retryable=false |

编排方逐字同步到操作端 `tests/fixtures/wx-cli-contract/`。详细命令、错误码表、依赖清单、运行边界与现场步骤见 `docs/image-resolve.md`。

## TDD 证据

先写六项 CLI 行为测试，实际运行因不存在 image 命令失败（`red-cli.log`）；key provider 以返回不可用的初始接缝运行，四项行为失败（`red-key-behavior.log`）；WXGF 边界解析初始接缝的一项行为失败（`red-codec.log`）；超时分类先复现 TRANSCODE_FAILED 与 TIMEOUT 不符（`red-timeout.log`），再修复；跨分片 local_id/时间对应另一服务端 ID 的误关联也先复现退出 0（`red-collision.log`），再实现歧义拒绝。另保留初次测试编译错误、macOS /var 与 /private/var 规范路径断言差异、Python 缺 tar 解包 filter 参数等开发失败原始证据。后续修复及回归日志均保留，没有把红灯当验收成功。

## 检查原文

以下代码块为命令和输出原文；全量 test/clippy 仅截取结果行，其余完整输出随证据包提供。测试 harness 共报告 218 passed；其中既有 daemon IPC 用例在内部 SKIP，实际执行通过 217 项，不能据 passed 数量宣称 daemon IPC 已验证。唯一新增 ignore（真实 HEVC 用例）已用 `--include-ignored` 实际运行。

```text
$ cargo check
    Checking wx-cli v0.6.3 (/Users/jy/remote-dispatch/users/weicoz/wt/weicoz-wx-cli-again-customer-images-01-20261026-124212-316162000)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.52s
```

```text
$ CC_x86_64_unknown_linux_gnu="$PWD/target/img01/tooling/linux-cc" AR_x86_64_unknown_linux_gnu="$PWD/target/img01/tooling/zig-ar" ZIG_GLOBAL_CACHE_DIR="$PWD/target/img01/zig-global" ZIG_LOCAL_CACHE_DIR="$PWD/target/img01/zig-local" cargo check --target x86_64-unknown-linux-gnu
    Checking wx-cli v0.6.3 (/Users/jy/remote-dispatch/users/weicoz/wt/weicoz-wx-cli-again-customer-images-01-20261026-124212-316162000)
warning: unused import: `bail`
 --> src/scanner/linux.rs:6:14
  |
6 | use anyhow::{bail, Context, Result};
  |              ^^^^
  |
  = note: `#[warn(unused_imports)]` (part of `#[warn(unused)]`) on by default

warning: field `known` is never read
  --> src/scanner/mod.rs:39:9
   |
33 | pub struct ScanOptions<'a> {
   |            ----------- field in this struct
...
39 |     pub known: &'a [KeyEntry],
   |         ^^^^^
   |
   = note: `ScanOptions` has derived impls for the traits `Clone` and `Debug`, but these are intentionally ignored during dead code analysis
   = note: `#[warn(dead_code)]` (part of `#[warn(unused)]`) on by default

warning: function `decode_salt_hex` is never used
   --> src/scanner/mod.rs:293:15
    |
293 | pub(crate) fn decode_salt_hex(value: &str) -> Option<[u8; 16]> {
    |               ^^^^^^^^^^^^^^^

warning: function `collect_salt_adjacent_keys` is never used
   --> src/scanner/mod.rs:384:15
    |
384 | pub(crate) fn collect_salt_adjacent_keys(
    |               ^^^^^^^^^^^^^^^^^^^^^^^^^^

warning: function `find_slice` is never used
   --> src/scanner/mod.rs:422:4
    |
422 | fn find_slice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    |    ^^^^^^^^^^

warning: function `push_raw_key` is never used
   --> src/scanner/mod.rs:428:4
    |
428 | fn push_raw_key(
    |    ^^^^^^^^^^^^

warning: `wx-cli` (bin "wx") generated 6 warnings (run `cargo fix --bin "wx" -p wx-cli` to apply 1 suggestion)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.60s
```

```text
$ python3 target/img01/check-windows.py  # 调用 cargo check --target x86_64-pc-windows-msvc
warning: field `known` is never read
  --> src/scanner/mod.rs:39:9
   |
33 | pub struct ScanOptions<'a> {
   |            ----------- field in this struct
...
39 |     pub known: &'a [KeyEntry],
   |         ^^^^^
   |
   = note: `ScanOptions` has derived impls for the traits `Clone` and `Debug`, but these are intentionally ignored during dead code analysis
   = note: `#[warn(dead_code)]` (part of `#[warn(unused)]`) on by default

warning: function `decode_salt_hex` is never used
   --> src/scanner/mod.rs:293:15
    |
293 | pub(crate) fn decode_salt_hex(value: &str) -> Option<[u8; 16]> {
    |               ^^^^^^^^^^^^^^^

warning: function `collect_salt_adjacent_keys` is never used
   --> src/scanner/mod.rs:384:15
    |
384 | pub(crate) fn collect_salt_adjacent_keys(
    |               ^^^^^^^^^^^^^^^^^^^^^^^^^^

warning: function `find_slice` is never used
   --> src/scanner/mod.rs:422:4
    |
422 | fn find_slice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    |    ^^^^^^^^^^

warning: function `push_raw_key` is never used
   --> src/scanner/mod.rs:428:4
    |
428 | fn push_raw_key(
    |    ^^^^^^^^^^^^

warning: `wx-cli` (bin "wx") generated 5 warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.14s
```

```text
$ python3 target/img01/check-windows.py --all-targets
    Checking wx-cli v0.6.3 (/Users/jy/remote-dispatch/users/weicoz/wt/weicoz-wx-cli-again-customer-images-01-20261026-124212-316162000)
warning: field `known` is never read
  --> src/scanner/mod.rs:39:9
   |
33 | pub struct ScanOptions<'a> {
   |            ----------- field in this struct
...
39 |     pub known: &'a [KeyEntry],
   |         ^^^^^
   |
   = note: `ScanOptions` has derived impls for the traits `Clone` and `Debug`, but these are intentionally ignored during dead code analysis
   = note: `#[warn(dead_code)]` (part of `#[warn(unused)]`) on by default

warning: function `decode_salt_hex` is never used
   --> src/scanner/mod.rs:293:15
    |
293 | pub(crate) fn decode_salt_hex(value: &str) -> Option<[u8; 16]> {
    |               ^^^^^^^^^^^^^^^

warning: function `collect_salt_adjacent_keys` is never used
   --> src/scanner/mod.rs:384:15
    |
384 | pub(crate) fn collect_salt_adjacent_keys(
    |               ^^^^^^^^^^^^^^^^^^^^^^^^^^

warning: function `find_slice` is never used
   --> src/scanner/mod.rs:422:4
    |
422 | fn find_slice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    |    ^^^^^^^^^^

warning: function `push_raw_key` is never used
   --> src/scanner/mod.rs:428:4
    |
428 | fn push_raw_key(
    |    ^^^^^^^^^^^^

warning: `wx-cli` (bin "wx") generated 5 warnings (2 duplicates)
warning: `wx-cli` (bin "wx" test) generated 2 warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.92s
```

```text
$ WX_TEST_HEVC="$PWD/target/img01/synthetic.h265" cargo test -- --test-threads=1 --include-ignored
    Finished `test` profile [unoptimized + debuginfo] target(s) in 1.68s
running 189 tests
test result: ok. 189 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.34s
running 4 tests
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.00s
running 10 tests
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.37s
running 13 tests
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.75s
running 2 tests
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.28s
```

```text
$ WX_TEST_BIN="$PWD/target/img01/package/wx" WX_TEST_HEVC="$PWD/target/img01/synthetic.h265" cargo test --test image_cli -- --include-ignored
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.22s
     Running tests/image_cli.rs (target/debug/deps/image_cli-c1709756e291e494)

running 13 tests
test image_wrong_account_is_a_hard_identity_failure ... ok
test image_symlink_resource_cannot_escape_account_tree ... ok
test image_unassociated_bubble_is_ignored ... ok
test image_key_failure_has_no_secret_or_plain_output ... ok
test image_resource_join_never_uses_reused_local_id ... ok
test image_regular_and_pixel_area_selection ... ok
test image_equal_area_prefers_full_path_and_never_fuzzy_matches_chat ... ok
test image_wxgf_real_transcode ... ok
test image_resource_join_rejects_cross_shard_local_id_collision ... ok
test image_truncated_container_is_retryable_not_a_key_error ... ok
test image_output_directory_rejects_shared_or_linked_paths ... ok
test image_corrupt_large_falls_back_to_thumbnail ... ok
test image_missing_and_long_id_not_rounded ... ok

test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.82s
```

```text
$ cargo test --test account_cli captured_cli_returns_while_two_account_daemons_keep_running -- --exact --nocapture
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.05s
     Running tests/account_cli.rs (target/debug/deps/account_cli-58cfc5b95bf8ef54)

running 1 test
SKIP daemon IPC integration: sandbox denies ps: Operation not permitted (os error 1)
test captured_cli_returns_while_two_account_daemons_keep_running ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.00s
```

```text
$ cargo clippy --all-targets
warning: `wx-cli` (bin "wx") generated 39 warnings (2 duplicates) (run `cargo clippy --fix --bin "wx" -p wx-cli -- ` to apply 20 suggestions)
warning: `wx-cli` (bin "wx" test) generated 51 warnings (37 duplicates) (run `cargo clippy --fix --bin "wx" -p wx-cli --tests -- ` to apply 14 suggestions)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.67s
```

```text
$ cargo build --release --locked
   Compiling wx-cli v0.6.3 (/Users/jy/remote-dispatch/users/weicoz/wt/weicoz-wx-cli-again-customer-images-01-20261026-124212-316162000)
    Finished `release` profile [optimized] target(s) in 33.25s
```

额外检查：新增模块、Windows scanner 和图片 CLI 测试的 `rustfmt --check`、`git diff --check` 均退出 0；构建脚本 Python 编译在 `PYTHONPYCACHEPREFIX=$PWD/target/img01/pycache` 下退出 0。裸 `cargo fmt --check` 发现基线多个既有未格式化文件，未全仓改写；直接 rustfmt 初次因默认工具链未配置失败，改用当前 stable 工具链的 rustfmt 通过。初次 py_compile 因系统缓存不可写失败，未修改系统目录。

跨平台原始命令最初因缺 Linux gcc/Windows SDK、OpenSSL 的 Windows Perl 要求与 llvm-lib 失败，日志保留。后在本 worktree 的 target 内配置 Zig（Linux 交叉 C 编译）、cargo-xwin 0.23.1 + MSVC CRT/SDK 和实际交叉构建的 Windows OpenSSL 3.5.0，最终两项目标检查通过。Windows check 使用 OpenSSL 源码的 MinGW 目标静态 COFF 库进行跨平台类型检查，不代表 MSVC release 已链接或原生运行；Windows 原生构建/打包还需 CI/现场补验。工具、SDK、C 库只在忽略的 target 下，未作为生产替代依赖提交。没有用空库或跳过 C 源码编译伪造成功。

当前仓库未配置 core.hooksPath，公共 hooks 目录仅有 sample，没有启用的 hook；提交仍使用正常 git commit，不设置 no-verify。

## 未验证与现场需要用户做的事

- zero-s-pc 全部未验证：用户现场解锁、保持目标账号登录，以同一 Windows 用户运行完整交付包；记录手动打开已有图片前/后的取钥布尔结果、0/30秒/2/10/60分钟有限观察、同 key 至少三张图片及新 CLI 进程复用。只保留聚合指标，不保存 key 或客户正文。
- 原生 DPAPI CurrentUser/目录 ACL、其他 Windows 用户拒绝解封、真实进程内存取钥、历史版本有效性尚未运行。交叉检查和替身不能替代这些验收。
- 真实 WXGF 多分区/动画遮罩、真实缩略图尺寸与可读性、Bubble 确定关联和覆盖未验证；Bubble 暂时排除。报错弹窗/后台页/订单页各至少三张，由授权用户人工核对关键字段，只记完整/部分/不可读计数；本票不 OCR、不调图像模型。
- 跨天、退出登录、微信/主机重启需另约维护窗口，保持未验证；不自行执行。现场验收后的明文图片由操作端/操作者清理。
- 操作端样例同步及跨仓集成、各架构 Release workflow 实际执行、Windows 原生完整交付包尚未验证。既有 daemon IPC 被沙箱阻塞。

不应把 ready/资源来源等同于图片关键文字可读。本票未部署或发布任何产物。
