# dlm 文档索引

**这是 `docs/` 的唯一入口。** 读完这一页，再去读具体文档；不要从 `STATUS` 的旧 dated 小节或某份证据开始。

## 当前状态（2026-11-12 复核）

**接手先读：[`2026-11-11-交接-HANDOVER.md`](2026-11-11-交接-HANDOVER.md)。** 本页是文档地图，那份才是当前交接。

| 项 | 值 |
|---|---|
| **GitHub** | **https://github.com/Samuel666198/dlm**（公开）· 首个 Release **`v0.1.0` 已上传** |
| 分支 | 开发 `sdd/dlm-kernel` @ `1af30cc`（已 push，**默认分支**）；发布 `main` @ `f9e024d`（孤儿分支只放 `latest.json`，摘要已回填） |
| 门禁 | `fmt 0` · `--lib 374/0` · `--bins 4/0` · `--test integration 121/0` · `clippy 0` · 扩展静态检查（统一走根目录 `gates.ps1`，它设 `DLM_NO_UPDATE_CHECK=1` 保证离线） |
| 产品面 | 内核 + CLI + **原生窗口/托盘 GUI** + Native Messaging 扩展（右键/页内悬浮球/媒体与文件嗅探/HLS 直链） |
| 最新进展 | `2026-11-12 更新功能 1-5 全部完成`（清单新格式 + 提示 UI + ping 双向版本号 + `dlm-updater.exe` + 扩展落地）· `2026-11-12 默认分支重设与首个 Release` |
| 交付物 | `target/package/dlm-0.1.0-win-x64.zip`，9,437,323 B，`sha256=97d3025429b2d8cf86d7fdb438e8ad3acf2c52febd3fd5ac6a1e33f06adcd4a6`；扩展 **0.2.13**。已同步到 `%LOCALAPPDATA%` |
| 下一步 | **发 `v0.1.1`** 验自我更新全链路（要用户点头）；速度/剩余两列、详情抽屉、保存目录与分类目录**已完成**（`evidence/2026-11-13-界面补完.md`）；下一轮待定 |
| 另一批任务 | 详情抽屉 + 统一目录 + 速度/剩余两列：[`evidence/2026-11-12-详情抽屉与统一目录梳理.md`](evidence/2026-11-12-详情抽屉与统一目录梳理.md) |
| 已修 | **E16**（继续入口）· **E17**（粘贴）· **E18**（设置透传）· **E19**（剪贴板提示）· 直连重投落点（A 方案）—— 见 [`evidence/2026-11-12-四项修复记录.md`](evidence/2026-11-12-四项修复记录.md) |
| 欠账 | 详情抽屉未画 · 下载目录三处不一致（`settings.out_dir` 死代码）· 速度/剩余列恒 `—` · 直连任务桌面端看不到 · errata E14（`--autostart` 隐藏窗口）· E15（启动 573ms）· exe 无内嵌图标 |
| 明确不做 | **站点抓取 / WebGrabber**（产品级功能，单独立项）；IE 内核监视、全局热键、代理服务器、声音事件 |
| 长期归档 | `E:\WorkSpace\archive\2026_retired\dlm-sdd-2026-10-15-dlm-kernel.zip`（`.superpowers/sdd` 的 2026-10-21 快照） |

## 怎么读

| 你的目的 | 按这个顺序读 |
|---|---|
| 第一次接手 | [`2026-11-11-交接-HANDOVER.md`](2026-11-11-交接-HANDOVER.md) → 本页 → `git log --oneline -12` |
| 改内核 | [`specs/2026-10-15-dlm-kernel-design.md`](superpowers/specs/2026-10-15-dlm-kernel-design.md) → [`plans/2026-10-15-dlm-kernel.md`](superpowers/plans/2026-10-15-dlm-kernel.md) 文末 `## 勘误` E1–E33（**与正文冲突以勘误为准**） |
| 改原生外壳（窗口/托盘） | [`specs/2026-11-09-native-shell-design.md`](superpowers/specs/2026-11-09-native-shell-design.md) → [`specs/2026-11-10-native-shell-errata.md`](superpowers/specs/2026-11-10-native-shell-errata.md) E1–E15（**同样以勘误为准**）→ [`evidence/2026-11-10-native-shell.md`](evidence/2026-11-10-native-shell.md) |
| 改产品层（GUI/队列） | [`specs/2026-10-22-dlm-product-ui-design.md`](superpowers/specs/2026-10-22-dlm-product-ui-design.md) → [`plans/2026-10-22-C1-C3-product-ui.md`](superpowers/plans/2026-10-22-C1-C3-product-ui.md) → [`specs/2026-10-27-dlm-quota-queue-b10-design.md`](superpowers/specs/2026-10-27-dlm-quota-queue-b10-design.md) |
| 改浏览器扩展 | `browser/README.md` → [`evidence/2026-10-29-inpage-widget.md`](evidence/2026-10-29-inpage-widget.md) → [`evidence/2026-10-28-media-sniff.md`](evidence/2026-10-28-media-sniff.md) |
| 看下一阶段 | [`plans/2026-10-21-三路路线图.md`](superpowers/plans/2026-10-21-三路路线图.md) → [`evidence/2026-10-21-idm能力对照表.md`](evidence/2026-10-21-idm能力对照表.md) 的 §10–§12 增量 |

## 编号释义

| 编号 | 出处 | 说明 |
|---|---|---|
| **E1–E33** | [`plans/2026-10-15-dlm-kernel.md`](superpowers/plans/2026-10-15-dlm-kernel.md) 文末 `## 勘误` | 实现期被实测推翻的结论；**权威，与正文/设计稿冲突以它为准**。E18 未发出，编号顺延，不回填 |
| **R1–R16** | [`specs/2026-10-15-dlm-kernel-design.md`](superpowers/specs/2026-10-15-dlm-kernel-design.md) §10 | 设计稿勘误；R5 已作废 R4 |
| **T0–T11** | 同计划 | 内核实现任务编号；T11 只在 E31 里作为账存在 |
| **T99-*** | `.superpowers/sdd/2026-10-15-dlm-kernel/` | 最终评审与修复轮的简报/报告（未跟踪，不入库） |
| **A1–A5 / A16** | 路线图 | 引擎侧：A1 If-Range、A2 限速、A3 完成动作、A4 劈分 v2、A16 配额；A5 IDM 断点兼容默认不做 |
| **B0–B3b / B10** | 路线图 | 浏览器侧：B0 取证、B1 宿主、B2 投递、B3a HLS、B3b-lite 嗅探、B10 无 Cookie 入队 |
| **C0–C3** | 路线图 | 产品层：Web UI、队列、交互、分类/更新 |
| **四闸 / floor** | 本页 | `--lib`、`--bins`、`--test integration`、`clippy -D warnings`；现在另加 `fmt --check` 与扩展静态检查，统一由根目录 `gates.ps1` 跑。数字是 floor，只许增、不许删、不许改判据方向 |

## 入口与状态（当前）

| 文档 | 是什么 |
|---|---|
| [`plans/2026-10-21-下一任交接-HANDOFF.md`](superpowers/plans/2026-10-21-下一任交接-HANDOFF.md) | 接手顺序、硬纪律、已裁定不要重做、具名欠账 |
| [`plans/2026-10-15-dlm-kernel-STATUS.md`](superpowers/plans/2026-10-15-dlm-kernel-STATUS.md) | 追加式执行账本：顶部是 2026-11-03 复核，越往下越旧 |
| [`plans/2026-10-21-三路路线图.md`](superpowers/plans/2026-10-21-三路路线图.md) | 下一阶段的范围与顺序（A/B/C） |
| [`plans/2026-10-18-控制员代裁清单.md`](superpowers/plans/2026-10-18-控制员代裁清单.md) | 我替用户做的 39 项裁定（含撤回两条） |

## 设计稿（specs）

| 文档 | 状态 |
|---|---|
| [`specs/2026-10-15-dlm-kernel-design.md`](superpowers/specs/2026-10-15-dlm-kernel-design.md) | 当前（内核；§10 R1–R16 勘误权威） |
| [`specs/2026-10-22-dlm-product-ui-design.md`](superpowers/specs/2026-10-22-dlm-product-ui-design.md) | 已交付（C0–C3） |
| [`specs/2026-10-27-dlm-quota-queue-b10-design.md`](superpowers/specs/2026-10-27-dlm-quota-queue-b10-design.md) | 已交付（A16/队列动作/B10） |
| [`specs/2026-10-28-b3a-hls-design.md`](superpowers/specs/2026-10-28-b3a-hls-design.md) | 已交付（B3a） |
| [`specs/2026-10-28-c3-categories-update-design.md`](superpowers/specs/2026-10-28-c3-categories-update-design.md) | 已交付（C3 增量） |
| [`specs/2026-10-28-media-sniff-design.md`](superpowers/specs/2026-10-28-media-sniff-design.md) | 已交付（B3b-lite） |

## 实现计划（plans）

| 文档 | 状态 |
|---|---|
| [`plans/2026-10-15-dlm-kernel.md`](superpowers/plans/2026-10-15-dlm-kernel.md) | 历史权威（T0–T10；文末 E1–E33） |
| [`plans/2026-10-21-A1-if-range.md`](superpowers/plans/2026-10-21-A1-if-range.md) | 已交付 |
| [`plans/2026-10-21-A2-rate-cap.md`](superpowers/plans/2026-10-21-A2-rate-cap.md) | 已交付 |
| [`plans/2026-10-21-A3a-completion-actions.md`](superpowers/plans/2026-10-21-A3a-completion-actions.md) | 已交付 |
| [`plans/2026-10-21-A3b-shutdown.md`](superpowers/plans/2026-10-21-A3b-shutdown.md) | 已交付 |
| [`plans/2026-10-21-A4-cursor-aware-split.md`](superpowers/plans/2026-10-21-A4-cursor-aware-split.md) | 已交付 |
| [`plans/2026-10-21-能力对照表.md`](superpowers/plans/2026-10-21-能力对照表.md) | 已交付（产出见 evidence） |
| [`plans/2026-10-22-C1-C3-product-ui.md`](superpowers/plans/2026-10-22-C1-C3-product-ui.md) | 已交付 |
| [`plans/2026-10-27-dlm-quota-queue-b10.md`](superpowers/plans/2026-10-27-dlm-quota-queue-b10.md) | 已交付 |
| [`plans/2026-10-28-b3a-hls.md`](superpowers/plans/2026-10-28-b3a-hls.md) | 已交付 |
| [`plans/2026-10-28-c3-categories-update.md`](superpowers/plans/2026-10-28-c3-categories-update.md) | 已交付 |
| [`plans/2026-10-28-media-sniff.md`](superpowers/plans/2026-10-28-media-sniff.md) | 已交付 |
| [`plans/2026-11-07-v21-product-polish.md`](superpowers/plans/2026-11-07-v21-product-polish.md) | v2.1 计划，**已全部交付**（P1–P4）；托盘 P5 暂缓 |

## 证据（evidence，一次性实测，不回改）

| 文档 | 主题 |
|---|---|
| [`evidence/2026-10-18-spec7-四场景验收.md`](evidence/2026-10-18-spec7-四场景验收.md) | spec §7 四场景 |
| [`evidence/2026-10-21-idm能力对照表.md`](evidence/2026-10-21-idm能力对照表.md) | IDM 能力 × dlm 现状（含增量） |
| [`evidence/2026-10-22-native-messaging-recon.md`](evidence/2026-10-22-native-messaging-recon.md) | B0 只读取证 |
| [`evidence/2026-10-22-b1-native-messaging.md`](evidence/2026-10-22-b1-native-messaging.md) | B1 ping/pong |
| [`evidence/2026-10-22-b2-browser-download.md`](evidence/2026-10-22-b2-browser-download.md) | B2 右键/页面投递 |
| [`evidence/2026-10-22-c3-gui.md`](evidence/2026-10-22-c3-gui.md) | C3 Web UI |
| [`evidence/2026-10-27-quota-queue-b10.md`](evidence/2026-10-27-quota-queue-b10.md) | A16 配额 / B10 入队 |
| [`evidence/2026-10-28-b3a-hls.md`](evidence/2026-10-28-b3a-hls.md) | B3a HLS |
| [`evidence/2026-10-28-c3-categories-update.md`](evidence/2026-10-28-c3-categories-update.md) | 分类与更新检查 |
| [`evidence/2026-10-28-media-sniff.md`](evidence/2026-10-28-media-sniff.md) | B3b-lite 媒体/文件嗅探 |
| [`evidence/2026-10-28-native-host-window.md`](evidence/2026-10-28-native-host-window.md) | 宿主无窗口与完成反馈 |
| [`evidence/2026-10-28-package.md`](evidence/2026-10-28-package.md) | 便携包产物、hash、冒烟 |
| [`evidence/2026-10-28-tabbit-extension.md`](evidence/2026-10-28-tabbit-extension.md) | Tabbit 扩展加载记录 |
| [`evidence/2026-10-28-user-agent-403.md`](evidence/2026-10-28-user-agent-403.md) | UA 缺失导致 403 |
| [`evidence/2026-10-29-inpage-widget.md`](evidence/2026-10-29-inpage-widget.md) | 页内组件与站点脚本拦截 |
| [`evidence/2026-11-03-floating-ball.md`](evidence/2026-11-03-floating-ball.md) | 悬浮球拖动、位置记忆、闲置淡出与面板标签页 |
| [`evidence/2026-11-08-v21-product-polish.md`](evidence/2026-11-08-v21-product-polish.md) | v2.1 四项交付与验收（打开文件/自动重试/UI 收尾/自启与单实例） |
| [`evidence/2026-11-10-native-shell.md`](evidence/2026-11-10-native-shell.md) | 原生外壳交付：窗口/托盘/设置/剪贴板/限流/休眠 |
| [`evidence/2026-11-11-extension-ball-fixes.md`](evidence/2026-11-11-extension-ball-fixes.md) | 扩展悬浮球点击失效：三轮排障与修复（0.2.9→0.2.11），含误判复盘 |
| [`evidence/2026-11-11-暂停与续传缺口.md`](evidence/2026-11-11-暂停与续传缺口.md) | E16：暂停后没有「继续」入口 —— 三条投递路径的入口缺口定位（只读，未修） |
| [`evidence/2026-11-11-首个测试版包.md`](evidence/2026-11-11-首个测试版包.md) | 首个测试版：包 hash、打包时实测、建议实测顺序、已知会坏清单、反馈位 |
| [`evidence/2026-11-11-粘贴失效与MCP选型.md`](evidence/2026-11-11-粘贴失效与MCP选型.md) | E17：桌面端粘贴失效根因（`eframe` 缺 `clipboard` feature）；MCP vs CLI 选型裁定（选 CLI，且已存在） |
| [`evidence/2026-11-12-IDM选项面板与站点抓取对标.md`](evidence/2026-11-12-IDM选项面板与站点抓取对标.md) | IDM 选项面板逐项对照（E18 设置不透传、E19 剪贴板无反应）；站点抓取裁定不做 |
| [`evidence/2026-11-12-四项修复记录.md`](evidence/2026-11-12-四项修复记录.md) | 第二版：E16/E17/E18/E19 修复内容、实测证据、测试竞态修复、直连重投的设计问题 |
| [`evidence/2026-11-12-详情抽屉与统一目录梳理.md`](evidence/2026-11-12-详情抽屉与统一目录梳理.md) | 详情抽屉是半截、下载目录三处不一致、速度/剩余列恒 `—` |
| [`evidence/2026-11-12-安装位置与下载目录方案.md`](evidence/2026-11-12-安装位置与下载目录方案.md) | 装 `E:\Program Files\dlm` / 数据 `E:\ProgramData\dlm` / 下载 `E:\dlm-downloads` |
| [`evidence/2026-11-12-更新功能方案.md`](evidence/2026-11-12-更新功能方案.md) | 更新功能现状（半截）与设计；扩展不能自更新的硬约束 |
| [`evidence/2026-11-12-更新功能裁定.md`](evidence/2026-11-12-更新功能裁定.md) | 裁定：GitHub 公开仓库 + 默认自动检查 + 不上商店；实施顺序 |
| [`evidence/2026-11-12-更新功能-步骤1-3.md`](evidence/2026-11-12-更新功能-步骤1-3.md) | 更新功能步骤 1-3（清单新格式 / 提示界面 / ping 双向版本号） |
| [`evidence/2026-11-12-更新功能-步骤4-5.md`](evidence/2026-11-12-更新功能-步骤4-5.md) | 更新功能步骤 4-5（`dlm-updater.exe` + 扩展落地）；E2E 20 项；两个 PowerShell 坑 |

## 不在 `docs/` 的账本

| 位置 | 是什么 |
|---|---|
| `.superpowers/sdd/2026-10-15-dlm-kernel/` | 逐任务报告、评审原文、review diff、`progress.md`；未跟踪、不入库，`git clean -fdx` 会连账清掉 |
| `E:\WorkSpace\archive\2026_retired\dlm-sdd-2026-10-15-dlm-kernel.zip` | 上述目录 2026-10-21 的只读归档 |
| `target/live-check/` | 2026-10-21 真公网验收的原始输出；可重跑再生 |
