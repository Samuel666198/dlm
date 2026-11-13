//! 落点命名：重命名模板（E1）与同名冲突（E2）。
//!
//! 这两件事都在**建任务那一刻**做完，之后的链路（进度、分段、恢复、打开文件夹）
//! 都只认一个最终文件名 —— 好处是"改了名"这件事只发生一次，不会下到一半改主意。
//!
//! ## 模板
//!
//! 认识四个变量：`{name}`（原名，不含扩展名）、`{ext}`（含点，可能为空）、
//! `{date}`（建任务当天的 `YYYY-MM-DD`）、`{n}`（同名冲突时自动编号，从 1 起）。
//! **不认识的变量原样保留**：宁可留个 `{foo}` 让人看见，也不悄悄吞掉。
//!
//! ## 同名冲突
//!
//! - `auto`（默认）：`a.zip` → `a (1).zip` → `a (2).zip`…
//! - `skip`：不建任务，回一句"同名文件已存在"
//! - `overwrite`：复用原名（调用方负责删旧文件，本模块只给名字）

use std::path::{Path, PathBuf};

/// 同名冲突策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnDuplicate {
    AutoNumber,
    Skip,
    Overwrite,
}

impl OnDuplicate {
    /// 从设置里的字符串解析。**认不出就退回 `auto`**：手改 JSON 写错不该让下载失败。
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_lowercase().as_str() {
            "skip" => Self::Skip,
            "overwrite" => Self::Overwrite,
            _ => Self::AutoNumber,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AutoNumber => "auto",
            Self::Skip => "skip",
            Self::Overwrite => "overwrite",
        }
    }
}

/// 拆原名成 `(主名, 扩展名含点)`。
pub fn split_name(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        // `.gitignore` 这种"点开头无扩展名"的情况：rfind 在 0 位置，不算扩展名。
        Some(0) | None => (name, ""),
        Some(i) => (&name[..i], &name[i..]),
    }
}

/// `{date}` 的取值：`YYYY-MM-DD`（**UTC**）。
///
/// 不引日期库：只需要"今天几号"，用 days-from-civil 的反函数算一次就够。
/// 用 UTC 而非本地时区：跨时区/夏令时会让同一文件在不同机器上得到不同名字，
/// 这里只要一个稳定标签。
pub fn date_string(secs_since_unix: u64) -> String {
    let days = (secs_since_unix / 86_400) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// 按模板渲染文件名。模板为空 = 原名。
pub fn render_template(template: &str, original: &str, date: &str, n: Option<u64>) -> String {
    let (stem, ext) = split_name(original);
    let tpl = template.trim();
    if tpl.is_empty() {
        return original.to_string();
    }
    let mut out = tpl
        .replace("{name}", stem)
        .replace("{ext}", ext.trim_start_matches('.'))
        .replace("{date}", date)
        .replace("{n}", &n.unwrap_or(1).to_string());
    // `{ext}` 不带点，模板里通常写成 `{name}.{ext}`；没有扩展名时别留一个孤零零的点。
    if ext.is_empty() {
        out = out.replace(&format!("{stem}."), stem);
    }
    out
}

/// 决定最终落点：返回 `Ok(名字)` 或 `Err(理由)`（只有 `skip` 会拒）。
///
/// `taken` 由调用方给（"这个名字已经被占了"），这样本函数不碰文件系统，能直接测。
pub fn resolve_duplicate(
    wanted: &str,
    policy: OnDuplicate,
    taken: impl Fn(&str) -> bool,
) -> Result<String, String> {
    if !taken(wanted) || policy == OnDuplicate::Overwrite {
        return Ok(wanted.to_string());
    }
    if policy == OnDuplicate::Skip {
        return Err(format!("同名文件已存在：{wanted}"));
    }
    let (stem, ext) = split_name(wanted);
    for i in 1..=9999u64 {
        let candidate = format!("{stem} ({i}){ext}");
        if !taken(&candidate) {
            return Ok(candidate);
        }
    }
    Err(format!("同名文件太多，编不出新名字了：{wanted}"))
}

/// 判断某个名字在目录里是否已被占（存在同名文件，或存在引擎的 `.part` 中间文件）。
pub fn taken_in(dir: &Path, name: &str) -> bool {
    let target: PathBuf = dir.join(name);
    target.exists() || target.with_extension(format!("{}.dlm", "part")).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_template_keeps_the_original_name() {
        assert_eq!(render_template("", "a.zip", "2026-11-13", None), "a.zip");
        assert_eq!(render_template("   ", "a.zip", "2026-11-13", None), "a.zip");
    }

    #[test]
    fn template_variables_are_substituted() {
        assert_eq!(
            render_template("{date}-{name}.{ext}", "movie.mkv", "2026-11-13", None),
            "2026-11-13-movie.mkv"
        );
        assert_eq!(
            render_template("{name} ({n}).{ext}", "a.zip", "x", Some(3)),
            "a (3).zip"
        );
        // 没有扩展名：不许留一个孤零零的点
        assert_eq!(
            render_template("{name}.{ext}", "README", "x", None),
            "README"
        );
        // 未知变量原样保留，让人看得见
        assert_eq!(
            render_template("{foo}-{name}", "a.zip", "x", None),
            "{foo}-a"
        );
    }

    #[test]
    fn duplicate_policy_auto_numbers_and_skip_refuses() {
        let taken = |name: &str| ["a.zip", "a (1).zip"].contains(&name);
        assert_eq!(
            resolve_duplicate("a.zip", OnDuplicate::AutoNumber, taken).unwrap(),
            "a (2).zip",
            "已经占了两个名字，要跳到 (2)"
        );
        assert!(resolve_duplicate("a.zip", OnDuplicate::Skip, taken).is_err());
        assert_eq!(
            resolve_duplicate("a.zip", OnDuplicate::Overwrite, taken).unwrap(),
            "a.zip"
        );
        assert_eq!(
            resolve_duplicate("fresh.zip", OnDuplicate::AutoNumber, taken).unwrap(),
            "fresh.zip",
            "没人占就用原名"
        );
    }

    #[test]
    fn policy_strings_round_trip_and_unknown_falls_back_to_auto() {
        for p in [
            OnDuplicate::AutoNumber,
            OnDuplicate::Skip,
            OnDuplicate::Overwrite,
        ] {
            assert_eq!(OnDuplicate::parse(p.as_str()), p);
        }
        assert_eq!(OnDuplicate::parse("nonsense"), OnDuplicate::AutoNumber);
    }

    #[test]
    fn dotfiles_have_no_extension() {
        assert_eq!(split_name(".gitignore"), (".gitignore", ""));
        assert_eq!(split_name("a.tar.gz"), ("a.tar", ".gz"));
    }
}
