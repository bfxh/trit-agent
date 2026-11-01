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

PACKS = ["squad.zip", "summoner_origin.zip", "trit_bridge.zip"]
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
            r.fail(f"{tag}: 包在但没启用 {', '.join(off)}",
                   f'python scripts/save_packs.py enable "{profile.as_posix()}" --save "{w.name}"'
                   "（游戏必须先退出）")
        elif conflict:
            r.fail(f"{tag}: Enabled 和 Disabled 撞了 {', '.join(conflict)}",
                   "Disabled 里还有同名条目，写启用也没用")
        elif cmds in (0, b"\x00"):
            r.warn(f"{tag}: 三个包都开着，但这个存档禁用了指令（allowCommands=0）",
                   "AI 发的 /function 会被服务器直接拒掉")
        else:
            r.ok(f"{tag}: 三个包都在且已启用")


def check_lang(r: Report, profile: pathlib.Path) -> None:
    print("\n[3] 汉化")
    mods = profile / "mods"
    plus = [p for p in mods.glob("*.jar") if "origins-plus-plus" in p.name]
    core = [p for p in mods.glob("origins-forge*.jar")]
    if not plus:
        r.fail("mods 里找不到 origins-plus-plus 的 jar")
    else:
        z = zipfile.ZipFile(plus[0])
        rel = "assets/origins-plus-plus/lang/zh_cn.json"
        en = json.loads(z.read("assets/origins-plus-plus/lang/en_us.json").decode("utf-8-sig"))
        if rel not in z.namelist():
            r.fail(f"{plus[0].name} 里没有 zh_cn.json", "python scripts/make_origins_zh.py "
                   f'--mods "{mods.as_posix()}" --inject')
        else:
            zh = json.loads(z.read(rel).decode("utf-8"))
            miss = [k for k in en if k not in zh]
            nozh = [k for k, v in zh.items()
                    if not re.search(r"[\u4e00-\u9fff]", str(v))]
            if miss or nozh:
                r.warn(f"起源++ 缺 {len(miss)} 条，{len(nozh)} 条没汉字",
                       "重跑 make_origins_zh.py --inject")
            else:
                r.ok(f"起源++ 词条 {len(zh)} / {len(en)} 全中文（已写进 jar，无需启用资源包）")
    if core:
        z = zipfile.ZipFile(core[0])
        rel = "assets/origins/lang/zh_cn.json"
        zh = json.loads(z.read(rel).decode("utf-8-sig"))
        nozh = [k for k, v in zh.items() if not re.search(r"[\u4e00-\u9fff]", str(v))]
        if nozh:
            r.warn(f"起源本体仍有 {len(nozh)} 条英文：{nozh[:3]}")
        else:
            r.ok(f"起源本体 {len(zh)} 条全中文")

    opts = profile / "options.txt"
    if opts.is_file():
        text = opts.read_text(encoding="utf-8", errors="replace")
        m = re.search(r"^resourcePacks:(.*)$", text, re.M)
        if m and "origins_zh" in m.group(1):
            r.ok("origins_zh 资源包已在 options.txt 里启用（双保险）")
        else:
            r.warn("origins_zh 资源包没在 options.txt 里启用",
                   "不影响：译文已经写进 jar。只有模组更新覆盖 jar 时才需要它")


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



def check_origins(r: Report, profile: pathlib.Path) -> None:
    """起源数据包：类型名/必填字段/引用，按模组真实注册表校验。

    查的是**装进存档的那一份 zip**，不是仓库里的源目录 —— 免得「本地改好了
    但没重新打包」这种情况被漏掉。
    """
    print()
    print("[5] 起源数据包")
    sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
    try:
        import validate_origins as vo
    except Exception as e:
        r.warn(f"校验器加载失败: {e}")
        return
    mods = profile / "mods"
    if not mods.is_dir():
        r.warn(f"没有 mods 目录（{mods}），跳过")
        return

    seen: dict[str, pathlib.Path] = {}
    for w in sorted((profile / "saves").iterdir()):
        p = w / "datapacks" / "summoner_origin.zip"
        if p.is_file():
            seen.setdefault(hash(p.read_bytes()), p)
    if not seen:
        r.fail("没有任何存档里有 summoner_origin.zip", "跑 build_datapacks.py --install")
        return

    groups = vo.registries(mods)
    known = groups.pop("（合计）")
    harvested, global_files = vo.harvest(mods)
    known |= harvested

    problems = 0
    for p in seen.values():
        files = vo.load_pack(p)
        own_ns = {rel.split("/")[1] for rel in files if rel.startswith("data/")}
        names = set(files)
        bad: list[str] = []
        for rel, d in files.items():
            if not isinstance(d, dict) or "/tags/" in rel:
                continue
            if "/origin_layers/" in rel:
                vo.check_layer(rel, d, bad, names, own_ns)
            elif "/powers/" in rel:
                vo.check_power(rel, d, known, bad, [])
            elif "/origins/" in rel:
                vo.check_origin(rel, d, bad, [], names, own_ns, global_files)
        if bad:
            problems += len(bad)
            for b in bad[:5]:
                r.fail(f"{p.parent.parent.name}: {b}")
        else:
            r.ok(f"起源包校验通过（{len(files)} 个 json；各存档共 {len(seen)} 份不同内容）")
    if problems:
        r.warn(f"共 {problems} 处，改完记得重新 build_datapacks.py 再装")


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
    check_lang(r, profile)
    check_sensors(r, profile)
    check_origins(r, profile)

    print()
    if r.bad:
        print(f"有 {r.bad} 项需要处理，见上面的 -> 提示。")
        return 1
    print("全部就绪。进游戏后：F8 交还/接管，F9 急停，F10 切视角。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
