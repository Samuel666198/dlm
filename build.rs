//! 构建脚本：把图标与版本信息塞进 Windows exe 的**资源段**。
//!
//! 只干这一件事，而且**不引入新依赖**：直接调 Windows SDK 里的 `rc.exe` 编一份 `.res`，
//! 再用 `rustc-link-arg-bins` 交给链接器。找不到 `rc.exe` 就只打一条 warning 继续 ——
//! 没有 SDK 的机器上仍然能编译、能跑，只是 exe 没有图标（不能让"没有图标"变成"构建不了"）。
//!
//! 图标为什么要进资源段：窗口/托盘图标是**运行时**画的（`gui::window_icon`、
//! `gui::app::tray::make_icon`），那只影响程序自己画出来的窗口；资源管理器里、开始菜单里、
//! 任务栏固定项里看到的是 **exe 文件自带的图标**，没有资源段就永远是那个白底默认图标。

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/icon/dlm.ico");
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(windows)]
    embed_icon_and_version();
}

#[cfg(windows)]
fn embed_icon_and_version() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let ico = manifest_dir.join("assets").join("icon").join("dlm.ico");

    if !ico.exists() {
        println!(
            "cargo:warning=缺少 {}，exe 将没有图标（用 packaging 或 tmp 里的绘图脚本重新生成）",
            ico.display()
        );
        return;
    }

    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    // FILEVERSION 要四个数，`0.1.0` 补成 `0,1,0,0`
    let mut nums: Vec<u32> = version
        .split('.')
        .filter_map(|p| p.parse::<u32>().ok())
        .collect();
    while nums.len() < 4 {
        nums.push(0);
    }
    let ver4 = format!("{},{},{},{}", nums[0], nums[1], nums[2], nums[3]);

    // .rc 里路径的反斜杠是转义字符，得写两份
    let ico_escaped = ico.display().to_string().replace('\\', "\\\\");

    let rc_body = format!(
        r#"// 由 build.rs 生成，不要手改
1 ICON "{ico_escaped}"
1 VERSIONINFO
FILEVERSION {ver4}
PRODUCTVERSION {ver4}
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "dlm"
      VALUE "FileDescription", "dlm download manager"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "dlm"
      VALUE "ProductName", "dlm"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 0x4b0
  END
END
"#
    );

    let rc_path = out_dir.join("dlm.rc");
    let res_path = out_dir.join("dlm.res");
    if std::fs::write(&rc_path, rc_body).is_err() {
        println!("cargo:warning=写 {} 失败，跳过图标", rc_path.display());
        return;
    }

    let Some(rc_exe) = find_rc() else {
        println!(
            "cargo:warning=找不到 rc.exe（Windows SDK），exe 将没有图标。装了 SDK 或设 RC=<路径> 即可"
        );
        return;
    };

    let ok = Command::new(&rc_exe)
        .arg("/nologo")
        .arg(format!("/fo{}", res_path.display()))
        .arg(&rc_path)
        .output();

    match ok {
        Ok(out) if out.status.success() && res_path.exists() => {
            // 只给 bins：测试/bench 的可执行文件不需要图标，少塞一份少一份噪音
            println!("cargo:rustc-link-arg-bins={}", res_path.display());
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            println!(
                "cargo:warning=rc.exe 失败（{}）：{}",
                out.status,
                stderr.trim()
            );
        }
        Err(err) => println!("cargo:warning=跑不起 rc.exe：{err}"),
    }
}

/// 找 `rc.exe`：先看 `RC` 环境变量，再扫 Windows Kits 下的最新 x64 版本。
#[cfg(windows)]
fn find_rc() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("RC") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    if let Ok(p) = std::env::var("WindowsSdkDir")
        && let Some(found) = newest_rc_under(&Path::new(&p).join("bin"))
    {
        return Some(found);
    }
    for base in [
        r"C:\Program Files (x86)\Windows Kits\10\bin",
        r"C:\Program Files\Windows Kits\10\bin",
    ] {
        if let Some(found) = newest_rc_under(Path::new(base)) {
            return Some(found);
        }
    }
    // 最后才认 PATH（PATH 上有就用）
    let probe = Command::new("rc.exe").arg("/?").output();
    if probe.is_ok() {
        return Some(PathBuf::from("rc.exe"));
    }
    None
}

/// `bin\<版本>\x64\rc.exe` 里挑版本号最大的那个（字符串倒序即可：10.0.26100.0 > 10.0.22621.0）。
#[cfg(windows)]
fn newest_rc_under(bin: &Path) -> Option<PathBuf> {
    let mut versions: Vec<PathBuf> = std::fs::read_dir(bin)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    versions.sort();
    versions.reverse();
    for v in versions {
        let candidate = v.join("x64").join("rc.exe");
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}
