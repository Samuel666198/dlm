# dlm 文档索引

**这是 `docs/` 的唯一入口。** 读完这一页，再去读具体文档；不要从 `STATUS` 的旧 dated 小节或某份证据开始。

## 当前状态（2026-11-03 复核）

| 项 | 值 |
|---|---|
| 分支 | `sdd/dlm-kernel`，未 push、无远程 |
| 门禁 | `fmt 0` · `--lib 322/0` · `--bins 4/0` · `--test integration 121/0` · `clippy 0` · 扩展静态检查 · UI 脚本语法（统一走根目录 `gates.ps1`） |
| 产品面 | 内核 + CLI + 本地 Web UI 队列 + Native Messaging 扩展（右键/页内组件/媒体与文件嗅探/HLS 直链） |
| 最新进展 | `2026-11-03 悬浮球`（扩展 0.2.7）、`2026-10-29 页内下载组件`、`2026-10-29 站点脚本下载拦截`、`2026-10-28 Windows x64 便携包` |
| v2.1 交付 | [`plans/2026-11-07-v21-product-polish.md`](superpowers/plans/2026-11-07-v21-product-polish.md) 已全部落地（P2 打开文件 / P1 自动重试 / P4 UI 收尾 / P3 自启与单实例）；托盘 P5 暂缓 |
| 长期归档 | `E:\WorkSpace\archive\2026_retired\dlm-sdd-2026-10-15-dlm-kernel.zip`（`.superpowers/sdd` 的 2026-10-21 快照） |

## 怎么读

| 你的目的 | 按这个顺序读 |
|---|---|
| 第一次接手 | 本页 → [`plans/2026-10-21-下一任交接-HANDOFF.md`](superpowers/plans/2026-10-21-下一任交接-HANDOFF.md) → [`plans/2026-10-15-dlm-kernel-STATUS.md`](superpowers/plans/2026-10-15-dlm-kernel-STATUS.md) 顶部的 2026-11-03 复核块与 2026-10-20 收口节 → `git log --oneline -8` |
| 改内核 | [`specs/2026-10-15-dlm-kernel-design.md`](superpowers/specs/2026-10-15-dlm-kernel-design.md) → [`plans/2026-10-15-dlm-kernel.md`](superpowers/plans/2026-10-15-dlm-kernel.md) 文末 `## 勘误` E1–E33（**与正文冲突以勘误为准**） |
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

## 不在 `docs/` 的账本

| 位置 | 是什么 |
|---|---|
| `.superpowers/sdd/2026-10-15-dlm-kernel/` | 逐任务报告、评审原文、review diff、`progress.md`；未跟踪、不入库，`git clean -fdx` 会连账清掉 |
| `E:\WorkSpace\archive\2026_retired\dlm-sdd-2026-10-15-dlm-kernel.zip` | 上述目录 2026-10-21 的只读归档 |
| `target/live-check/` | 2026-10-21 真公网验收的原始输出；可重跑再生 |
