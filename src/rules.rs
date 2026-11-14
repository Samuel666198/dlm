//! 下载规则（0.6.0 · 特色功能 2）：URL 命中规则就按规则落地。
//!
//! 只有一条匹配语义：**按顺序先命中先用**，和限速窗口一致 —— 界面上的顺序就是优先级，
//! 不做"最长匹配""最具体优先"这类需要解释的隐式规则。
//!
//! 匹配只用 `grab::matches`（只有 `*`），不引入正则：用户写 `*/releases/*` 就够用，
//! 正则的代价是写错时不报错、只是永远不命中。

use std::path::{Path, PathBuf};

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
///
/// 规则目录的两种写法都认，但**只有绝对路径原样用**：
/// - `D:\软件` ⇒ 就是这个目录；
/// - `视频` ⇒ 接到默认目录下面（`out_dir\视频`）。
///
/// 相对路径**不按进程当前目录解析**：GUI 的 cwd 没人说得清，按 cwd 拼出来的落点
/// 用户找不到（与 `settings::parse_out_dir_input` 拒绝相对路径同一个理由）。
pub fn dir_path_for(url: &str, rules: &[Rule], default_dir: &Path) -> PathBuf {
    match match_rule(url, rules).and_then(|r| r.dir.as_deref()) {
        // 空白当没写：界面里把目录清空会留下 `Some("")`，那是"清掉了"而不是"落到当前目录"。
        Some(dir) if !dir.trim().is_empty() => {
            let path = Path::new(dir.trim());
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                default_dir.join(path)
            }
        }
        _ => default_dir.to_path_buf(),
    }
}

/// 命中后实际该用的连接数：规则优先，否则回默认。
pub fn connections_for(url: &str, rules: &[Rule], default: usize) -> usize {
    match_rule(url, rules)
        .and_then(|r| r.connections)
        .unwrap_or(default)
}

/// 一条任务最终该用的（落点, 连接数）。
///
/// 把两处分词合成**一个出口**：以后加"按规则改文件名"这类字段时，
/// 只要改这里和 [`Rule`]，不会散落到 `runner` 的各个分支里。
///
/// 落点给的是 `PathBuf` 而不是 `String`：调用方（`runner`）拿它去 `join` 文件名、
/// 去判重、去 `--out-dir` 传子进程 —— 每一步都要 `Path`，
/// 留成字符串只会换来一串 `PathBuf::from`。
pub fn plan_for(
    url: &str,
    rules: &[Rule],
    default_dir: &Path,
    default_connections: usize,
) -> (PathBuf, usize) {
    (
        dir_path_for(url, rules, default_dir),
        connections_for(url, rules, default_connections),
    )
}

/// 界面上「上移 / 下移」一格。返回有没有真的动过。
///
/// 顺序就是优先级（先命中先用），所以"排在前面的规则赢"这件事必须能被用户改 ——
/// 而改顺序只能靠挪，不能靠改数字：数字会撞、会留空，挪不会。
///
/// 越界（已经在头/尾）返回 `false`，界面据此把按钮置灰。
pub fn move_rule(rules: &mut [Rule], index: usize, up: bool) -> bool {
    if index >= rules.len() {
        return false;
    }
    let target = if up {
        if index == 0 {
            return false;
        }
        index - 1
    } else {
        if index + 1 >= rules.len() {
            return false;
        }
        index + 1
    };
    rules.swap(index, target);
    true
}

/// 一条规则能不能用：通配不能是空的。
///
/// 空通配在匹配里永远为假（`grab::matches` 对空串直接 `false`），
/// 所以它不是"匹配一切"而是"什么都不匹配" —— 留在表里只会让人以为规则生效了。
pub fn validate_rule(rule: &Rule) -> Result<(), String> {
    if rule.pattern.trim().is_empty() {
        return Err("通配不能为空".to_string());
    }
    Ok(())
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
        assert_eq!(
            dir_path_for(url, &rules, Path::new("D:\\Downloads")),
            PathBuf::from("D:\\软件")
        );
        assert_eq!(connections_for(url, &rules, 4), 8, "第一条的连接数生效");
    }

    #[test]
    fn no_match_falls_back_to_defaults() {
        let rules = vec![rule("*.mp4", Some("D:\\视频"), Some(16))];
        assert_eq!(
            dir_path_for("https://x/a.zip", &rules, Path::new("D:\\Downloads")),
            PathBuf::from("D:\\Downloads")
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
            Path::new("D:\\Downloads"),
            4,
        );
        assert_eq!(dir, PathBuf::from("D:\\软件"));
        assert_eq!(conn, 8);

        let (dir, conn) = plan_for(
            "https://x.example/other/a.zip",
            &rules,
            Path::new("D:\\Downloads"),
            4,
        );
        assert_eq!((dir, conn), (PathBuf::from("D:\\Downloads"), 4));
    }

    #[test]
    fn a_rule_with_empty_fields_only_overrides_what_it_names() {
        let rules = vec![rule("*github.com*", None, Some(2))];
        let url = "https://github.com/a/b.zip";
        assert_eq!(
            dir_path_for(url, &rules, Path::new("D:\\Downloads")),
            PathBuf::from("D:\\Downloads")
        );
        assert_eq!(connections_for(url, &rules, 8), 2);
    }

    /// 规则里的相对目录**接到默认落点下面**，不按 cwd 解析：
    /// 否则同一条规则在 CLI（cwd = 某处）与 GUI（cwd 没人管）会落到两个目录。
    #[test]
    fn a_relative_rule_dir_hangs_under_the_default_dir() {
        let rules = vec![rule("*/releases/*", Some("视频"), None)];
        let url = "https://x.example/releases/a.mp4";
        assert_eq!(
            dir_path_for(url, &rules, Path::new("D:\\Downloads")),
            PathBuf::from("D:\\Downloads").join("视频")
        );
        // 绝对路径不受默认目录影响。
        let abs = vec![rule("*/releases/*", Some("E:\\影音"), None)];
        assert_eq!(
            dir_path_for(url, &abs, Path::new("D:\\Downloads")),
            PathBuf::from("E:\\影音")
        );
    }

    /// 挪一格就是改优先级：挪完，命中的必须是挪上来的那条。
    #[test]
    fn moving_a_rule_changes_which_one_wins() {
        let mut rules = vec![
            rule("*.zip", Some("D:\\压缩包"), None),
            rule("*/releases/*", Some("D:\\软件"), None),
        ];
        let url = "https://x.example/releases/a.zip";
        // 先命中 *.zip。
        assert_eq!(
            dir_path_for(url, &rules, Path::new("D:\\dl")),
            PathBuf::from("D:\\压缩包")
        );

        assert!(move_rule(&mut rules, 1, true), "第二条上移");
        assert_eq!(rules[0].pattern, "*/releases/*");
        assert_eq!(
            dir_path_for(url, &rules, Path::new("D:\\dl")),
            PathBuf::from("D:\\软件"),
            "挪上去的规则现在赢"
        );

        // 到头/到尾不动，且返回 false（界面据此把按钮置灰）。
        assert!(!move_rule(&mut rules, 0, true), "已经在第一条");
        assert!(!move_rule(&mut rules, 1, false), "已经在最后一条");
        assert!(!move_rule(&mut rules, 9, true), "下标越界不许 panic");
    }

    #[test]
    fn an_empty_pattern_is_rejected_because_it_matches_nothing() {
        assert!(validate_rule(&rule(" ", None, None)).is_err());
        assert!(validate_rule(&rule("*/x/*", None, None)).is_ok());
    }
}
