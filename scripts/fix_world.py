#!/usr/bin/env python3
"""一键修复：让 trit-agent 的数据包在**当前打开的世界**里真正生效。

## 为什么需要这个脚本

数据包 zip 拷进 `saves/<世界>/datapacks/` **不等于启用**。MC 只加载
`level.dat` 里 `DataPacks.Enabled` 列出的包，新拷进去的默认关闭，而且
**不报错、不提示**。

表现就是「传感器一条数据都不吐」「AI 永远 hp=?」「AI 站桩不动」——
看着像 AI 坏了，其实是包没开。

## 两条路，各有代价

| 做法 | 优点 | 代价 |
|---|---|---|
| 改 level.dat（`save_packs.py enable`） | 一次搞定，永久生效 | **必须先退出游戏**，否则退出时游戏把内存里的 level.dat 整个覆盖写回，改了等于没改 |
| 游戏内 `/datapack enable` | 立刻生效，不用退出 | 要玩家手动敲一条命令 |

所以这个脚本：**游戏在跑 -> 走游戏内命令（打印出来让你抄）**；
**游戏没跑 -> 直接改 level.dat**。

## 用法

    python scripts/fix_world.py                       # 自动判断
    python scripts/fix_world.py --profile "E:/MC/MC/profiles/烦人的村民"
    python scripts/fix_world.py --save "新的世界 (7)"  # 只处理某个存档
"""
from __future__ import annotations

import argparse
import pathlib
import subprocess
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import save_packs as sp  # noqa: E402

DEFAULT_PROFILE = r"E:/MC/MC/profiles/烦人的村民"

# 状态机需要的数据包。资源包不在其中（当前工程无资源包）。
NEEDED = ["squad.zip", "trit_bridge.zip"]


def enabled_list(level: pathlib.Path) -> list[str]:
    try:
        _raw, root, _rd = sp.load_level(level)
    except Exception:
        return []
    d = root.get("Data", root)
    return list(d.get("DataPacks", {}).get("Enabled", []))


def needs_enable(world: pathlib.Path) -> list[str]:
    """返回这个存档里「已拷进去但没启用」的包名。"""
    on = set(enabled_list(world / "level.dat"))
    out = []
    for z in NEEDED:
        if (world / "datapacks" / z).is_file() and f"file/{z}" not in on:
            out.append(z)
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--profile", default=DEFAULT_PROFILE)
    ap.add_argument("--save", help="只处理这个存档")
    a = ap.parse_args()
    profile = pathlib.Path(a.profile)

    if not profile.is_dir():
        print(f"实例目录不存在: {profile}")
        return 2

    running = sp.game_running()
    print(f"实例: {profile}")
    print(f"游戏进程: {'在跑' if running else '没在跑'}")

    targets = [s for s in sorted((profile / "saves").glob("*"))
                if s.is_dir() and (s / "level.dat").is_file()]
    if a.save:
        targets = [s for s in targets if s.name == a.save]
        if not targets:
            print(f"没有叫 {a.save!r} 的存档")
            return 1

    pending: list[tuple[str, list[str]]] = []
    for w in targets:
        miss = needs_enable(w)
        if miss:
            pending.append((w.name, miss))

    if not pending:
        print("\n所有存档的数据包都已启用，无需处理。")
        return 0

    if running:
        # 游戏在跑：改 level.dat 会被退出时的存档覆盖，必须走游戏内命令。
        print("\n以下存档的数据包已拷好但**没启用**。")
        print("游戏在运行，改 level.dat 会被退出时的存档整个覆盖写回 ——")
        print("所以请**进游戏**（进到要玩的那个世界）后敲下面这些命令：\n")
        for name, miss in pending:
            print(f"  存档「{name}」需要敲：")
            for z in miss:
                print(f'      /datapack enable "file/{z}"')
            print()
        print("只需要敲**当前所在世界**对应的那一组 —— 命令是全局生效的，")
        print("在哪个世界敲就启用哪个世界的包。")
        print("\n敲完等 1 秒，聊天框会开始每秒刷一行 <TRIT hp=... ...>，")
        print("那就说明传感器活了。")
        print("\n（更省事的办法：退出游戏后再跑一次本脚本，会直接改 level.dat）")
        return 0

    # 游戏没跑：直接改 level.dat
    print()
    rc = sp.cmd_enable(profile, a.save, all_saves=not a.save)
    return rc


if __name__ == "__main__":
    sys.exit(main())
