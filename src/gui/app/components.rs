//! 组件状态：桌面端（含内核）/ MCP 桥接 / 浏览器扩展 / 更新通道。
//!
//!
//! ## 为什么要有这一层
//!
//! 这四个东西以前各有各的活法：版本一览塞在「高级」页顶上、扩展版本在「浏览器扩展」页
//! 又列一遍、MCP 只判"文件在不在"、状态栏孤零零一个 `v0.7.0`。同一个"扩展版本"在三处
//! 指三样东西，用户问"我装的哪个版本"时没有一处能一次答全。
//!
//! 所以先把**判定**抽成纯函数（本文件），界面只负责把它画成四张卡。
//!
//! ## 四态，一条不能混
//!
//! | 态 | 含义 | 典型 |
//! |---|---|---|
//! | [`ComponentState::Ok`] | 一切正常 | 扩展运行版本 == 磁盘版本 |
//! | [`ComponentState::NeedsAction`] | 装了，但用户得做一件事 | 磁盘 0.2.16、浏览器在跑 0.2.15 |
//! | [`ComponentState::Missing`] | **确实**没有 | 找不到 `dlm-mcp.exe` |
//! | [`ComponentState::Unknown`] | 探测不到，**不等于没装** | 扩展没连上、读不到版本、还没检查过 |
//!
//! `Unknown` 单独存在的理由：把"探测不到"写成"未安装"是**措辞事故** —— 用户会去重装一个
//! 本来就装着的程序。这条由用例 `an_unreachable_component_is_unknown_not_missing` 钉住。

use crate::gui::app::theme;

/// 四个组件。顺序就是界面上的顺序。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ComponentId {
    Desktop,
    Mcp,
    Extension,
    Channel,
}

impl ComponentId {
    pub(crate) const ALL: &'static [ComponentId] = &[
        ComponentId::Desktop,
        ComponentId::Mcp,
        ComponentId::Extension,
        ComponentId::Channel,
    ];

    pub(crate) fn title(self) -> &'static str {
        match self {
            ComponentId::Desktop => "桌面端",
            ComponentId::Mcp => "MCP 桥接",
            ComponentId::Extension => "浏览器扩展",
            ComponentId::Channel => "更新通道",
        }
    }
}

/// 一个组件此刻处在什么状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ComponentState {
    Ok,
    /// 带一句"要做的那件事"，界面直接把它显示出来，不再另写提示。
    NeedsAction(String),
    Missing,
    Unknown(String),
}

impl ComponentState {
    /// 状态那一格显示的字。
    pub(crate) fn short(&self) -> String {
        match self {
            ComponentState::Ok => "正常".to_string(),
            ComponentState::NeedsAction(_) => "需处理".to_string(),
            ComponentState::Missing => "未安装".to_string(),
            ComponentState::Unknown(_) => "未知".to_string(),
        }
    }

    /// 颜色语义：正常=绿、需处理=警示、未安装/未知=灰。
    ///
    /// 「未安装」不用红：它不是一个错误，MCP 本来就是按需装。
    pub(crate) fn color(&self) -> eframe::egui::Color32 {
        match self {
            ComponentState::Ok => theme::ok(),
            ComponentState::NeedsAction(_) => theme::warn(),
            ComponentState::Missing | ComponentState::Unknown(_) => theme::ink_3(),
        }
    }
}

/// 界面上的一行（一张卡）。
pub(crate) struct ComponentRow {
    pub id: ComponentId,
    /// 版本那一列。读不到就给 `—`，**不编**。
    pub version: String,
    pub state: ComponentState,
    /// 状态下面那行说明：为什么是这个态、下一步做什么。
    pub hint: String,
}

/// 判定所需的全部外部事实。参数化是为了能测：真去问文件系统/起子进程的用例，
/// 会在别人的机器上红。
pub(crate) struct ComponentInput<'a> {
    pub app_name: &'a str,
    pub release: &'a str,
    /// 内核版本（清单/本地标记里的 core）。
    pub core_current: &'a str,
    /// 扩展：磁盘上那份（`%LOCALAPPDATA%\dlm\extension` 的 manifest）。
    pub extension_disk: Option<&'a str>,
    /// 扩展：浏览器里**真正在跑**的那份（扩展自己上报）。
    pub extension_running: Option<&'a str>,
    /// 扩展：本程序包里带的。
    pub extension_packaged: Option<&'a str>,
    /// 真跑 `dlm-mcp --version` 拿到的版本；`None` = 没探测到版本。
    pub mcp_version: Option<&'a str>,
    /// 与 GUI 同目录有没有 `dlm-mcp.exe`。
    pub mcp_present: bool,
    /// 更新清单地址；`None` = 更新检查被关掉了。
    pub channel_url: Option<&'a str>,
    pub latest: Option<&'a str>,
    /// 上一次检查的时间戳（秒），`None` = 还没检查过。
    pub checked_at: Option<u64>,
    pub update_error: Option<&'a str>,
}

/// 四行状态。顺序固定 = [`ComponentId::ALL`] —— **一处定义**，别再手抄一遍。
pub(crate) fn build_rows(input: &ComponentInput<'_>) -> Vec<ComponentRow> {
    ComponentId::ALL
        .iter()
        .map(|id| match id {
            ComponentId::Desktop => desktop_row(input),
            ComponentId::Mcp => mcp_row(input),
            ComponentId::Extension => extension_row(input),
            ComponentId::Channel => channel_row(input),
        })
        .collect()
}

fn desktop_row(input: &ComponentInput<'_>) -> ComponentRow {
    ComponentRow {
        id: ComponentId::Desktop,
        version: format!("{} {}", input.app_name, input.release),
        state: ComponentState::Ok,
        hint: format!(
            "内核 dlm.exe {}；内核与桌面端同一个安装包，随它一起升级",
            if input.core_current.trim().is_empty() {
                "—"
            } else {
                input.core_current
            }
        ),
    }
}

fn mcp_row(input: &ComponentInput<'_>) -> ComponentRow {
    if let Some(version) = input.mcp_version.filter(|v| !v.trim().is_empty()) {
        return ComponentRow {
            id: ComponentId::Mcp,
            version: version.to_string(),
            state: ComponentState::Ok,
            hint: "装在本程序同目录；它自己去找 dlm-gui 写的端点文件，不用抄 token".to_string(),
        };
    }
    if input.mcp_present {
        // 文件在、版本读不出来 —— 这是"探测不到"，不是"没装"。
        return ComponentRow {
            id: ComponentId::Mcp,
            version: "—".to_string(),
            state: ComponentState::Unknown("有 dlm-mcp.exe，但读不到版本".to_string()),
            hint: "点「重新探测」；仍然读不到就试着直接运行一次 dlm-mcp.exe --version".to_string(),
        };
    }
    ComponentRow {
        id: ComponentId::Mcp,
        version: "未安装".to_string(),
        state: ComponentState::Missing,
        hint: "单独分发，约 2 MB；装到本程序同目录后自动识别".to_string(),
    }
}

fn extension_row(input: &ComponentInput<'_>) -> ComponentRow {
    let disk = input.extension_disk;
    let running = input.extension_running;
    let packaged = input.extension_packaged;

    match (running, disk) {
        // 两边都有：能对账，这是唯一能说"该重载了"的情形。
        (Some(run), Some(on_disk)) if run != on_disk => ComponentRow {
            id: ComponentId::Extension,
            version: on_disk.to_string(),
            state: ComponentState::NeedsAction(
                "浏览器里跑的还是旧版：请在扩展管理页点一次「重新加载」".to_string(),
            ),
            hint: format!("浏览器中运行 {run} · 磁盘上已是 {on_disk}"),
        },
        (Some(run), Some(on_disk)) => ComponentRow {
            id: ComponentId::Extension,
            version: on_disk.to_string(),
            state: ComponentState::Ok,
            hint: format!("浏览器中运行 {run}，与磁盘一致"),
        },
        (Some(run), None) => ComponentRow {
            id: ComponentId::Extension,
            version: run.to_string(),
            state: ComponentState::Ok,
            hint: "浏览器中正在运行；读不到磁盘上那份的版本".to_string(),
        },
        // 扩展没连上：可能装着只是浏览器没开，所以是未知而不是未安装。
        (None, Some(on_disk)) => ComponentRow {
            id: ComponentId::Extension,
            version: on_disk.to_string(),
            state: ComponentState::Unknown("浏览器未连接，无法对账".to_string()),
            hint: "磁盘上有扩展；打开浏览器并启用扩展后这里会显示运行版本".to_string(),
        },
        (None, None) => match packaged {
            Some(pkg) => ComponentRow {
                id: ComponentId::Extension,
                version: pkg.to_string(),
                state: ComponentState::Unknown("未检测到已装入浏览器".to_string()),
                hint: format!("本程序内置 {pkg}；需要用「加载已解压」在扩展管理页装一次"),
            },
            None => ComponentRow {
                id: ComponentId::Extension,
                version: "—".to_string(),
                state: ComponentState::Unknown("没有找到扩展文件".to_string()),
                hint: "本程序包里应带 browser/extension；没有的话这份安装不完整".to_string(),
            },
        },
    }
}

fn channel_row(input: &ComponentInput<'_>) -> ComponentRow {
    if let Some(err) = input.update_error.filter(|e| !e.trim().is_empty()) {
        return ComponentRow {
            id: ComponentId::Channel,
            version: input.latest.unwrap_or("—").to_string(),
            state: ComponentState::Unknown(format!("检查失败：{err}")),
            hint: "网络或清单地址的问题；点「立即检查」重试".to_string(),
        };
    }
    if input.channel_url.is_none() {
        return ComponentRow {
            id: ComponentId::Channel,
            version: "—".to_string(),
            state: ComponentState::Unknown("更新检查已关闭".to_string()),
            hint: "启动参数 --no-update-check，或设置里的「自动检查更新」被关掉".to_string(),
        };
    }
    let Some(latest) = input.latest else {
        return ComponentRow {
            id: ComponentId::Channel,
            version: "—".to_string(),
            state: ComponentState::Unknown("尚未检查".to_string()),
            hint: "点「立即检查」拉一次清单".to_string(),
        };
    };
    if latest == input.release {
        ComponentRow {
            id: ComponentId::Channel,
            version: latest.to_string(),
            state: ComponentState::Ok,
            hint: match input.checked_at {
                Some(at) => format!("已是最新；上次检查于 {}", ago_label(at)),
                None => "已是最新".to_string(),
            },
        }
    } else {
        ComponentRow {
            id: ComponentId::Channel,
            version: latest.to_string(),
            state: ComponentState::NeedsAction(format!("有新版 {latest}")),
            hint: "点「立即更新」走更新面板；它会先校验摘要再替换".to_string(),
        }
    }
}

/// 上次检查到现在多久。`now` 作参数传进来是为了能测（时钟不进纯函数）。
pub(crate) fn ago_label_at(checked_at: u64, now: u64) -> String {
    let secs = now.saturating_sub(checked_at);
    match secs {
        0..=59 => "刚刚".to_string(),
        60..=3599 => format!("{} 分钟前", secs / 60),
        3600..=86399 => format!("{} 小时前", secs / 3600),
        _ => format!("{} 天前", secs / 86400),
    }
}

fn ago_label(checked_at: u64) -> String {
    ago_label_at(checked_at, crate::queue::now_secs())
}

/// 「复制版本信息」用的那一行：**只放版本，不放状态**。
///
/// 贴到 issue 里时"未知"这种状态词是噪音，版本号才是要核对的东西。
pub(crate) fn versions_line(rows: &[ComponentRow]) -> String {
    rows.iter()
        .map(|r| format!("{} {}", r.id.title(), r.version))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// 状态栏悬停时那一行摘要：四个组件各一段。
///
/// 抽成纯函数是因为它是**唯一**一处能一眼看全四个数的地方 —— 拼错了没人会发现。
#[allow(dead_code)] // 状态栏悬停摘要：下一步接上
pub(crate) fn summary_line(rows: &[ComponentRow]) -> String {
    rows.iter()
        .map(|r| match &r.state {
            ComponentState::Ok => format!("{} {}", r.id.title(), r.version),
            ComponentState::NeedsAction(what) => {
                format!("{} {}（{what}）", r.id.title(), r.version)
            }
            ComponentState::Missing => format!("{} 未安装", r.id.title()),
            ComponentState::Unknown(why) => format!("{} 未知（{why}）", r.id.title()),
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>() -> ComponentInput<'a> {
        ComponentInput {
            app_name: "Download Manager",
            release: "0.7.0",
            core_current: "0.1.0",
            extension_disk: None,
            extension_running: None,
            extension_packaged: None,
            mcp_version: None,
            mcp_present: false,
            channel_url: Some("https://example.invalid/latest.json"),
            latest: Some("0.7.0"),
            checked_at: Some(1_700_000_000),
            update_error: None,
        }
    }

    /// 四行、顺序与 `ComponentId::ALL` 一致：界面靠这个顺序，乱一个就是错位。
    #[test]
    fn every_component_gets_exactly_one_row_in_a_stable_order() {
        let rows = build_rows(&input());
        let ids: Vec<ComponentId> = rows.iter().map(|r| r.id).collect();
        assert_eq!(ids, ComponentId::ALL.to_vec());
        assert_eq!(ComponentId::ALL.len(), 4, "四个组件，不多不少");
    }

    /// **措辞事故的防线**：探测不到 ≠ 没装。
    #[test]
    fn an_unreachable_component_is_unknown_not_missing() {
        // 扩展：磁盘上有，浏览器没连上。
        let mut i = input();
        i.extension_disk = Some("0.2.16");
        let rows = build_rows(&i);
        let ext = &rows[2];
        assert!(
            matches!(ext.state, ComponentState::Unknown(_)),
            "扩展没连上必须是未知，不能写成未安装：{:?}",
            ext.state
        );

        // MCP：文件在，但版本读不出来。
        let mut i = input();
        i.mcp_present = true;
        let rows = build_rows(&i);
        assert!(
            matches!(rows[1].state, ComponentState::Unknown(_)),
            "有 exe 却读不到版本时不能说未安装：{:?}",
            rows[1].state
        );
        assert_eq!(rows[1].version, "—", "读不到版本就写破折号，不编");

        // MCP：真的没有。
        let rows = build_rows(&input());
        assert_eq!(rows[1].state, ComponentState::Missing);
        assert_eq!(rows[1].version, "未安装");
    }

    /// 扩展三个版本数不一致 ⇒ 该重载了。这是唯一能说"需处理"的情形。
    #[test]
    fn a_reload_is_asked_for_only_when_running_and_disk_disagree() {
        let mut i = input();
        i.extension_running = Some("0.2.15");
        i.extension_disk = Some("0.2.16");
        let rows = build_rows(&i);
        match &rows[2].state {
            ComponentState::NeedsAction(what) => {
                assert!(what.contains("重新加载"), "要说清做什么：{what}");
            }
            other => panic!("版本不一致必须报需处理：{other:?}"),
        }
        assert_eq!(rows[2].version, "0.2.16", "版本列显示磁盘上那份（最新）");

        // 一致就是正常，别吓人。
        let mut i = input();
        i.extension_running = Some("0.2.16");
        i.extension_disk = Some("0.2.16");
        let rows = build_rows(&i);
        assert_eq!(rows[2].state, ComponentState::Ok);

        // 两个都读不到、包里也没带：未知。
        let mut i = input();
        i.extension_running = None;
        i.extension_disk = None;
        i.extension_packaged = None;
        let rows = build_rows(&i);
        assert!(matches!(rows[2].state, ComponentState::Unknown(_)));
    }

    #[test]
    fn an_installed_mcp_reports_its_version() {
        let mut i = input();
        i.mcp_present = true;
        i.mcp_version = Some("0.7.0");
        let rows = build_rows(&i);
        assert_eq!(rows[1].state, ComponentState::Ok);
        assert_eq!(rows[1].version, "0.7.0");
    }

    /// 更新通道：一致=正常、有新版=需处理、没地址=未知、报错=未知。
    #[test]
    fn the_channel_tells_latest_apart_from_checked_and_broken() {
        let rows = build_rows(&input());
        assert_eq!(rows[3].state, ComponentState::Ok);

        let mut i = input();
        i.latest = Some("0.8.0");
        let rows = build_rows(&i);
        assert!(matches!(rows[3].state, ComponentState::NeedsAction(_)));

        let mut i = input();
        i.channel_url = None;
        let rows = build_rows(&i);
        assert!(matches!(rows[3].state, ComponentState::Unknown(_)));

        let mut i = input();
        i.checked_at = None;
        i.latest = None;
        let rows = build_rows(&i);
        assert!(
            matches!(rows[3].state, ComponentState::Unknown(_)),
            "没查过不能谎称已是最新"
        );

        let mut i = input();
        i.update_error = Some("connection refused");
        let rows = build_rows(&i);
        assert!(matches!(rows[3].state, ComponentState::Unknown(_)));
    }

    /// 桌面端那行必须带上发行版与内核两个数（排查第一句就问这个）。
    #[test]
    fn the_desktop_row_carries_release_and_core() {
        let rows = build_rows(&input());
        assert!(rows[0].version.contains("0.7.0"), "{}", rows[0].version);
        assert!(
            rows[0].hint.contains("0.1.0"),
            "内核版本要露出来：{}",
            rows[0].hint
        );
    }

    /// 状态栏摘要：四个组件各一段，缺一个都算漏。
    #[test]
    fn the_summary_line_mentions_all_four_components() {
        let rows = build_rows(&input());
        let line = summary_line(&rows);
        for id in ComponentId::ALL {
            assert!(line.contains(id.title()), "摘要里缺 {}：{line}", id.title());
        }
        assert_eq!(line.matches('·').count(), 3, "四段之间三个分隔：{line}");
    }

    /// 「多久以前」只说人话，不吐秒数。
    #[test]
    fn the_ago_label_speaks_human_time() {
        assert_eq!(ago_label_at(1_000, 1_030), "刚刚");
        assert_eq!(ago_label_at(1_000, 1_000 + 120), "2 分钟前");
        assert_eq!(ago_label_at(1_000, 1_000 + 7_200), "2 小时前");
        assert_eq!(ago_label_at(1_000, 1_000 + 172_800), "2 天前");
        // 时钟回拨也不许 panic、不许出现"负几分钟"。
        assert_eq!(ago_label_at(1_000, 500), "刚刚");
    }

    /// 复制给 issue 的那一行：四个版本号都在，但不掺状态词。
    #[test]
    fn the_copy_line_carries_versions_without_state_words() {
        let mut i = input();
        i.extension_disk = Some("0.2.16");
        i.mcp_present = true;
        i.mcp_version = Some("0.7.0");
        let rows = build_rows(&i);
        let line = versions_line(&rows);
        for id in ComponentId::ALL {
            assert!(line.contains(id.title()), "缺 {}：{line}", id.title());
        }
        assert!(line.contains("0.2.16") && line.contains("0.7.0"), "{line}");
        for noise in ["正常", "未知", "未安装", "需处理"] {
            assert!(
                !line.contains(noise),
                "复制行不该带状态词「{noise}」：{line}"
            );
        }
    }
}
