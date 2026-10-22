# 群消息按起点回读契约

操作端先验证配置中的群已开启采集，再串行安排补读与增量轮次，避免同库并发访问。本命令只读本地源库，不发送微信消息；不负责生成或管理 backfillId。

```sh
wx --wxid wxid_example --pid 1234 backfill '123@chatroom' \
  --since 1700000000 --until 1702592000 --cursor '' --limit 200
wx --wxid wxid_example backfill '123@chatroom' \
  --since 1700000000 --until 1702592000 --cursor '<next_cursor>' --limit 200
```

- `chat` 是精确群 wxid；账号必须用 `--wxid` 选择，`--pid` 的行为与 incremental 一致。
- `since`、`until` 必填，使用 UTC Unix 秒整数；要求非负、可表示的时间，且 `since < until`。只输出 `[since, until)` 内的消息；30 天的起止范围由控制端计算，CLI 不读系统时钟、不自行扩大窗口。
- `--cursor` 首次可省略或传空字符串。后续原样传回 `next_cursor`，并保持账号、群、since、until 一致。游标不透明，不得自行解码构造。
- `--limit` 默认 200，允许 1–10000。一页最多 limit 条，筛选在 SQL 查询中完成。

成功退出码 0，stdout 是带末尾 LF 的单行 JSON：

```json
{"account_wxid":"wxid_example","chat_wxid":"123@chatroom","done":true,"has_more":false,"messages":[],"next_cursor":"<opaque cursor>"}
```

此示意为已读完的空页；真实样例见 `tests/fixtures/contract/backfill-first.json`、`backfill-next.json`、`backfill-done.json`。`messages` 的逐条结构与 incremental 相同：字符串 `server_id` / `svrid`，`local_id`、`shard`、`chat_wxid`、`sender_wxid`、可空 `is_self`、数值 `type`、`text`、Unix 秒 `timestamp`、`mentions`、`quote`、`raw`。

顺序与 incremental 相同：分片相对路径字典序、各分片 local_id 升序，不按发送时间排序。不能遇到 timestamp >= until 就停止，因为更大的 local_id 仍可能是窗口内晚到消息。`has_more` 表示当前快照还有窗口内消息；`done = !has_more`，包含末页恰好 limit 条、已知空群与空窗口结果。`done=true` 后操作端结束请求，不需额外空页确认。

补读不读取、不创建、不修改账号的 `incremental.db`，也不保存隐式补读检查点。游标携带独立用途和时间范围；增量游标与补读游标交叉使用返回 `CURSOR_KIND_MISMATCH`。补读的相同输入在源库未变化时可重试；源库追加、补入历史消息后可能有新结果，不提供增量状态库的批次持久化重放保证。调用方按消息唯一键幂等，持久化 next_cursor；配置中请求消失时停止调度。

复用增量读取的分片发现、只读事务、账号/群绑定、正文及 @/引用解析、代际和边界核验。消息表/分片消失、代际变化、游标边界被删除均报错，不能自动重置游标。

执行期失败退出码 1，stdout 沿用 `{"error":{"code":"...","message":"..."}}`：

| code | 条件 |
|---|---|
| TIME_RANGE_INVALID | 负数、时间不可表示、since >= until |
| CURSOR_RANGE_MISMATCH | 续读请求的 since/until 与游标不一致 |
| CURSOR_KIND_MISMATCH | 补读与增量游标交叉使用 |
| CURSOR_INVALID | 游标损坏、版本/用途/字段非法 |
| CURSOR_ACCOUNT_MISMATCH / CURSOR_CHAT_MISMATCH | 游标账号或群不符 |
| CHAT_NOT_FOUND | 消息分片和联系人中均无此群 |
| CURSOR_OUT_OF_RANGE / DB_ROTATED | 沿用增量边界及代际错误 |

其余账号、密钥、消息未就绪、结构不支持等错误沿用 incremental。参数缺失、非整数、limit 越界由 clap 报参数错误（stderr、退出码 2），与既有 CLI 风格一致。
