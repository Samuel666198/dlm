# 右键/点击下载弹终端窗口与完成反馈（2026-10-28）

## 症状

在 Tabbit 里点击一个下载链接后，出现一个**空白终端窗口**，页面没有任何“开始/完成”提示；
关掉终端窗口后，正在下载的任务也停了。

## 现场证据

- `%USERPROFILE%\Downloads` 有
  `diffusion_pytorch_model-00001-of-00002.safetensors`（9,350,576,863 B）和同名 `.dlm.json`；
  对应任务日志 `1793196276-2.log` 为 **0 字节**（dlm 的 stdout/stderr 被重定向到日志，终端窗口里本来就没有内容）。
- 宿主创建 dlm 时没有设置 `CREATE_NO_WINDOW`：控制台子系统程序由浏览器后台进程拉起时，
  Windows 会新开一个控制台窗口；关窗口 = 杀掉 dlm。

## 修复

1. **宿主不再弹窗**：`src/host.rs` 在 Windows 上给 `Command` 加
   `creation_flags(CREATE_NO_WINDOW)`（`0x0800_0000`）。
2. **完成/失败可见**：宿主在子进程退出后向扩展回一条
   `{op:"done", job, ok, code, log}`；`background` 存 `lastResult`、切徽标并通知当前标签页；
   页面右下角显示“正在下载 / 下载完成 / 下载失败（退出码）”，popup 同步显示最近结果。
   新判据：`host::tests::host_reports_when_the_download_process_exits`。
3. **Service Worker 缓存**：Tabbit 里只更新 `background.js` 文件、重启浏览器后，
   Service Worker 仍跑旧代码（content 已是新版本、`background` 的新分支不执行）。
   改为稳定入口 `background-sw.js`，按扩展版本
   `importScripts("background-impl.js?v=<version>")`；升版本即换 URL，强制重取实现。
   验证标记：content 的 `data-dlm-version` 与 storage 的 `buildVersion`。

## 验证

- Tabbit 真实左键点击本地慢速链接（32 MiB，约 3 MB/s）：
  - 下载期间 `Get-Process dlm` 的 `MainWindowHandle = 0`（没有可见终端窗口）；
  - 落盘 `slow.bin` **33,554,432 B**；
  - 页面状态从 `dlm: 正在下载 <job>` 变为 `dlm: 下载完成`。
- Edge 156 自动化（`verify-media-sniff.mjs`）：`host_status=dlm: 下载完成`、
  `done_status=dlm: 下载完成`；面板下载、点击接管、普通链接对照全过。
- 扩展版本 **0.2.4**；已安装副本与源码一致。

## 被中断的 9.35 GB 文件

- 该次运行被关窗口杀掉时，断点里只确认了很少的覆盖（日志 0 B、状态文件仅几 KB）；
  旧文件里的 9.35 GB 大部分**没有记进断点**，dlm 不能安全信任。
- 用断点里的 URL/Referer/Cookie 续传时，dlm 实际从头重下（约 5–6 MB/s），已停止，避免再跑一遍 10 GB。
- 因此 `Downloads` 里那份 `diffusion_pytorch_model-00001-of-00002.safetensors` **不是完整文件**；
  需要的话请删除后重新点击下载（现在不会再弹终端窗口，状态会显示“正在下载/完成”）。
- 后续若要少丢进度，应把“运行中被硬杀时的状态保存频率”纳入内核改进项，而不是信任未确认字节。

## 四闸

`--lib` **309/0**（新增 1 条 host done 判据）· `--bins` **4/0** · `integration` **117/0** ·
`clippy --all-targets -- -D warnings` **0**。
