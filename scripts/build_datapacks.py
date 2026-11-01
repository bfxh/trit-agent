#!/usr/bin/env python3
"""打包并安装数据包。

    python scripts/build_datapacks.py                 # 只打包 -> dist/
    python scripts/build_datapacks.py --install <profile>
                                                     # 打包 + 装进 Minecraft 实例

`<profile>` 是启动器的实例目录，例如：
    E:/MC/MC/profiles/烦人的村民

会装到：
    <profile>/saves/*/datapacks/     数据包（每个存档都装）
    <profile>/resourcepacks/         语言包
"""
from __future__ import annotations

import argparse
import json
import pathlib
import re
import shutil
import sys
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
SRC = ROOT / "datapacks"
DIST = ROOT / "dist"

# 数据包 pack_format：1.18.2 是 9（1.18 / 1.18.1 才是 8）
DATA_PACK_FORMAT = 9
# 资源包 pack_format：1.18.2 是 8
RESOURCE_PACK_FORMAT = 8

RESOURCE_PACKS = {"summoner_lang"}


def validate_json(zf: zipfile.ZipFile) -> None:
    for n in zf.namelist():
        if n.endswith((".json", ".mcmeta")):
            json.loads(zf.read(n).decode("utf-8"))


FUNC_REF = re.compile(r"\bfunction\s+([a-z0-9_]+:[a-z0-9_/]+)")


def validate_function_refs(name: str) -> list[str]:
    """检查 mcfunction 里 `function ns:path` 的引用是否指向真实存在的函数。

    只校验**同一个命名空间**里的引用 —— 跨数据包的引用（比如 squad 里的
    `function trit_bridge:on`）是合法的，查不了。

    这条检查是有来历的：改意图指令文本时改漏一处，游戏里表现是「点了没反应」，
    而且只在函数被调用那一刻才报错，肉眼根本追不到。
    """
    src = SRC / name
    data = src / "data"
    if not data.is_dir():
        return []

    have: set[str] = set()
    for ns in data.iterdir():
        fdir = ns / "functions"
        if fdir.is_dir():
            for p in fdir.rglob("*.mcfunction"):
                have.add(f"{ns.name}:{p.relative_to(fdir).with_suffix('').as_posix()}")

    problems: list[str] = []
    for p in data.rglob("*.mcfunction"):
        text = p.read_text(encoding="utf-8", errors="replace")
        # 去掉注释行，注释里写 `function xxx` 是说明不是调用
        body = "\n".join(l for l in text.splitlines() if not l.lstrip().startswith("#"))
        for m in FUNC_REF.finditer(body):
            ref = m.group(1)
            if ref.split(":")[0] not in {h.split(":")[0] for h in have}:
                continue
            if ref not in have:
                problems.append(f"{p.name} -> {ref}")
    return problems


def build_one(name: str) -> pathlib.Path:
    src = SRC / name
    out = DIST / f"{name}.zip"
    if not src.is_dir():
        raise SystemExit(f"缺少目录: {src}")
    DIST.mkdir(exist_ok=True)
    if out.exists():
        out.unlink()
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        for p in sorted(src.rglob("*")):
            if p.is_file():
                z.write(p, p.relative_to(src).as_posix())
    with zipfile.ZipFile(out) as z:
        validate_json(z)
    return out


def check_format(path: pathlib.Path, want: int) -> tuple[bool, int]:
    with zipfile.ZipFile(path) as z:
        meta = json.loads(z.read("pack.mcmeta").decode("utf-8"))
    got = meta["pack"]["pack_format"]
    return got == want, got


def install(profile: pathlib.Path) -> None:
    saves = profile / "saves"
    targets = sorted(p for p in saves.glob("*") if p.is_dir())
    if not targets:
        raise SystemExit(f"{saves} 下没有存档")
    for s in targets:
        dp = s / "datapacks"
        dp.mkdir(exist_ok=True)
        for name in SRC.iterdir():
            if not (name.is_dir()) or name.name in RESOURCE_PACKS:
                continue
            shutil.copy2(DIST / f"{name.name}.zip", dp / f"{name.name}.zip")
        print(f"  数据包 -> {dp}")

    rp = profile / "resourcepacks"
    rp.mkdir(exist_ok=True)
    for name in RESOURCE_PACKS:
        z = DIST / f"{name}.zip"
        if z.exists():
            shutil.copy2(z, rp / z.name)
            print(f"  资源包 -> {rp / z.name}（要在游戏里手动启用）")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--install", metavar="PROFILE", help="Minecraft 实例目录")
    args = ap.parse_args()

    names = sorted(p.name for p in SRC.iterdir() if p.is_dir())
    ok = True
    warnings: list[str] = []
    for name in names:
        z = build_one(name)
        want = RESOURCE_PACK_FORMAT if name in RESOURCE_PACKS else DATA_PACK_FORMAT
        good, got = check_format(z, want)
        ok &= good
        flag = "ok" if good else f"pack_format={got} 期望 {want}"
        print(f"{name:<16} {z.stat().st_size / 1024:7.1f} KB  {flag}")
        for prob in validate_function_refs(name):
            warnings.append(f"{name}: {prob}")

    if warnings:
        print("\n函数引用有问题（游戏里会在调用那一刻才报错，建议先修）：")
        for w in warnings:
            print(f"  ! {w}")

    if not ok:
        print("\n有数据包格式不对，别装。")
        sys.exit(1)

    if args.install:
        profile = pathlib.Path(args.install)
        if not profile.is_dir():
            raise SystemExit(f"实例目录不存在: {profile}")
        install(profile)

    print("\n提示：进入世界后执行 /reload；传感器默认关闭，需要时执行")
    print("      /function trit_bridge:on")


if __name__ == "__main__":
    main()
