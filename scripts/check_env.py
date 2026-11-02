#!/usr/bin/env python3
"""一条命令自检：重启游戏后如果「还是没有」，先跑这个。

    python scripts/check_env.py
    python scripts/check_env.py --profile "E:/MC/MC/profiles/烦人的村民"

为什么要这个：这个项目里已经出现过五次**静默失败**（墙方位、方块 tag 前缀、
窗口标题、`data modify` 空路径、拷了包但没启用）。它们的共同点是游戏不报错、
日志里也没有，只能靠「外面看一眼」才发现。所以把「外面看一眼」做成命令。

只读，不写任何文件。任何一项 ✗ 后面都写着「怎么办」。
"""
from __future__ import annotations

import argparse
import json
import pathlib
import re
import subprocess
import sys
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
import save_packs as sp  # noqa: E402  复用 NBT 读取

PACKS = ["squad.zip", "trit_bridge.zip"]  # 状态机数据包；起源包已隔离到 archive/origins/
# 自带剧情脚本的教学存档，故意不往里塞东西，别当成故障
SKIP_SAVES = {"新手教程"}
DEFAULT_PROFILE = pathlib.Path(r"E:/MC/MC/profiles/烦人的村民")


class Report:
    def __init__(self) -> None:
        self.bad = 0

    def ok(self, msg: str, hint: str = "") -> None:
        print(f"  [ok]   {msg}")

    def warn(self, msg: str, hint: str = "") -> None:
        print(f"  [注意] {msg}")
        if hint:
            print(f"         -> {hint}")

    def fail(self, msg: str, hint: str = "") -> None:
        self.bad += 1
        print(f"  [✗]    {msg}")
        if hint:
            print(f"         -> {hint}")


def game_running() -> bool:
    try:
        out = subprocess.run(["tasklist"], capture_output=True, text=True,
                             encoding="utf-8", errors="replace", timeout=20).stdout
    except Exception:
        return False
    return bool(re.search(r"(?i)\b(javaw|java)\.exe", out))


def check_binary(r: Report) -> None:
    print("\n[1] AI 本体")
    exe = ROOT / "target" / "release" / "trit-agent.exe"
    if not exe.is_file():
        r.fail("target/release/trit-agent.exe 不存在",
               "cargo build --release")
        return
    newest = max((p.stat().st_mtime for p in (ROOT / "src").rglob("*.rs")), default=0)
    if exe.stat().st_mtime < newest - 1:
        r.warn("exe 比源码旧，改过的代码没编进去",
               "cargo build --release")
    else:
        r.ok(f"exe 是最新的（{exe.stat().st_size / 1024 / 1024:.1f} MB）")

    cfg = ROOT / "trit-agent.toml"
    if not cfg.is_file():
        r.fail("没有 trit-agent.toml", "双击 启动AI.bat 会在当前目录找配置")
        return
    text = cfg.read_text(encoding="utf-8", errors="replace")
    m = re.search(r'^log_path\s*=\s*"([^"]*)"', text, re.M)
    if not m or not m.group(1):
        r.fail("配置里 log_path 是空的", 'log_path = "<实例>/logs/latest.log"')
    elif not pathlib.Path(m.group(1)).is_file():
        r.fail(f"log_path 指向的文件不存在: {m.group(1)}",
               "游戏至少启动过一次才会生成 logs/latest.log")
    else:
        r.ok(f"日志路径可用: {m.group(1)}")

    m = re.search(r'^window_title\s*=\s*"([^"]*)"', text, re.M)
    if m:
        r.ok(f"窗口标题关键字 = {m.group(1)!r}（找不到会原地等，不会退出）")


def check_saves(r: Report, profile: pathlib.Path) -> None:
    print("\n[2] 存档里的数据包")
    saves = profile / "saves"
    if not saves.is_dir():
        r.fail(f"找不到存档目录 {saves}")
        return
    for w in sorted(saves.iterdir()):
        lv = w / "level.dat"
        if not lv.is_file() or w.name in SKIP_SAVES:
            continue
        dp_dir = w / "datapacks"
        have = {p.name for p in dp_dir.iterdir()} if dp_dir.is_dir() else set()
        missing = [p for p in PACKS if p not in have]
        try:
            _, root, _r = sp.load_level(lv)
        except Exception as e:
            r.fail(f"{w.name}: level.dat 读不了 {e}")
            continue
        d = root.get("Data", root)
        packs = d.get("DataPacks", {})
        enabled = list(packs.get("Enabled", []))
        disabled = set(packs.get("Disabled", []))
        off = [p for p in PACKS if f"file/{p}" not in enabled]
        conflict = [p for p in PACKS if f"file/{p}" in disabled]
        cmds = d.get("allowCommands")
        tag = f"{w.name}"
        if missing:
            r.fail(f"{tag}: datapacks/ 里缺 {', '.join(missing)}",
                   "python scripts/build_datapacks.py --install <实例>")
        elif off:
            # 两条路：游戏没跑 -> 改 level.dat；游戏在跑 -> 走游戏内命令，
            # 因为运行期改 level.dat 会被退出时的存档整个覆盖写回。
            if game_running():
                hint = ("在游戏里敲："
                        + "  ".join(f'/datapack enable "file/{p}"' for p in off)
                        + "（或退出游戏后跑 python scripts/fix_world.py）")
            else:
                hint = (f'python scripts/fix_world.py --save "{w.name}"'
                        "（游戏已关，会直接改 level.dat）")
            r.fail(f"{tag}: 包在但没启用 {', '.join(off)}", hint)
        elif conflict:
            r.fail(f"{tag}: Enabled 和 Disabled 撞了 {', '.join(conflict)}",
                   "Disabled 里还有同名条目，写启用也没用")
        elif cmds in (0, b"\x00"):
            r.warn(f"{tag}: 三个包都开着，但这个存档禁用了指令（allowCommands=0）",
                   "AI 发的 /function 会被服务器直接拒掉")
        else:
            r.ok(f"{tag}: 三个包都在且已启用")


def check_sensors(r: Report, profile: pathlib.Path) -> None:
    print("\n[4] 数据包格式")
    seen = bad = 0
    for w in sorted((profile / "saves").iterdir()):
        dp = w / "datapacks"
        if not dp.is_dir():
            continue
        for p in sorted(dp.glob("*.zip")):
            try:
                z = zipfile.ZipFile(p)
                meta = json.loads(z.read("pack.mcmeta").decode("utf-8"))
                fmt = meta["pack"]["pack_format"]
            except Exception as e:
                r.fail(f"{w.name}/{p.name} 读不了: {e}")
                bad += 1
                continue
            seen += 1
            if fmt != 9:
                r.fail(f"{w.name}/{p.name} pack_format={fmt}，1.18.2 数据包应为 9")
                bad += 1
    if seen and not bad:
        r.ok(f"{seen} 个数据包 pack_format 全部 = 9")


def check_mod_tags(r: Report, profile: pathlib.Path) -> None:
    """[5] 模组 function 标签的坏引用 —— 静默废掉整个标签的隐形杀手。

    MC 1.18 的标签加载是 **all-or-nothing**：`minecraft:tick` 里只要有一个条目
    指向不存在的函数，**整个标签就作废**，挂在上面的所有 tick 函数一个都不跑，
    而且**只打一行 ERROR，不报「你的功能没生效」**。

    这就是本项目踩过的最隐蔽的一个坑：整合包里有个 epicfight 改名时漏改引用的
    jar，它的 tick.json 引用了一个不存在的函数，于是 `minecraft:tick` 整体作废
    -> 我们的 trit_bridge:tick 从来没跑过 -> 传感器一行都不吐 -> AI 永远
    `hp=?` -> 按三值原则站桩。表现是「AI 不动」，真因在三个文件之外。

    这里扫一遍所有模组 jar 的 function 标签，把「引用了不存在的东西」揪出来。
    """
    print("\n[5] 模组标签完整性")
    mods = profile / "mods"
    if not mods.is_dir():
        r.warn(f"没有 mods 目录（{mods}），跳过")
        return

    # 先收集「所有已存在的函数」：jar 里的 .mcfunction + 数据包里的。
    # 命名空间级判定（函数是否存在）足够定位这类坏引用。
    existing: set[str] = set()
    # 哪些命名空间是「整个整合包共用一套」的，无法按命名空间判存在性。
    # 模组常用 'modid' 或 'forge' 之类的通用命名空间，出现即视为存在。
    loose_ns: set[str] = set()
    jars = sorted(mods.glob("*.jar"))
    for j in jars:
        try:
            z = zipfile.ZipFile(j)
        except Exception:
            continue
        for n in z.namelist():
            if n.startswith("data/") and n.endswith("/functions/") :
                continue
            if n.startswith("data/") and "/functions/" in n and n.endswith(".mcfunction"):
                rel = n[len("data/"):-len(".mcfunction")]
                ns, _, fn = rel.partition("/")
                existing.add(f"{ns}:{fn}")
                loose_ns.add(ns)
            elif n.startswith("data/") and "/functions/" in n and n.endswith(".mcmeta"):
                rel = n[len("data/"):-len(".mcmeta")]
                ns, _, fn = rel.partition("/")
                existing.add(f"{ns}:{fn}")
                loose_ns.add(ns)

    # 数据包里定义的函数也算存在（含我们自己的补丁）
    for w in sorted((profile / "saves").iterdir()):
        dp = w / "datapacks"
        if not dp.is_dir():
            continue
        for p in sorted(dp.glob("*.zip")):
            try:
                z = zipfile.ZipFile(p)
            except Exception:
                continue
            for n in z.namelist():
                if n.startswith("data/") and "/functions/" in n and n.endswith(".mcfunction"):
                    rel = n[len("data/"):-len(".mcfunction")]
                    ns, _, fn = rel.partition("/")
                    existing.add(f"{ns}:{fn}")
                    loose_ns.add(ns)

    problems: list[str] = []
    checked = 0
    for j in jars:
        try:
            z = zipfile.ZipFile(j)
        except Exception:
            continue
        for n in z.namelist():
            if not (n.startswith("data/minecraft/tags/functions/") and n.endswith(".json")):
                continue
            tag = n[len("data/minecraft/tags/functions/"):-len(".json")]
            try:
                jd = json.loads(z.read(n).decode("utf-8-sig"))
            except Exception as e:
                problems.append(f"{j.name} 的 {tag} 标签解析失败: {e}")
                continue
            vals = jd.get("values", []) if isinstance(jd, dict) else []
            checked += 1
            for v in vals:
                if not isinstance(v, str) or v.startswith("#"):
                    continue  # 以 # 开头是引用别的标签，不在本次判定范围
                ns, _, fn = v.partition(":")
                if not fn:
                    continue
                if v in existing:
                    continue
                # 命名空间里一个函数都没有 = 这个命名空间根本不存在，
                # 那这条引用必然解析不了（除非是运行时注册的自定义函数）。
                if ns not in loose_ns:
                    problems.append(
                        f"{j.name} 的 minecraft:{tag} 标签引用了不存在的函数 {v}"
                        f"（整个 minecraft:{tag} 标签会作废！）")
    if problems:
        for p in problems[:6]:
            r.fail(p, "要么补上这个函数，要么把该 jar 移出 mods/")
    else:
        r.ok(f"{checked} 个模组 function 标签引用全部有效")




def check_proto(r, profile) -> None:
    """[6] 传感器协议：数据包报的字段名和 Rust 解析器认的对不对得上。

    校验对象是**存档里已装的 zip**，不是源码目录 —— 装了什么游戏就加载什么。
    改了源码忘了重新 --install，这里会直接报出来。
    """
    print("[6] 传感器协议")
    try:
        import build_datapacks as bd
    except Exception as e:
        r.warn(f"校验器加载失败: {e}")
        return

    known = set(bd._rust_known_fields())
    if not known:
        r.warn("没从 src/perceive/mod.rs 读到字段，跳过")
        return
    print(f"      Rust 解析器认得 {len(known)} 个字段: {' '.join(sorted(known))}")

    seen: set[tuple[str, int]] = set()
    problems = 0
    for w in sorted((profile / "saves").iterdir()):
        if not w.is_dir():
            continue
        for pack in ("trit_bridge",):
            z = w / "datapacks" / f"{pack}.zip"
            if not z.is_file():
                continue
            key = (pack, hash(z.read_bytes()))
            if key in seen:
                continue
            seen.add(key)
            with zipfile.ZipFile(z) as zf:
                for n in sorted(zf.namelist()):
                    if not n.endswith(".mcfunction"):
                        continue
                    if "sense" not in pathlib.Path(n).name:
                        continue
                    text = zf.read(n).decode("utf-8")
                    for i, ln in enumerate(text.splitlines(), 1):
                        if "tellraw" not in ln or "<TRIT" not in ln:
                            continue
                        for p in bd.check_tellraw_line(ln, f"{w.name}/{n}:{i}", known):
                            problems += 1
                            r.fail(p)
    if problems == 0:
        r.ok("数据包报的字段 Rust 全认得，标签与取值一一对应")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--profile", default=str(DEFAULT_PROFILE), help="游戏实例目录")
    a = ap.parse_args()
    profile = pathlib.Path(a.profile)
    if not profile.is_dir():
        print(f"实例目录不存在: {profile}")
        print("用 --profile 指定，例如 --profile \"E:/MC/MC/profiles/烦人的村民\"")
        return 2

    r = Report()
    print(f"实例: {profile}")
    print(f"游戏进程: {'在跑' if game_running() else '没在跑（改存档前正好）'}")

    check_binary(r)
    check_saves(r, profile)
    check_sensors(r, profile)
    check_mod_tags(r, profile)
    check_proto(r, profile)

    print()
    if r.bad:
        print(f"有 {r.bad} 项需要处理，见上面的 -> 提示。")
        return 1
    print("全部就绪。进游戏后：F8 交还/接管，F9 急停，F10 切视角。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
