#!/usr/bin/env python3
"""生成 trit_bridge 数据包（传感器：把血量/坐标/周围敌人数写进聊天）。

为什么要这么做：Forge 不装 mod 时，外部程序唯一能稳定读到的游戏状态就是
客户端 `logs/latest.log`。而数据包里的 `tellraw` 会进聊天框、也会进日志，
于是「数据包 -> 聊天 -> 日志 -> 状态机」成了一条不需要任何 Java 代码的感知链路。
"""
from __future__ import annotations

import json
import pathlib

ROOT = pathlib.Path(__file__).resolve().parents[1]
BRIDGE = ROOT / "datapacks" / "trit_bridge"

VANILLA_HOSTILES = [
    "zombie", "husk", "drowned", "zombie_villager", "skeleton", "stray",
    "creeper", "spider", "cave_spider", "enderman", "witch", "pillager",
    "vindicator", "evoker", "illusioner", "ravager", "vex", "blaze",
    "wither_skeleton", "slime", "magma_cube", "phantom", "guardian",
    "elder_guardian", "piglin", "piglin_brute", "zombified_piglin",
    "hoglin", "zoglin", "ghast", "shulker", "silverfish", "endermite",
]

# 会爆炸的实体。苦力怕是 10 格起跑（它会追你），TNT 是 3 格（不会追）。
EXPLOSIVES = ["creeper", "tnt", "end_crystal", "fireball", "small_fireball", "wither_skull"]

# 距离档。数据包算精确距离要遍历实体，这里只做几次半径判定，
# 从远到近依次覆盖，最后留下的就是最近那一档。外部按档位下界使用。
DIST_TIERS = [32, 16, 10, 6, 3]


def write(rel: str, text: str) -> None:
    p = BRIDGE / rel
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(text, encoding="utf-8")


def hostiles_tag() -> dict:
    """优先复用召唤师数据包里的敌对实体清单，保证两边口径一致。"""
    src = ROOT / "datapacks" / "summoner_origin" / "data" / "summoner" / "tags" / "entity_types" / "hunt_targets.json"
    mod_values: list = []
    if src.exists():
        raw = json.loads(src.read_text(encoding="utf-8"))
        for v in raw["values"]:
            eid = v if isinstance(v, str) else v["id"]
            if eid.startswith("minecraft:"):
                continue  # 原版用下面这份固定清单
            if any(k in eid for k in ("fu_lu", "trident", "arrow")):
                continue  # 俘虏 / 投掷物不算威胁
            mod_values.append({"id": eid, "required": False})

    values: list = [f"minecraft:{m}" for m in VANILLA_HOSTILES] + mod_values
    return {"replace": False, "values": values}


def sense_function() -> str:
    parts = [
        ("hp", "Health", 10),
        ("food", "foodLevel", 1),
        ("px", "Pos[0]", 10),
        ("py", "Pos[1]", 10),
        ("pz", "Pos[2]", 10),
    ]
    lines = [
        "# 传感器：每 20 tick 由 pulse 调用一次，读一帧状态写进聊天",
        "scoreboard players set hst trit_bridge 0",
        "# 距离档：从远到近依次覆盖，最终留下最近那一档（0 = 视野内没有）",
        "scoreboard players set dst trit_bridge 0",
        "scoreboard players set xst trit_bridge 0",
    ]
    for score, nbt, scale in parts:
        lines.append(
            f"execute store result score {score} trit_bridge run data get entity @s {nbt} {scale}"
        )
    lines.append(
        "execute as @e[type=#trit_bridge:hostiles,limit=32,distance=..32] run scoreboard players add hst trit_bridge 1"
    )
    for t in DIST_TIERS:
        lines.append(
            f"execute if entity @e[type=#trit_bridge:hostiles,limit=1,distance=..{t}] "
            f"run scoreboard players set dst trit_bridge {t}"
        )
    for t in DIST_TIERS:
        lines.append(
            f"execute if entity @e[type=#trit_bridge:explosives,limit=1,distance=..{t}] "
            f"run scoreboard players set xst trit_bridge {t}"
        )
    tell = [
        {"text": "<TRIT hp="},
        {"score": {"name": "hp", "objective": "trit_bridge"}},
        {"text": " food="},
        {"score": {"name": "food", "objective": "trit_bridge"}},
        {"text": " x="},
        {"score": {"name": "px", "objective": "trit_bridge"}},
        {"text": " y="},
        {"score": {"name": "py", "objective": "trit_bridge"}},
        {"text": " z="},
        {"score": {"name": "pz", "objective": "trit_bridge"}},
        {"text": " h="},
        {"score": {"name": "hst", "objective": "trit_bridge"}},
        {"text": " d="},
        {"score": {"name": "dst", "objective": "trit_bridge"}},
        {"text": " c="},
        {"score": {"name": "xst", "objective": "trit_bridge"}},
        {"text": ">"},
    ]
    lines.append("tellraw @s " + json.dumps(tell, ensure_ascii=False, separators=(",", ":")))
    return "\n".join(lines) + "\n"


def main() -> None:
    write("pack.mcmeta", json.dumps(
        {"pack": {"pack_format": 9, "description": "trit-agent 传感器：状态写进聊天，供外部读取"}},
        ensure_ascii=False, indent=2) + "\n")

    write("data/minecraft/tags/functions/load.json",
          json.dumps({"values": ["trit_bridge:load"]}, indent=2) + "\n")
    write("data/minecraft/tags/functions/tick.json",
          json.dumps({"values": ["trit_bridge:tick"]}, indent=2) + "\n")

    write("data/trit_bridge/functions/load.mcfunction",
          "\n".join([
              "# 首次加载：建记分板，默认关闭（聊天刷屏，开之前先想清楚）",
              "scoreboard objectives add trit_bridge dummy",
              "scoreboard players set on trit_bridge 0",
              "scoreboard players set t trit_bridge 0",
              "",
          ]))

    write("data/trit_bridge/functions/tick.mcfunction",
          "\n".join([
              "# 计时器，每 20 tick（1 秒）触发一次 pulse",
              "scoreboard players add t trit_bridge 1",
              "execute if score t trit_bridge matches 20.. run function trit_bridge:pulse",
              "execute if score t trit_bridge matches 20.. run scoreboard players set t trit_bridge 0",
              "",
          ]))

    write("data/trit_bridge/functions/pulse.mcfunction",
          "\n".join([
              "# 开关关着就什么都不做",
              "execute if score on trit_bridge matches 1 as @a at @s run function trit_bridge:sense",
              "",
          ]))

    write("data/trit_bridge/functions/sense.mcfunction", sense_function())
    write("data/trit_bridge/functions/on.mcfunction",
          "scoreboard players set on trit_bridge 1\ntellraw @a {\"text\":\"[trit] 传感器已开启\",\"color\":\"gray\"}\n")
    write("data/trit_bridge/functions/off.mcfunction",
          "scoreboard players set on trit_bridge 0\ntellraw @a {\"text\":\"[trit] 传感器已关闭\",\"color\":\"gray\"}\n")

    write("data/trit_bridge/tags/entity_types/hostiles.json",
          json.dumps(hostiles_tag(), ensure_ascii=False, indent=2) + "\n")
    write("data/trit_bridge/tags/entity_types/explosives.json",
          json.dumps({"replace": False,
                      "values": [f"minecraft:{e}" for e in EXPLOSIVES]},
                     ensure_ascii=False, indent=2) + "\n")

    n = len(hostiles_tag()["values"])
    print(f"trit_bridge 生成完毕，敌对实体 {n} 个，爆炸源 {len(EXPLOSIVES)} 个")
    print(f"距离档：{DIST_TIERS}（数据包只报档位下界，精确距离由 Rust 侧按档使用）")
    for p in sorted(BRIDGE.rglob("*")):
        if p.is_file():
            print("  ", p.relative_to(ROOT))


if __name__ == "__main__":
    main()
