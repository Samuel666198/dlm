"""从源图生成 dlm 的全部图标产物（一次性、可复现）。

## 为什么要脚本

图标有四个落点，必须**同一份像素**，否则资源管理器里、窗口标题栏、托盘里会长得不一样：

| 产物 | 谁用 |
|---|---|
| `assets/icon/dlm.ico` | exe 资源段（`build.rs` 编进 .res）+ 安装包 `SetupIconFile` |
| `assets/icon/dlm-256.png` | 文档/发布页展示，不参与运行时 |
| `assets/icon/dlm-64.rgba` | egui 窗口图标（`gui::window_icon`） |
| `assets/icon/dlm-32.rgba` | 托盘图标（`gui::app::tray::make_icon`） |

`.rgba` 是裸像素而不是 PNG：这个项目**没有引入图片解码依赖**（`Cargo.lock` 里连
`png` 都没有），运行时解码要么加一个 crate，要么把像素烘进仓库。选后者 —— 与
`updater` 直接调 `powershell` 解压同一个取舍：不为一件事引入依赖树。

## 源图处理

源图是 1254×1254 的 RGBA 图，squircle 外面有一圈**极淡的白色光晕**
（alpha 16–63，占 50% 面积）。那层光晕在浅色界面上看不出来，但在深色任务栏上会
变成一层灰雾，所以要裁掉：

1. 按 alpha ≥ 200 求主体 bbox（squircle 本体）
2. 扩成正方形并居中，留 2% 内边距（圆角不该贴到画布边缘）
3. alpha ≤ 64 的残留光晕置全透明，边缘抗锯齿（alpha 128+）保留

用法：
    python packaging/make-icon.py
    python packaging/make-icon.py --source assets/icon/source/dlm-icon.webp
"""

from __future__ import annotations

import argparse
import hashlib
import struct
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
ICON_DIR = ROOT / "assets" / "icon"
DEFAULT_SOURCE = ICON_DIR / "source" / "dlm-icon.webp"

# ico 里要放的尺寸。与 0.7.x 之前那份一致（见 docs/evidence/2026-11-12-安装包.md）。
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]

# 运行时烘出来的两档。
WINDOW_RGBA_SIZE = 64
TRAY_RGBA_SIZE = 32

# 光晕阈值：低于它的 alpha 一律当背景（源图实测：光晕 16–63，抗锯齿边 128+）。
HALO_ALPHA = 64

# 裁到主体后留的边距（占主体边长的比例）。
PAD_RATIO = 0.02


def prepare(source: Path) -> Image.Image:
    """裁到 squircle 主体、去掉光晕，返回正方形母版。"""
    im = Image.open(source).convert("RGBA")
    alpha = im.split()[3]

    # 主体 bbox：alpha >= 200 的那块。
    body = alpha.point(lambda v: 255 if v >= 200 else 0)
    bbox = body.getbbox()
    if bbox is None:
        raise SystemExit(f"源图里找不到不透明主体：{source}")

    left, top, right, bottom = bbox
    side = max(right - left, bottom - top)
    pad = round(side * PAD_RATIO)
    side += pad * 2

    # 以 bbox 中心为基准取正方形，允许越界（越界部分补全透明）。
    cx = (left + right) // 2
    cy = (top + bottom) // 2
    box = (cx - side // 2, cy - side // 2, cx - side // 2 + side, cy - side // 2 + side)

    out = Image.new("RGBA", (side, side), (0, 0, 0, 0))
    out.paste(im.crop(box), (0, 0))

    # 去掉残留光晕。做在「裁好之后」的图上，避免误伤主体内部的半透明像素。
    px = out.load()
    for y in range(side):
        for x in range(side):
            r, g, b, a = px[x, y]
            if a <= HALO_ALPHA:
                px[x, y] = (0, 0, 0, 0)

    return out


def resize(master: Image.Image, size: int) -> Image.Image:
    # LANCZOS：缩小时对细节（三支箭的圆头）保留得最好。
    return master.resize((size, size), Image.LANCZOS)


def write_rgba(master: Image.Image, size: int, dest: Path) -> int:
    """写裸 RGBA（行优先、每像素 4 字节）。返回字节数。"""
    flat = resize(master, size).tobytes("raw", "RGBA")
    dest.write_bytes(flat)
    return len(flat)


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--source", default=str(DEFAULT_SOURCE))
    args = ap.parse_args()

    source = Path(args.source)
    if not source.is_absolute():
        source = ROOT / source
    if not source.exists():
        raise SystemExit(f"源图不存在：{source}")

    ICON_DIR.mkdir(parents=True, exist_ok=True)
    master = prepare(source)
    print(f"master: {master.size[0]}x{master.size[1]}（已裁到主体并去光晕）")

    # 1) ico：把每一档单独画好再塞进去（Pillow 的 save(sizes=...) 会自己缩，
    #    但它对 256 之外的老式尺寸处理不一致，显式给 images 更稳）。
    frames = [resize(master, s) for s in ICO_SIZES]
    ico_path = ICON_DIR / "dlm.ico"
    frames[-1].save(
        ico_path,
        format="ICO",
        sizes=[(s, s) for s in ICO_SIZES],
        append_images=frames[:-1],
    )
    # Pillow 的 ICO 写出顺序不保证，校验一下每一档真的在。
    got = sorted({s for s in ICO_SIZES if s in _ico_sizes(ico_path)})
    if got != sorted(ICO_SIZES):
        raise SystemExit(f"ico 尺寸不全：期望 {ICO_SIZES}，实际 {got}")

    # 2) 256 png（文档/发布页用）
    png_path = ICON_DIR / "dlm-256.png"
    resize(master, 256).save(png_path, format="PNG")

    # 3) 运行时裸像素
    win_path = ICON_DIR / f"dlm-{WINDOW_RGBA_SIZE}.rgba"
    tray_path = ICON_DIR / f"dlm-{TRAY_RGBA_SIZE}.rgba"
    win_bytes = write_rgba(master, WINDOW_RGBA_SIZE, win_path)
    tray_bytes = write_rgba(master, TRAY_RGBA_SIZE, tray_path)

    for p, note in [
        (ico_path, f"{len(ICO_SIZES)} 档"),
        (png_path, "256x256"),
        (win_path, f"{WINDOW_RGBA_SIZE}x{WINDOW_RGBA_SIZE} 裸 RGBA，{win_bytes} 字节"),
        (tray_path, f"{TRAY_RGBA_SIZE}x{TRAY_RGBA_SIZE} 裸 RGBA，{tray_bytes} 字节"),
    ]:
        print(f"  {p.relative_to(ROOT)}  {note}  sha256={sha256(p)[:16]}…")

    # 两条自检：字节数必须等于 4*w*h；裸像素不许整张全透明（那等于没图标）。
    for size, path in [(WINDOW_RGBA_SIZE, win_path), (TRAY_RGBA_SIZE, tray_path)]:
        n = path.stat().st_size
        want = size * size * 4
        if n != want:
            raise SystemExit(f"{path.name} 字节数 {n} != {want}")
        if max(path.read_bytes()) == 0:
            raise SystemExit(f"{path.name} 全透明，图标没画上去")
    print("自检通过：字节数正确、像素非全透明")


def _ico_sizes(path: Path) -> set[int]:
    """读 ICO 目录表，返回里面实际含的尺寸（256 记作 0）。"""
    data = path.read_bytes()
    if len(data) < 6 or data[0:4] != b"\x00\x00\x01\x00":
        return set()
    (count,) = struct.unpack_from("<H", data, 4)
    out = set()
    for i in range(count):
        off = 6 + i * 16
        if off + 2 > len(data):
            break
        w = data[off]
        out.add(256 if w == 0 else w)
    return out


if __name__ == "__main__":
    main()
