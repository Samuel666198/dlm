# B3a 最小 HLS 直链下载（设计）

**状态**：已实现（2026-10-28；四闸 305/4/117/clippy 0）
**范围**：只做 CLI 直链 `--hls`；不接队列/浏览器嗅探，不做 DASH/DRM。

## 1. 入口

- `dlm --hls --url <playlist.m3u8> ...`；`--hls` 是**显式开关**，不自动识别扩展名
  （队列/浏览器投递的 `.m3u8` 仍按普通文件处理，避免静默改变行为）。
- `--hls` 与 `--data-file`、`--max-bytes` 互斥（HLS 是顺序拼接、不可按字节断点续传）。
- 默认输出名：URL 末段；若以 `.m3u8` 结尾则换成 `.ts`；无末段则 `download.ts`。
  `--out-name` 仍优先。
- `--max-speed` 可叠加（每读一块 consume）；`--sha256` 在合并完成后校验。
- 不复用 `.dlm.json`、不可续传；失败保留已写的半截文件，下次从头开始。
- `--no-start` 只打印摘要，不发请求。

## 2. 解析

- 读 m3u8 文本（保留相对 URL 的基准）：
  - master：`#EXT-X-STREAM-INF:BANDWIDTH=n` 后的 URI，选 BANDWIDTH 最大者；无 BANDWIDTH 取第一。
  - media：`#EXTINF` 后的 URI 为分片；`#EXT-X-MAP:URI="…"` 的初始化段先下。
  - `#EXT-X-KEY:METHOD=NONE` 放行；其它 KEY（AES-128/SAMPLE-AES）⇒ 明确报“加密 HLS 不支持”。
  - 没有分片 ⇒ 报错；`#EXT-X-ENDLIST` 不强制（VOD 列表照下）。
- 所有 URI 用 `Url::join` 解析；跨 host 的分片请求**不带** Cookie/Auth/Referer（防凭据外泄）；
  同 host 沿用 Task 的请求头（identity/Accept/Referer/Cookie/Basic）。
- 分片逐个 GET，按响应 body 顺序写入同一个输出文件；总字节 = 实际写入量。

## 3. 报告与退出码

- 成功：`Report{ok:true, bytes_total=delivered, confirmed=delivered, message:None}`；
  `--sha256` 不符 ⇒ `ok:false`，保留文件。
- HTTP/解析/网络失败：`Ok(Report{ok:false,message})`（不是“跑不起来”的 `Err`），已写字节数如实报；
  退出码 1。
- HLS 不产生进度行；`main` 不打印进度行的前导换行。
- `--on-complete` 在成功后照常执行（沿用 main 的现有逻辑）。

## 4. 验收

- 单测：master 选最高带宽、相对 URI、`#EXT-X-KEY` 拒绝、默认输出名。
- 集成：本机 node 对端提供 master/media + 三个分片，`--hls` 合并后逐字节等于期望；
  `--data-file`/`--max-bytes` 同给退 2；加密 KEY 退 1 且不落完整文件。
- 四闸；README/STATUS/路线图/能力表/证据。
- 不做：DASH、DRM、浏览器媒体嗅探、队列 HLS、断点续传。
