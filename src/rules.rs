//! 下载规则（0.6.0 · 特色功能 2）：URL 命中规则就按规则落地。
//!
//! 只有一条匹配语义：**按顺序先命中先用**，和限速窗口一致 —— 界面上的顺序就是优先级，
//! 不做"最长匹配""最具体优先"这类需要解释的隐式规则。
//!
//! 匹配只用 `grab::matches`（只有 `*`），不引入正则：用户写 `*/releases/*` 就够用，
//! 正则的代价是写错时不报错、只是永远不命中。

use crate::grab;
use serde::{Deserialize, Serialize};

/// 一条规则。空字段 = 不改这一项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    /// URL 通配（整条 URL 或路径都认）。
    pub pattern: String,
    /// 命中后落到这个目录；空 = 用默认目录。
    #[serde(default)]
    pub dir: Option<String>,
    /// 命中后的连接数；`None` = 用全局设置。
    #[serde(default)]
    pub connections: Option<usize>,
}

/// 命中的第一条规则。
pub fn match_rule<'a>(url: &str, rules: &'a [Rule]) -> Option<&'a Rule> {
    rules.iter().find(|r| grab::matches(&r.pattern, url))
}

/// 命中后实际该用的目录：规则优先，否则回默认。
pub fn dir_for<'a>(url: &str, rules: &'a [Rule], default_dir: &'a str) -> &'a str {
    match_rule(url, rules)
        .and_then(|r| r.dir.as_deref())
        .unwrap_or(default_dir)
}

/// 命中后实际该用的连接数：规则优先，否则回默认。
pub fn connections_for(url: &str, rules: &[Rule], default: usize) -> usize {
    match_rule(url, rules)
        .and_then(|r| r.connections)
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(pattern: &str, dir: Option<&str>, conn: Option<usize>) -> Rule {
        Rule {
            pattern: pattern.to_string(),
            dir: dir.map(str::to_string),
            connections: conn,
        }
    }

    #[test]
    fn first_matching_rule_wins_and_order_is_the_priority() {
        let rules = vec![
            rule("*/releases/*", Some("D:\\软件"), Some(8)),
            rule("*.zip", Some("D:\\压缩包"), None),
        ];
        let url = "https://x.example/releases/a.zip";
        assert_eq!(dir_for(url, &rules, "D:\\Downloads"), "D:\\软件");
        assert_eq!(connections_for(url, &rules, 4), 8, "第一条的连接数生效");
    }

    #[test]
    fn no_match_falls_back_to_defaults() {
        let rules = vec![rule("*.mp4", Some("D:\\视频"), Some(16))];
        assert_eq!(
            dir_for("https://x/a.zip", &rules, "D:\\Downloads"),
            "D:\\Downloads"
        );
        assert_eq!(connections_for("https://x/a.zip", &rules, 4), 4);
        assert!(match_rule("https://x/a.zip", &[]).is_none());
    }

    #[test]
    fn plan_for_folds_directory_and_connections_into_one_answer() {
        let rules = vec![rule("*/releases/*", Some("D:\\软件"), Some(8))];
        let (dir, conn) = plan_for(
            "https://x.example/releases/a.zip",
            &rules,
            "D:\\Downloads",
            4,
        );
        assert_eq!(dir, "D:\\软件");
        assert_eq!(conn, 8);

        let (dir, conn) = plan_for("https://x.example/other/a.zip", &rules, "D:\\Downloads", 4);
        assert_eq!((dir.as_str(), conn), ("D:\\Downloads", 4));
    }

    #[test]
    fn a_rule_with_empty_fields_only_overrides_what_it_names() {
        let rules = vec![rule("*github.com*", None, Some(2))];
        let url = "https://github.com/a/b.zip";
        assert_eq!(dir_for(url, &rules, "D:\\Downloads"), "D:\\Downloads");
        assert_eq!(connections_for(url, &rules, 8), 2);
    }
}
/// 一条任务最终该用的（目录, 连接数）。
///
/// 把两处分词合成**一个出口**：以后加"按规则改文件名"这类字段时，
/// 只要改这里和 [`Rule`]，不会散落到 `runner` 的各个分支里。
pub fn plan_for(
    url: &str,
    rules: &[Rule],
    default_dir: &str,
    default_connections: usize,
) -> (String, usize) {
    (
        dir_for(url, rules, default_dir).to_string(),
        connections_for(url, rules, default_connections),
    )
}
