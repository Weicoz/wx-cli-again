# 按消息提取最佳本地图片（契约 v1）

```powershell
wx --wxid wxid_test_collector image resolve 10001@chatroom --message-id 9007199254740993002 --output-dir D:\restricted-temp\images --json
```

`--wxid`、精确群 wxid、字符串服务端消息 ID、绝对输出目录为必需项。只读本机数据库和已经落盘的资源；不操作桌面、不下载图片。stdout 只输出一份 JSON，stderr 不打印底层异常、XML、图片正文或密钥。成功退出 0；图片不可用退出 2；参数、登录、数据库或进程故障退出 1（参数解析可由 clap 返回 2，操作端须结合 JSON 形状判断）。`ACCOUNT_MISMATCH` 必须使操作端停止对应账号任务。

消息 ID 全程作为字符串，SQL 使用 `CAST(server_id AS TEXT)` 精确比较。资源按群、local_id、消息秒数、图片类型精确连接，不使用旧 `extract` 的同 local_id 最新行回退。跨分片重复、local_id/时间与另一服务端 ID 冲突或缺失消息均不猜测。资源 MD5 只取唯一的已识别 protobuf 字段，不从任意字符串或昵称推断。只枚举该账号、该群 attach 树下同一 MD5 的 full / `_h` / `_t` 文件；拒绝符号链接和越出数据树的候选。

对每个成功解码候选比较真实像素面积，最大者胜出；同面积依次 full_path、hd、bubble、thumbnail，同来源按路径稳定排序。损坏大图不妨碍选择可解缩略图。**当前资源 schema 没有证明 Bubble 与消息关联的字段，所以本版本不采用 Bubble**；不会按文件名近似或体积猜测其关联。`resourceKind` 只表示来源，不能当作文字可读性保证。

输出统一为真实解码后重新编码的静态 PNG；GIF、APNG、动画 WebP 取第一帧并在 `convertedFrom` 记录原格式，BMP/TIFF 转 PNG 时也记录来源。JPEG、静态 PNG/WebP 的 `convertedFrom=null`。新路径不启动 daemon、不推进 incremental/backfill 游标；图片密钥不可得不会使文本读取失败。已有 attachments / extract / incremental 接缝不变。

## JSON 字段

六类合成样例位于 `tests/fixtures/contract/image-*.json`；操作端由编排方逐字复制到 `tests/fixtures/wx-cli-contract/`。不修改外仓。样例仅将每次生成的绝对 `outputPath` 替换为 `<restricted-output-dir>/image.png`，其余字段均来自真实 CLI；样例不含图片字节。

| 字段 | 约定 |
| --- | --- |
| contractVersion | 固定 1 |
| accountWxid / chatId / messageId | 请求身份，messageId 始终为字符串 |
| status | ready 或 unavailable |
| resourceKind | thumbnail / hd / full_path / bubble；unavailable 为 null |
| outputPath | fork 在指定受限目录中创建的随机文件路径；unavailable 为 null |
| mimeType / width / height / byteLength / sha256 | 实际输出的 MIME、真实尺寸、字节数和 64 位小写 SHA-256；unavailable 全为 null |
| convertedFrom | wxgf / gif / png / webp / bmp / tiff 或 null |
| errorCode | ready 为 null；unavailable 为下表错误码 |
| retryable | 只对暂态错误为 true；身份错误不能重试吞掉 |

| errorCode | retryable | 含义 |
| --- | --- | --- |
| MESSAGE_NOT_READY | true | 消息尚未落盘、服务端 ID 不存在或关联有歧义 |
| LOCAL_RESOURCE_MISSING | true | 没有该消息确定关联的已落盘资源 |
| IMAGE_KEY_UNAVAILABLE | false | 图片 key 不可得、DPAPI 不能解封或安全缓存不可写 |
| RESOURCE_INCOMPLETE | true | 文件仍在变化、为空、V2 段长度不足或 WXGF 分区截断 |
| DECODE_FAILED | false | 已知格式损坏或完整资源不能解码 |
| FORMAT_UNSUPPORTED | false | 不支持的图片格式/容器布局或超出输入资源上限 |
| TRANSCODE_FAILED | false | 随包解码依赖缺失/校验不符，或 HEVC 转码失败 |
| ACCOUNT_MISMATCH | false | 账号、目录、本人联系人或安全记录作用域不符 |
| TIMEOUT | true | 有界取钥、缓存锁或转码预算耗尽 |

## 密钥与明文目录

Windows 使用 DPAPI CurrentUser（不设 LOCAL_MACHINE，禁止交互 UI）保护整个缓存记录，以账号 wxid + 规范化 db_dir 的 SHA-256 作为作用域及 DPAPI entropy。缓存位置为账号应用目录下 `image-keys/<scope>/image-keys.dpapi`，内含记录版本、作用域、验证时间及最多四个历史 key 版本；磁盘只写 DPAPI 密文。目录 DACL 禁止继承，仅当前用户 SID 获得完整权限；文件继承该 ACL。原子替换缓存，并用系统文件锁协调并行 CLI（等锁最多 500ms）。密钥结构不实现 Debug，明文序列化缓冲和 key 在释放时清零。

每次使用先反验目标资源。所有历史版本均不符时，每条 resolve 请求至多扫描一次目标账号主进程（扫描预算 3 秒），不因 PID 变化清空历史版本。目标 V2 长度不足不会扫描；AES 首块已验证、后续体损坏时返回资源错误，不当作 key 失效。对其他图片或旧版本仍可用的 key 保留有界验证能力。不自动打开图片，不退出或重启微信。非 Windows 的新命令不会把 DPAPI 替换成明文文件；加密 V2 返回 IMAGE_KEY_UNAVAILABLE，安全缓存测试使用显式替身。旧 extract 的平台 provider 行为保留。

输出目录必须为绝对路径，不能是链接、源账号数据目录、账号配置目录或这两个目录的祖先；先检查该边界，再创建目录/改 ACL。在 Unix 上新建目录权限 0700，已存在的目录须归当前用户且无 group/other 权限；Windows 应用当前用户专用 DACL。图片以独占创建的随机文件名输出，不覆盖既有文件；操作端负责上传后、取消或崩溃恢复时清理。没有成功候选时不创建明文文件。

## WXGF 与随包解码依赖

运行时只使用 `wx[.exe]` 旁 `image-codec/` 中的 FFmpeg，绝不从 PATH 找解码器，不加载微信 DLL。源码锁定 FFmpeg 8.0.1：

- URL：https://ffmpeg.org/releases/ffmpeg-8.0.1.tar.xz
- SHA-256：`05ee0b03119b45c0bdb4df654b96802e909e0a752f72e4fe3794f487229e5a41`
- 最小构建只启用本地 pipe、HEVC parser/demuxer/decoder、BMP 编码、image2pipe、像素格式转换；禁用网络、自动发现外部库及 GPL 组件。BMP 仅在内存 pipe 中流转，再由锁定 image crate 转为 PNG。
- `manifest.json` 记录源码锁和实际二进制 SHA-256；运行时检查版本、源码锁与二进制摘要。交付目录同时带完整对应源码归档、LGPL 许可和构建参数。FFmpeg 是独立 LGPL 程序，不把它标成项目的 Apache-2.0。

```bash
cargo build --release --locked
python3 scripts/build-image-codec.py --install-dir target/release/image-codec
# 整体交付 target/release/wx[.exe] 和 target/release/image-codec/。
```

Windows 解码器在 MSYS2 MINGW64（gcc、make、python）下运行同一脚本，添加 `--target x86_64-pc-windows-msvc`；wx.exe 仍由 MSVC Rust 工具链构建。Release workflow 为各平台生成 `wx-image-<target>.zip`，同时把依赖放进 npm 平台包。旧单二进制资产保留，图片消费者应使用完整 zip。下载/源码编译只在构建阶段发生，客户主机离线运行不下载依赖。

WXGF 先验证头长，再在有限头区寻找 BE 长度分区，逐段检查边界和 Annex B NALU（禁止 forbidden bit、要求合法 temporal ID，首段须有 VPS/SPS/PPS 和图像片段），最多 64 分区。不从第一个起始码直接吞掉整个尾部。静态取首个可解分区；多个近似等大分区按已知 mask/color 交替布局尝试首个可解 color，丢弃 mask，不承诺动画透明度保真。其他布局可返回 unavailable，真实版本覆盖须现场确认。实现参考结构依据：[chatlog wxgf.go 固定版本](https://github.com/majin72/chatlog/blob/a7162bca9454fa43b5950a2414670983fe180e56/pkg/util/dat2img/wxgf.go)，未复制其实现。

单文件读取上限 64MiB，图片解码上限 2000 万像素与 100MiB 分配预算；HEVC 子进程限制单次分配和像素数，stdout 有界，stderr 丢弃，超时终止并回收。候选数最多 256 月目录 × 3 文件；新请求的解码循环使用 10 秒截止时间（不含前置进程/数据库验证，单个同步标准图片解码不能被该截止时间抢占）。这不替代操作端整页 15 秒预算及子进程终止。

## 可重复检查

```bash
cargo check
cargo check --target x86_64-unknown-linux-gnu
cargo check --target x86_64-pc-windows-msvc
cargo test -- --test-threads=1
cargo clippy --all-targets
cargo build --release --locked
```

`tests/image_cli.rs` 使用合成数据库、运行时生成的图片和假进程占用；临时目录用 Drop 清理，不访问客户微信。六份 golden 验证字段、文件真实解码、尺寸、长度和摘要，仅归一化输出路径；更新命令为 `UPDATE_CONTRACT_FIXTURES=1 cargo test --test image_cli`。普通 cargo test 中唯一新增 ignore 是真实 WXGF 编解码用例，需要先在临时目录生成合成 HEVC；不要把合成图片文件纳入 Git。

```bash
# 测试生成器可使用支持 libx265 的 FFmpeg；仅生成黑色测试帧，非运行时依赖。
ffmpeg -nostdin -v error -f lavfi -i color=black:s=16x16:r=1 -frames:v 1 -c:v libx265 -x265-params pools=none:frame-threads=1:log-level=error -f hevc /tmp/wx-synthetic.h265
python3 scripts/build-image-codec.py --install-dir target/debug/image-codec
WX_TEST_HEVC=/tmp/wx-synthetic.h265 cargo test --test image_cli -- --include-ignored
rm /tmp/wx-synthetic.h265
```

安全缓存测试使用非 Windows 替身覆盖：跨账号/数据目录拒绝、模拟 DPAPI 身份不符、缓存进程重开、目标失效后仅扫描一次、历史版本复用、体损坏不重扫和四版本上限。另有 Windows 专属原生 DPAPI CurrentUser/entropy 往返测试，在非 Windows 不运行。

## zero-s-pc 现场验收：全部未验证

本票未连接或修改 zero-s-pc，未发送微信消息，未改自动下载设置。需要用户现场解锁并保持微信登录，以实际采集账号身份运行完整 Windows 交付包；所有客户图片只保留在授权的受限临时目录，验收完删除。

1. 先记录打开前取钥布尔结果，再由用户手动打开一张已有图片，重复同消息取图；不要发送消息。确认首次成功后 0/30秒/2/10/60分钟的有限观察，仅保存成功数、尺寸、格式、来源、错误码和耗时。
2. 同一个安全缓存跨至少三张已有图片和新的 wx.exe 进程复用；检查 DPAPI 文件非 JSON、目录 ACL 只有当前用户，另一个 Windows 身份不能解封。测试历史 key 是否可对旧图片反验。不要输出 key。
3. 对真实 WXGF、截断文件和缩略图回退检查实际输出可解；Bubble 当前被保守排除，需要后续拿到确定关联证据才能扩大支持，不按体积或名称推断。
4. 报错弹窗、后台页、订单页各至少三张，由授权用户人工核对关键字段，只汇总完整/部分/不可读计数，不记录正文或 OCR 文本。
5. 图片失败时继续 incremental，确认文本和原游标/ack 接缝不受影响。跨天、退出登录、微信重启、主机重启分别记未验证；须另约维护窗口，不能自行执行。

跨仓样例同步、操作端集成、Release workflow 各架构真正运行和 Windows 原生依赖打包亦待编排方验收。交叉 cargo check 只证明目标代码可编译，不证明 Windows 密钥、DPAPI/ACL 或真实微信格式可用。
