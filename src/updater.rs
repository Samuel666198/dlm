//! 更新器（`dlm-updater.exe`）的可测部分：下载、校验、解压、替换、回滚。
//!
//! ## 为什么要独立进程
//!
//! Windows 上**正在运行的程序不能覆盖自己的 exe**。所以更新一定是：
//! `dlm-gui` 发现新版 → 用户点确认 → 启动 `dlm-updater.exe` → `dlm-gui` 退出
//! → updater 替换文件 → 重启 `dlm-gui`。updater 自己是**从别处的副本**跑起来的
//! （`dlm-gui` 先把它拷到临时目录），否则它也没法替换自己。
//!
//! ## 为什么每一步都要能回滚
//!
//! 替换到一半失败会留下「两个新 exe + 一个旧 exe」这种半新半旧的安装 —— 那比起没更新更糟。
//! 做法：旧的先 `rename` 成 `<名字>.dlm-old`（同盘改名不搬字节），全部换成功才删备份；
//! 任何一步失败就把备份改回去。`rename` 在文件被占用时会失败，于是「GUI 还没退干净」
//! 这件事会在这里被挡住，而不是留下一个装了一半的目录。
//!
//! ## 为什么校验摘要不自己做
//!
//! `engine::sha256_of_path` / `hash_matches` 已经存在，且 `hash_matches` 的判据
//! （大小写不敏感、空期望**永不放行**）是下载侧钉过的 —— 更新器复用同一套判据，
//! 「一条恒真的闸比没有闸更坏」这句话就只需要在一个地方成立。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::engine::{hash_matches, sha256_of_path};

/// 安装标记文件名：装在 `queue.json` / `settings.json` 旁边。
pub const INSTALLED_FILE: &str = "installed.json";

/// 更新的三个 exe（`dlm-updater.exe` 也算：它由 updater 自己一同换掉）。
pub const BINARIES: &[&str] = &["dlm.exe", "dlm-gui.exe", "dlm-native-host.exe"];

/// 更新包里的扩展目录（相对包根）。
pub const EXTENSION_IN_PACKAGE: &str = "browser/extension";

/// 「这一版装的是哪些东西」的记录。
///
/// 为什么要有它：`dlm-gui` 的编译期版本号只在**刚编译出来的那一刻**是对的。
/// 装过一次更新之后，磁盘上的 exe 是新的，而编译期常量还是旧的 ——
/// 没有这份记录，刚装上新版的应用会认为自己还是旧版，每次启动都提示一次「有更新」。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    pub release: String,
    pub core: String,
    pub extension: String,
    pub installed_at: u64,
}

/// 本机现状：发行版版本 + 磁盘上的扩展版本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalState {
    pub release: String,
    /// 磁盘上扩展的 `manifest.version`；读不到就是 `None`（还没装过扩展）。
    pub extension: Option<String>,
}

/// 读 `installed.json`。不存在或读不懂都当「没有」——不能因为一个标记文件坏了就起不来。
pub fn read_installed(dir: &Path) -> Option<Installed> {
    let raw = fs::read(dir.join(INSTALLED_FILE)).ok()?;
    serde_json::from_slice(&raw).ok()
}

/// 原子写（tmp + rename）：断电不会留半截标记。
pub fn write_installed(dir: &Path, installed: &Installed) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("建数据目录失败：{}", dir.display()))?;
    let path = dir.join(INSTALLED_FILE);
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(installed)?)
        .with_context(|| format!("写安装标记失败：{}", tmp.display()))?;
    fs::rename(&tmp, &path).with_context(|| format!("提交安装标记失败：{}", path.display()))?;
    Ok(())
}

/// 磁盘上那份扩展的版本（读 `manifest.json`）。读不到就是 `None`。
pub fn extension_version(ext_dir: &Path) -> Option<String> {
    let raw = fs::read(ext_dir.join("manifest.json")).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    value
        .get("version")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// 本机现状。发行版版本优先取 `installed.json`，没有就退回编译期常量
/// （也就是「这一份是从源码装的、从没更新过」）。
pub fn local_state(data_dir: &Path, ext_dir: &Path) -> LocalState {
    LocalState {
        release: read_installed(data_dir)
            .map(|i| i.release)
            .unwrap_or_else(|| crate::gui::RELEASE.to_string()),
        extension: extension_version(ext_dir),
    }
}

/// 下载到 `dest`（覆盖），返回字节数。
///
/// 超时给 5 分钟：更新包将近 10 MB，慢网也该够；而「卡住不动」不该无限等。
pub fn download(url: &str, dest: &Path) -> Result<u64> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(300))
        .connect_timeout(Duration::from_secs(15))
        .build()
        .context("建下载客户端失败")?;
    let mut response = client
        .get(url)
        .send()
        .with_context(|| format!("请求更新包失败：{url}"))?;
    if !response.status().is_success() {
        bail!("更新包下载失败：HTTP {}", response.status());
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("建临时目录失败：{}", parent.display()))?;
    }
    let mut file =
        fs::File::create(dest).with_context(|| format!("创建临时文件失败：{}", dest.display()))?;
    let bytes = response
        .copy_to(&mut file)
        .with_context(|| format!("写更新包失败：{}", dest.display()))?;
    file.flush().context("刷新更新包到底层失败")?;
    Ok(bytes)
}

/// 校验摘要，返回**算得的值**（写日志用）。
///
/// 不符就 `Err`，且错误文案里带着两个值 —— 用户报「校验失败」时，
/// 我们需要的正是这两个十六进制串。
pub fn verify_file(path: &Path, expected: &str) -> Result<String> {
    let got = sha256_of_path(path)?;
    if !hash_matches(&got, expected) {
        bail!("sha256 不符：算得 {got}，期望 {}", expected.trim());
    }
    Ok(got)
}

/// 解压更新包到 `dest`。
///
/// ## 为什么直接调 .NET，而不是 `Expand-Archive`
///
/// 实测（2026-11-12）：Windows PowerShell **5.1**（就是 `powershell.exe`）的
/// `Expand-Archive` 在这台机器上会走进调用
/// `ExtractToDirectory(src, dest, overwrite)` 的路径，而 .NET Framework 里**没有三参数重载**，
/// 于是报 `Cannot find an overload for "ExtractToDirectory" and the argument count: "3"`。
/// 加不加 `-Force`、目标目录建没建，都躲不开（两种都试过）。
///
/// 直接调两参数的 `[System.IO.Compression.ZipFile]::ExtractToDirectory`：
/// .NET Framework 4.5+ 就有，5.1 与 pwsh 7 实测都通。
///
/// 为什么不用 `zip` crate：打包侧本来就是 `Compress-Archive`，解压用同一套工具最省心，
/// 也不用为一件事同时引入一个 crate 和它的传递依赖。代价是必须能跑 `powershell.exe` ——
/// Windows 上一定有。
///
/// `dest` 必须**不存在**：解压库对「已存在的目录」的处理各版本不一致，先删干净最省事。
pub fn extract_zip(zip: &Path, dest: &Path) -> Result<()> {
    if dest.exists() {
        fs::remove_dir_all(dest)
            .with_context(|| format!("清空解压目录失败：{}", dest.display()))?;
    }
    let script = format!(
        "$ErrorActionPreference='Stop'; \
         Add-Type -AssemblyName System.IO.Compression.FileSystem; \
         [System.IO.Compression.ZipFile]::ExtractToDirectory('{}','{}')",
        ps_quote(zip),
        ps_quote(dest)
    );
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .output()
        .context("起 powershell 解压失败")?;
    if !output.status.success() {
        bail!(
            "解压失败（exit {}）：{}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// 把路径塞进 PowerShell 单引号串：只需要把 `'` 写成 `''`。
fn ps_quote(path: &Path) -> String {
    path.display().to_string().replace('\'', "''")
}

/// 把 `staging` 里的 `names` 换进 `install_dir`，失败全部回滚。
///
/// 顺序：先确认 staging 里**每一个**都在（缺一个就不动手，免得换一半），
/// 再逐个「旧改名 → 新拷入」。成功后删备份；失败就把备份改回去。
pub fn swap_binaries(install_dir: &Path, staging: &Path, names: &[&str]) -> Result<()> {
    for name in names {
        if !staging.join(name).is_file() {
            bail!("更新包不完整：缺 {name}");
        }
    }
    // (文件名, 备份路径)。备份路径为空 = 这个文件原本不存在，回滚时删掉新拷入的。
    let mut swapped: Vec<(&str, PathBuf)> = Vec::new();
    let mut outcome: Result<()> = Ok(());
    for name in names {
        let target = install_dir.join(name);
        let backup = install_dir.join(format!("{name}.dlm-old"));
        let _ = fs::remove_file(&backup);
        if target.is_file() {
            // 被占用（GUI 还没退干净）时这里就会失败 —— 正是我们要挡的地方。
            if let Err(err) = fs::rename(&target, &backup) {
                outcome = Err(err).with_context(|| {
                    format!(
                        "旧的 {name} 挪不动（程序可能还在运行）：{}",
                        target.display()
                    )
                });
                break;
            }
            swapped.push((name, backup));
        } else {
            swapped.push((name, PathBuf::new()));
        }
        if let Err(err) = fs::copy(staging.join(name), &target) {
            outcome =
                Err(err).with_context(|| format!("拷入新的 {name} 失败：{}", target.display()));
            break;
        }
    }
    match outcome {
        Ok(()) => {
            for (_, backup) in &swapped {
                if !backup.as_os_str().is_empty() {
                    let _ = fs::remove_file(backup);
                }
            }
            Ok(())
        }
        Err(err) => {
            let restored = rollback(install_dir, &swapped);
            if restored > 0 {
                bail!("{err:#}（已回滚 {restored} 个文件到旧版本）");
            }
            Err(err)
        }
    }
}

/// 把 `swapped` 里的文件恢复成旧的，返回恢复了几个。
///
/// 倒序恢复：正常路径是「旧的挪走 → 新的拷进来」，反过来就是「删掉新的 → 旧的挪回来」。
fn rollback(install_dir: &Path, swapped: &[(&str, PathBuf)]) -> usize {
    let mut restored = 0;
    for (name, backup) in swapped.iter().rev() {
        let target = install_dir.join(name);
        let _ = fs::remove_file(&target);
        if !backup.as_os_str().is_empty() && backup.is_file() && fs::rename(backup, &target).is_ok()
        {
            restored += 1;
        }
    }
    restored
}

/// 把更新包里的扩展文件装到 `ext_dir`，返回拷了几个文件。
///
/// **会删掉旧的 `background-<版本>.js`**：扩展的 service worker 文件名带版本号
/// （为了绕缓存），不删就会在目录里堆一串 0.2.11 / 0.2.12 / … 的旧文件。
pub fn install_extension(staging_root: &Path, ext_dir: &Path) -> Result<usize> {
    let staged = staging_root.join(EXTENSION_IN_PACKAGE);
    if !staged.is_dir() {
        bail!("更新包里没有 {EXTENSION_IN_PACKAGE}：不装");
    }
    fs::create_dir_all(ext_dir)
        .with_context(|| format!("建扩展目录失败：{}", ext_dir.display()))?;
    // 以更新包里的 manifest 为准：它指哪个 service_worker，哪个才是该留的。
    let keep = fs::read(staged.join("manifest.json"))
        .ok()
        .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok())
        .and_then(|v| {
            v.get("background")
                .and_then(|b| b.get("service_worker"))
                .and_then(|s| s.as_str())
                .map(str::to_string)
        });
    let mut copied = 0;
    for entry in fs::read_dir(&staged).with_context(|| format!("读 {} 失败", staged.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name();
        fs::copy(&path, ext_dir.join(&name))
            .with_context(|| format!("拷扩展文件失败：{}", path.display()))?;
        copied += 1;
    }
    if let Some(keep) = keep {
        for entry in
            fs::read_dir(ext_dir).with_context(|| format!("读 {} 失败", ext_dir.display()))?
        {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("background-") && name.ends_with(".js") && name != keep {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dlm-updater-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn installed_state_roundtrips_and_a_broken_file_is_the_same_as_none() {
        let dir = tmp_dir("installed");
        assert_eq!(read_installed(&dir), None, "没有标记文件 = 从没更新过");
        let state = Installed {
            release: "1.0.0".into(),
            core: "0.1.1".into(),
            extension: "0.2.13".into(),
            installed_at: 1700000000,
        };
        write_installed(&dir, &state).unwrap();
        assert_eq!(read_installed(&dir), Some(state));

        fs::write(dir.join(INSTALLED_FILE), b"not json").unwrap();
        assert_eq!(read_installed(&dir), None, "坏标记不许让程序起不来");
        let _ = fs::remove_dir_all(dir);
    }

    /// 装过一次更新之后，「本机版本」必须来自 `installed.json` 而不是编译期常量 ——
    /// 否则刚装上新版的应用每次启动都提示「有更新」。
    #[test]
    fn the_local_release_comes_from_the_installed_marker_not_the_compile_time_constant() {
        let dir = tmp_dir("release");
        let ext = dir.join("extension");
        fs::create_dir_all(&ext).unwrap();
        fs::write(ext.join("manifest.json"), br#"{"version":"0.2.13"}"#).unwrap();

        let fresh = local_state(&dir, &ext);
        assert_eq!(
            fresh.release,
            crate::gui::RELEASE,
            "从源码装的：用编译期常量"
        );
        assert_eq!(
            fresh.extension.as_deref(),
            Some("0.2.13"),
            "磁盘上的扩展版本"
        );

        write_installed(
            &dir,
            &Installed {
                release: "1.0.0".into(),
                core: "0.1.1".into(),
                extension: "0.2.13".into(),
                installed_at: 1,
            },
        )
        .unwrap();
        assert_eq!(local_state(&dir, &ext).release, "1.0.0");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn verify_file_names_both_digests_and_never_passes_an_empty_expectation() {
        let dir = tmp_dir("verify");
        let file = dir.join("a.bin");
        fs::write(&file, b"abc").unwrap();
        // `abc` 的 sha256，与 engine 的判据同一套。
        let want = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(verify_file(&file, want).unwrap(), want);
        assert!(
            verify_file(&file, &want.to_uppercase()).is_ok(),
            "大小写不算不符（与 --sha256 同一判据）"
        );
        let err = verify_file(&file, "").unwrap_err().to_string();
        assert!(err.contains("sha256"), "{err}");
        let err = verify_file(&file, "deadbeef").unwrap_err().to_string();
        assert!(err.contains(want), "错误里要带上算得的值：{err}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn swap_replaces_every_binary_and_leaves_no_backup_behind() {
        let dir = tmp_dir("swap-ok");
        let install = dir.join("install");
        let stage = dir.join("stage");
        fs::create_dir_all(&install).unwrap();
        fs::create_dir_all(&stage).unwrap();
        for name in BINARIES {
            fs::write(install.join(name), b"old").unwrap();
            fs::write(stage.join(name), b"new").unwrap();
        }
        swap_binaries(&install, &stage, BINARIES).unwrap();
        for name in BINARIES {
            assert_eq!(
                fs::read(install.join(name)).unwrap(),
                b"new",
                "{name} 应换成新版本"
            );
            assert!(
                !install.join(format!("{name}.dlm-old")).exists(),
                "{name} 的备份应已删除"
            );
        }
        let _ = fs::remove_dir_all(dir);
    }

    /// 缺一个文件就**一个都不换**：换一半比没更新更糟。
    #[test]
    fn swap_refuses_before_touching_anything_when_the_package_is_incomplete() {
        let dir = tmp_dir("swap-missing");
        let install = dir.join("install");
        let stage = dir.join("stage");
        fs::create_dir_all(&install).unwrap();
        fs::create_dir_all(&stage).unwrap();
        for name in BINARIES {
            fs::write(install.join(name), b"old").unwrap();
        }
        fs::write(stage.join(BINARIES[0]), b"new").unwrap();
        let err = swap_binaries(&install, &stage, BINARIES)
            .unwrap_err()
            .to_string();
        assert!(err.contains("不完整"), "{err}");
        for name in BINARIES {
            assert_eq!(
                fs::read(install.join(name)).unwrap(),
                b"old",
                "{name} 不该被动过"
            );
        }
        let _ = fs::remove_dir_all(dir);
    }

    /// 回滚：把第二个文件的新版本塞成「目录」，让 `copy` 必失败，验第一个文件回到旧内容。
    #[test]
    fn a_failed_swap_rolls_the_already_replaced_files_back() {
        let dir = tmp_dir("swap-rollback");
        let install = dir.join("install");
        let stage = dir.join("stage");
        fs::create_dir_all(&install).unwrap();
        fs::create_dir_all(&stage).unwrap();
        for name in BINARIES {
            fs::write(install.join(name), b"old").unwrap();
            fs::write(stage.join(name), b"new").unwrap();
        }
        // 目标位置换成目录 ⇒ 拷不进去（Windows 与 Unix 上都会失败）。
        fs::remove_file(install.join(BINARIES[1])).unwrap();
        fs::create_dir_all(install.join(BINARIES[1])).unwrap();

        let err = swap_binaries(&install, &stage, BINARIES)
            .unwrap_err()
            .to_string();
        assert!(err.contains("回滚"), "{err}");
        assert_eq!(
            fs::read(install.join(BINARIES[0])).unwrap(),
            b"old",
            "已经换掉的第一个文件必须回滚"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn install_extension_copies_files_and_drops_the_previous_service_worker() {
        let dir = tmp_dir("ext");
        let stage = dir.join("stage");
        let staged_ext = stage.join(EXTENSION_IN_PACKAGE);
        let ext = dir.join("extension");
        fs::create_dir_all(&staged_ext).unwrap();
        fs::create_dir_all(&ext).unwrap();
        fs::write(
            staged_ext.join("manifest.json"),
            br#"{"version":"0.2.12","background":{"service_worker":"background-0.2.12.js"}}"#,
        )
        .unwrap();
        fs::write(staged_ext.join("background-0.2.12.js"), b"new sw").unwrap();
        fs::write(staged_ext.join("content.js"), b"new content").unwrap();
        // 旧安装里的残留
        fs::write(ext.join("background-0.2.11.js"), b"old sw").unwrap();
        fs::write(ext.join("content.js"), b"old content").unwrap();

        let copied = install_extension(&stage, &ext).unwrap();
        assert_eq!(copied, 3, "manifest + sw + content");
        assert_eq!(fs::read(ext.join("content.js")).unwrap(), b"new content");
        assert!(ext.join("background-0.2.12.js").exists());
        assert!(
            !ext.join("background-0.2.11.js").exists(),
            "旧的 service worker 要清掉，否则目录里堆一串版本"
        );
        assert_eq!(
            extension_version(&ext).as_deref(),
            Some("0.2.12"),
            "装完磁盘版本就该是新版本"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn install_extension_refuses_a_package_without_the_extension() {
        let dir = tmp_dir("ext-missing");
        let err = install_extension(&dir, &dir.join("extension"))
            .unwrap_err()
            .to_string();
        assert!(err.contains(EXTENSION_IN_PACKAGE), "{err}");
        let _ = fs::remove_dir_all(dir);
    }

    /// 真打一个 zip 再解压：钉住「打包用 Compress-Archive、解压用 Expand-Archive」这条约定。
    #[test]
    fn a_real_zip_roundtrips_through_expand_archive() {
        let dir = tmp_dir("zip");
        let src = dir.join("src");
        fs::create_dir_all(src.join("browser").join("extension")).unwrap();
        fs::write(src.join("dlm.exe"), b"binary").unwrap();
        fs::write(
            src.join("browser").join("extension").join("manifest.json"),
            b"{}",
        )
        .unwrap();
        let zip = dir.join("pkg.zip");
        let packed = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &format!(
                    "Compress-Archive -Path '{}' -DestinationPath '{}' -Force",
                    src.join("*").display(),
                    zip.display()
                ),
            ])
            .output()
            .expect("起 powershell 打包");
        assert!(packed.status.success(), "打包失败：{packed:?}");
        assert!(zip.is_file());

        let out = dir.join("out");
        extract_zip(&zip, &out).unwrap();
        assert!(out.join("dlm.exe").is_file(), "解压后应有 exe");
        assert!(
            out.join("browser")
                .join("extension")
                .join("manifest.json")
                .is_file(),
            "解压后应有扩展目录"
        );
        let _ = fs::remove_dir_all(dir);
    }
}
