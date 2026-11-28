//! 设置项的**目录表**（0.6.0 阶段 2）：分类 / 分组 / 标题 / 一句解释，以及每个键对应的
//! `Settings` 字段名。
//!
//! ## 为什么要把界面文字抽成数据
//!
//! 计划对阶段 2 的判据是「每个设置项都有归属分类，无『其它』兜底」。这句话以前根本没法
//! 检查 —— 界面是一串手写 `row(...)` 调用，漏一个字段不会红，只会在界面里"神秘消失"。
//! 现在它变成两条机械判据（见本文件末尾的用例）：
//!
//! 1. `Settings` 序列化出来的**每一个键**都必须在这张表里有且只有一条（`serde` 的键名即字段名）；
//! 2. 控件渲染是个对 [`SettingKey`] 穷举的 `match` —— 加了新键却忘了画控件，**编译器**就红。
//!
//! ## 分类只有这七个
//!
//! 下载 / 连接与速度 / 限速计划 / 代理 / Agent 接入 / 界面 / 高级。没有"其它"：
//! 一条设置说不清该归哪儿，就是分类还没想清楚，不该拿兜底挡住。

/// 设置项的分类（左侧那一列）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Category {
    Download,
    Connection,
    SpeedPlan,
    Proxy,
    Agent,
    Interface,
    /// 组件：桌面端（含内核）/ MCP 桥接 / 浏览器扩展 / 更新通道。
    ///
    /// 这一页是**状态面板**，没有可勾可填的设置项 —— 所以它在
    /// `every_spec_belongs_to_one_of_the_categories` 里是具名豁免。
    Component,
    Advanced,
}

impl Category {
    pub(crate) const ALL: &'static [Category] = &[
        Category::Download,
        Category::Connection,
        Category::SpeedPlan,
        Category::Proxy,
        Category::Agent,
        Category::Interface,
        Category::Component,
        Category::Advanced,
    ];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Category::Download => "下载",
            Category::Connection => "连接与速度",
            Category::SpeedPlan => "限速计划",
            Category::Proxy => "代理",
            Category::Agent => "Agent 接入",
            Category::Interface => "界面",
            Category::Component => "组件",
            Category::Advanced => "高级",
        }
    }
}

/// 每一个设置项的稳定键。控件渲染对它穷举 —— 漏一个编译就红。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum SettingKey {
    OutDir,
    CategoryDirs,
    RenameTemplate,
    OnDuplicate,
    OnComplete,
    ScanAfterDone,
    Connections,
    MaxParallel,
    PerHostConnections,
    MaxSpeedKiB,
    SpeedWindows,
    NightBatch,
    Proxy,
    AgentConfirm,
    Autostart,
    CloseToTray,
    Theme,
    SidebarCollapsed,
    TrayHint,
    TrayHintShown,
    Rules,
    GrabTemplates,
    AutoCheckUpdate,
    WatchClipboard,
    SuppressAutoLaunch,
    /// **不是**一个存盘的设置：只借设置页那一行放一个「导出诊断日志」按钮。
    ///
    /// 之所以混进来：设置页的每一行都由 `SettingSpec` 驱动，为了一个按钮
    /// 另造一套行渲染不划算；而 `setting_control` 里对这个 key 走的是按钮分支，
    /// 不会写进 `settings.json`（`key_string()` 返回空串，保存时跳过）。
    ExportDiag,
}

/// 一条设置的静态描述。
pub(crate) struct SettingSpec {
    pub key: SettingKey,
    /// `Settings` 里的字段名（= JSON 键名）。完整性用例靠它对账。
    pub field: &'static str,
    pub category: Category,
    /// 右侧分组卡片的标题。
    pub group: &'static str,
    pub title: &'static str,
    /// 标题下面那行小字：这一项到底干什么。
    pub hint: &'static str,
    /// 只读项：值是程序自己维护的，界面只显示不给改。
    pub read_only: bool,
}

const fn spec(
    key: SettingKey,
    field: &'static str,
    category: Category,
    group: &'static str,
    title: &'static str,
    hint: &'static str,
) -> SettingSpec {
    SettingSpec {
        key,
        field,
        category,
        group,
        title,
        hint,
        read_only: false,
    }
}

const fn internal(
    key: SettingKey,
    field: &'static str,
    category: Category,
    group: &'static str,
    title: &'static str,
    hint: &'static str,
) -> SettingSpec {
    SettingSpec {
        read_only: true,
        ..spec(key, field, category, group, title, hint)
    }
}

/// 全部设置项。**`Settings` 的每个字段都必须在这里出现一次**（用例守）。
pub(crate) const SPECS: &[SettingSpec] = &[
    // ---- 下载 ----
    spec(
        SettingKey::OutDir,
        "out_dir",
        Category::Download,
        "存放位置",
        "默认目录",
        "新任务落到这里；留空 = 系统下载文件夹",
    ),
    spec(
        SettingKey::CategoryDirs,
        "category_dirs",
        Category::Download,
        "存放位置",
        "按类型分子目录",
        "视频 / 音频 / 压缩包各进一个子目录；只影响新任务",
    ),
    spec(
        SettingKey::RenameTemplate,
        "rename_template",
        Category::Download,
        "命名",
        "重命名模板",
        "留空 = 原名；可用 {name} {ext} {date} {n}",
    ),
    spec(
        SettingKey::OnDuplicate,
        "on_duplicate",
        Category::Download,
        "命名",
        "同名文件",
        "目标位置已有同名文件时怎么办",
    ),
    spec(
        SettingKey::OnComplete,
        "on_complete",
        Category::Download,
        "完成后",
        "完成后动作",
        "留空 = 什么都不做；open 打开文件；run:程序.exe 跑一个程序",
    ),
    spec(
        SettingKey::ScanAfterDone,
        "scan_after_done",
        Category::Download,
        "完成后",
        "Defender 扫描",
        "用 Windows Defender 扫一遍；失败只记日志，不影响任务成败",
    ),
    // ---- 连接与速度 ----
    spec(
        SettingKey::MaxParallel,
        "max_parallel",
        Category::Connection,
        "连接",
        "同时下载的任务数",
        "一次跑几个任务；设成 1 就是排队一个个下",
    ),
    spec(
        SettingKey::Connections,
        "connections",
        Category::Connection,
        "连接",
        "每个任务的连接数",
        "单个任务开几条连接；过高容易被站点限流",
    ),
    spec(
        SettingKey::PerHostConnections,
        "per_host_connections",
        Category::Connection,
        "连接",
        "同一服务器上限",
        "同一个 host 上所有任务合计最多几条；0 = 不限",
    ),
    spec(
        SettingKey::MaxSpeedKiB,
        "max_speed_kib",
        Category::Connection,
        "速度",
        "全局限速 (KiB/s)",
        "0 = 不限速；限速计划命中时以窗口值为准",
    ),
    // ---- 限速计划 ----
    spec(
        SettingKey::SpeedWindows,
        "speed_windows",
        Category::SpeedPlan,
        "时段",
        "限速计划",
        "每行一条 22:00-06:00=512；命中窗口时覆盖全局限速，优先级看顺序",
    ),
    spec(
        SettingKey::NightBatch,
        "night_batch",
        Category::SpeedPlan,
        "夜间批次",
        "夜间批次",
        "一个开关串起三件事：只在夜里派发、夜里限速、跑完关机",
    ),
    // ---- 代理 ----
    spec(
        SettingKey::Proxy,
        "proxy",
        Category::Proxy,
        "代理",
        "代理地址",
        "http:// 或 socks5://；留空 = 直连",
    ),
    // ---- Agent 接入 ----
    spec(
        SettingKey::AgentConfirm,
        "agent_confirm",
        Category::Agent,
        "安全",
        "Agent 下发要确认",
        "Agent 加任务前先回对话问人；关掉之后 Agent 可以不经确认直接加",
    ),
    // ---- 界面 ----
    spec(
        SettingKey::CloseToTray,
        "close_to_tray",
        Category::Interface,
        "窗口",
        "关闭窗口时收进托盘",
        "关闭窗口时收进托盘；退出请用托盘菜单",
    ),
    spec(
        SettingKey::SidebarCollapsed,
        "sidebar_collapsed",
        Category::Interface,
        "窗口",
        "启动时收起侧栏",
        "仅保留图标条；点击栏头图标可随时展开",
    ),
    spec(
        SettingKey::TrayHint,
        "tray_hint",
        Category::Interface,
        "窗口",
        "收进托盘时提示",
        "第一次收进托盘时提醒一次，免得以为程序关了",
    ),
    internal(
        SettingKey::TrayHintShown,
        "tray_hint_shown",
        Category::Interface,
        "窗口",
        "已经提示过",
        "程序自己记的状态，不用管",
    ),
    spec(
        SettingKey::Theme,
        "theme",
        Category::Interface,
        "窗口",
        "主题",
        "跟随系统 / 浅色 / 深色；切换后立即生效",
    ),
    // **位置有讲究**：「窗口」组必须是连续的。
    // 之前 Theme 被排在 CloseToTray 之后、SidebarCollapsed 之前，中间夹了个「外观」，
    // 渲染时 `render_specs` 一见 group 变化就另起一张卡 —— 于是「窗口」被画成**两张同名卡**，
    // 中间隔着「外观」。用户 2026-11-17 报的就是这个。
    spec(
        SettingKey::Autostart,
        "autostart",
        Category::Interface,
        "启动",
        "开机自启",
        "与 --autostart-install 同源，这里只反映状态",
    ),
    // ---- 高级 ----
    spec(
        SettingKey::WatchClipboard,
        "watch_clipboard",
        Category::Advanced,
        "剪贴板",
        "监视剪贴板",
        "只匹配看起来像下载链接的文本，不上传任何内容",
    ),
    spec(
        SettingKey::Rules,
        "rules",
        Category::Advanced,
        "下载规则",
        "URL 规则",
        "命中规则就改落点或连接数；按顺序先命中先用（暂只支持改 settings.json）",
    ),
    spec(
        SettingKey::GrabTemplates,
        "grab_templates",
        Category::Advanced,
        "站点抓取",
        "抓取模板",
        "按站点记的深度与通配；在抓取窗口里「存为模板」就会记到这里",
    ),
    spec(
        SettingKey::SuppressAutoLaunch,
        "suppress_auto_launch",
        Category::Advanced,
        "浏览器",
        "抑制重复自动拉起",
        "浏览器下载发现桌面端没开时只自动打开一次；关掉则每次都尝试打开",
    ),
    spec(
        SettingKey::ExportDiag,
        "",
        Category::Advanced,
        "排查",
        "导出诊断日志",
        "把版本/设置/队列摘要/最近任务日志/依赖探测写进一个文本文件，方便反馈",
    ),
    spec(
        SettingKey::AutoCheckUpdate,
        "auto_check_update",
        Category::Advanced,
        "更新",
        "自动检查更新",
        "启动后查一次清单；关掉则一个外网请求都不发",
    ),
];

/// 某一类下的设置项（保持表里的顺序 = 界面上的顺序）。
pub(crate) fn specs_in(category: Category) -> impl Iterator<Item = &'static SettingSpec> {
    SPECS.iter().filter(move |s| s.category == category)
}

/// 搜索：标题、解释、分组名任一命中即可，大小写不敏感。空查询 = 全中。
pub(crate) fn matches(spec: &SettingSpec, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    spec.title.to_lowercase().contains(&q)
        || spec.hint.to_lowercase().contains(&q)
        || spec.group.to_lowercase().contains(&q)
        || spec.field.to_lowercase().contains(&q)
}

/// 搜索命中的设置项，跨分类（顺序仍按表）。
pub(crate) fn search(query: &str) -> Vec<&'static SettingSpec> {
    SPECS.iter().filter(|s| matches(s, query)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// **判据 1**：`Settings` 的每个字段都在这张表里，且只有一条。
    ///
    /// 靠 `serde` 拿字段名，不靠手抄 —— 手抄的清单和结构体一起漏掉同一个字段时谁都发现不了。
    #[test]
    fn every_settings_field_has_exactly_one_spec() {
        let value = serde_json::to_value(crate::gui::settings::Settings::default())
            .expect("Settings 要能序列化");
        let fields: BTreeSet<String> = value
            .as_object()
            .expect("Settings 序列化成对象")
            .keys()
            .cloned()
            .collect();
        // `field == ""` 的是**不是存盘项**的按钮行（如「导出诊断日志」）：
        // 它不写 settings.json，自然也不该出现在"字段对账"里。
        let spec_fields: BTreeSet<String> = SPECS
            .iter()
            .filter(|s| !s.field.is_empty())
            .map(|s| s.field.to_string())
            .collect();

        let missing: Vec<&String> = fields.difference(&spec_fields).collect();
        let extra: Vec<&String> = spec_fields.difference(&fields).collect();
        assert!(
            missing.is_empty(),
            "这些设置项没有归属（界面上会神秘消失）：{missing:?}"
        );
        assert!(
            extra.is_empty(),
            "表里有不存在的设置项（拼错了字段名？）：{extra:?}"
        );
        let persisted = SPECS.iter().filter(|s| !s.field.is_empty()).count();
        assert_eq!(
            persisted,
            fields.len(),
            "一个字段只能有一条：重复的条目会让界面上出现两个同名开关"
        );

        // 键也不能重复（否则穷举 match 之外的地方会认错条）。
        let keys: BTreeSet<SettingKey> = SPECS.iter().map(|s| s.key).collect();
        assert_eq!(keys.len(), SPECS.len(), "SettingKey 重复");
    }

    /// **判据 2**：每个条目都落在分类列表之一，没有"其它"兜底。
    #[test]
    fn every_spec_belongs_to_one_of_the_categories() {
        for spec in SPECS {
            assert!(
                Category::ALL.contains(&spec.category),
                "{} 落在一个不在列表里的分类",
                spec.field
            );
            assert!(
                !spec.category.title().contains("其它"),
                "分类列表里不许出现『其它』"
            );
            assert!(!spec.title.trim().is_empty(), "{} 没有标题", spec.field);
            assert!(
                !spec.hint.trim().is_empty(),
                "{} 没有一句解释（判据要求每行都有）",
                spec.field
            );
        }
        // 每个分类都要有内容：空分类等于把一条设置藏起来了。
        //
        // 「组件」是例外：那一页是一片**状态面板**（四个组件的版本、状态与动作），
        // 没有可勾可填的设置项。允许它空，但空得有名有姓 —— 不是漏了。
        for category in Category::ALL {
            if *category == Category::Component {
                continue;
            }
            assert!(
                specs_in(*category).next().is_some(),
                "分类「{}」是空的",
                category.title()
            );
        }
    }

    #[test]
    fn search_hits_title_hint_group_and_field_names() {
        let by_title = search("连接数");
        assert!(
            by_title.iter().any(|s| s.key == SettingKey::Connections),
            "标题要能搜到"
        );
        let by_hint = search("socks5");
        assert!(
            by_hint.iter().any(|s| s.key == SettingKey::Proxy),
            "解释里提到的关键词也要能搜到"
        );
        let by_field = search("max_speed_kib");
        assert_eq!(
            by_field.len(),
            1,
            "按字段名搜索是给「在 settings.json 里看到这个键的人」用的"
        );
        assert_eq!(search("").len(), SPECS.len(), "空查询 = 全部");
        assert!(search("绝对不存在的东西").is_empty());
    }

    /// 只读项也要有归属 —— 它们是"程序自己维护的状态"，但不许从分类里消失。
    #[test]
    fn internal_state_is_listed_but_marked_read_only() {
        let sh = SPECS
            .iter()
            .find(|s| s.field == "tray_hint_shown")
            .expect("tray_hint_shown 要在表里");
        assert!(sh.read_only, "它是程序自己记的，界面不给改");
    }
    /// **同一分类里，同名的分组必须是连续的。**
    ///
    /// 渲染层 `render_specs` 一见 `group` 变化就另起一张卡，所以一张卡里放不下
    /// 被别的组打断的字段。0.7.0 的「窗口」就吃过这个亏：`Theme`（当时 group 是
    /// 「外观」）夹在 `CloseToTray` 与 `SidebarCollapsed` 之间，于是「窗口」被画成
    /// **两张同名卡**。用户 2026-11-17 报的就是这个。
    ///
    /// 这条用例守的是"字段顺序"，而不是渲染代码 —— 因为错在顺序，不在渲染。
    #[test]
    fn a_group_is_never_split_by_another_group_in_the_same_category() {
        for category in Category::ALL {
            let groups: Vec<&str> = specs_in(*category).map(|s| s.group).collect();
            // 每个组名出现的位置必须是一段连续区间。
            let mut seen: Vec<&str> = Vec::new();
            let mut closed: Vec<&str> = Vec::new();
            for g in &groups {
                if seen.last() == Some(g) {
                    continue;
                }
                if seen.contains(g) {
                    panic!(
                        "分类「{}」的「{g}」被别的组打断了 —— 界面上会裂成两张同名卡。\
                         当前顺序：{groups:?}",
                        category.title()
                    );
                }
                if let Some(last) = seen.last() {
                    closed.push(last);
                }
                seen.push(g);
            }
        }
    }

    /// 「窗口」这一组必须装下全部窗口相关项，且与「外观」分开后不再裂开。
    #[test]
    fn the_window_group_holds_everything_window_related() {
        let window: Vec<&str> = specs_in(Category::Interface)
            .filter(|s| s.group == "窗口")
            .map(|s| s.title)
            .collect();
        for expected in ["关闭窗口时收进托盘", "启动时收起侧栏", "主题"] {
            assert!(
                window.contains(&expected),
                "「窗口」里缺「{expected}」：{window:?}"
            );
        }
        // 主题属于"窗口"，不该自成一个组 —— 自成一个组就会把「窗口」劈成两半。
        assert!(
            specs_in(Category::Interface).all(|s| s.group != "外观"),
            "「外观」这个组名已经并回「窗口」，不该再出现"
        );
    }
}
