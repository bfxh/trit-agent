#!/usr/bin/env python3
"""生成 trit_bridge 数据包（传感器：把游戏状态写进聊天，外部程序读日志）。

为什么要这么做：Forge 不装 mod 时，外部程序唯一能稳定读到的游戏状态就是
客户端 `logs/latest.log`。而数据包里的 `tellraw` 会进聊天框、也会进日志，
于是「数据包 -> 聊天 -> 日志 -> 状态机」成了一条不需要任何 Java 代码的感知链路。

## 为什么是两条通道

一条通道只能有一个频率，而"要什么"是分层的：

- **瞄一眼就要知道**（瞄准角、前方有没有墙、脚下踩的是什么）——这些量一变，
  动作就得变。1 秒一次等于让 AI 闭着眼挥镐，`Forage` 里那句
  `pitch_next = now + burst + 900` 就是被这个 1 秒逼出来的妥协。
- **可以慢慢知道**（有几个怪、最近的在几格、木头在哪个方向、天亮没）——
  这些量变化慢，20 tick 一次完全够，而且测起来贵得多（要遍历实体、
  要打 40 多条射线）。

所以拆成 `<TRIT ...>`（慢，20 tick，常开）和 `<TRITF ...>`（快，2 tick，按需）。

**快通道默认是关的**，这不是省那点命令数，是省眼：2 tick 一条等于每秒往
聊天框刷 10 行，站着不动也刷。而它存在的意义只在"AI 正在动手"那几秒——
所以由 Rust 侧在进入采集/挖矿时发 `/function trit_bridge:fast_on` 打开，
收手时关掉。不动手就零刷屏、零开销。

Rust 侧把两条通道**合并到同一份状态**上（补丁式），字段各自只覆盖自己报的那些。
动手期间瞄准闭环的等待时间从 900ms 掉到 120ms 量级 —— 这是全套改动里
唯一真正影响"手感"的一项。
"""
from __future__ import annotations

import json
import pathlib

ROOT = pathlib.Path(__file__).resolve().parents[1]
BRIDGE = ROOT / "datapacks" / "trit_bridge"

# ---------------------------------------------------------------------------
# 实体标签
# ---------------------------------------------------------------------------

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

# ---------------------------------------------------------------------------
# 方块分类 —— 这份编码 Rust 侧有一份一模一样的（perceive::Block）
#
# 只有能塞进一个 scoreboard 整数的分类，才值得报。所以键是"我拿它干什么"，
# 不是"它是什么方块"：
#   0 空      —— 能过去
#   1 原木    —— 砍
#   2 矿石    —— 挖
#   3 固体    —— 挡住去路 / 能挖但不值钱
#   4 水      —— 能游，但别在这挖
#   5 岩浆    —— 绝对不挖
#   6 危险    —— 火 / 仙人掌 / 甜浆果 / 岩浆块
#
# 判定从低优先级到高优先级依次 set，靠"后写的赢"让高优先级最终生效。
# ---------------------------------------------------------------------------
CLS_EMPTY, CLS_LOG, CLS_ORE, CLS_SOLID, CLS_WATER, CLS_LAVA, CLS_HAZARD = range(7)

# 「挡路」判据。原版没有 `#minecraft:solid` 这种 tag，只能自己拼：
# 先用覆盖广的原版 tag，再补一批 tag 没收录的方块。
#
# 这里不追求穷尽所有方块 —— 漏掉的后果是「AI 以为能过去，撞上，然后靠
# stuck 检测兜底」，不是灾难。装了 mod 方块想让它认路，往下面的
# SOLID_BLOCKS 里加一行就行。
SOLID_TAGS = [
    "#minecraft:base_stone_overworld",
    "#minecraft:base_stone_nether",
    "#minecraft:dirt",
    "#minecraft:sand",
    "#minecraft:logs",
    "#minecraft:planks",
    "#minecraft:leaves",
    "#minecraft:wool",
    "#minecraft:stone_bricks",
    "#minecraft:walls",
    "#minecraft:fences",
    "#minecraft:terracotta",
    "#minecraft:coral_blocks",
    "#minecraft:wart_blocks",
    "#minecraft:nylium",
    "#minecraft:impermeable",
]

SOLID_BLOCKS = [
    # 岩石类（tag 已覆盖大部分，这里补深层与特殊岩）
    "bedrock", "obsidian", "crying_obsidian", "gravel", "clay",
    "cobbled_deepslate", "deepslate", "tuff", "calcite", "dripstone_block",
    "pointed_dripstone", "amethyst_block", "budding_amethyst", "smooth_basalt",
    "blackstone", "polished_blackstone", "basalt", "netherrack",
    "soul_sand", "soul_soil", "end_stone", "end_stone_bricks",
    "purpur_block", "purpur_pillar", "quartz_block",
    # 冰雪
    "snow_block", "snow", "ice", "packed_ice", "blue_ice", "frosted_ice", "powder_snow",
    # 玻璃与金属块
    "glass", "glass_pane", "glowstone", "sea_lantern", "shroomlight",
    "coal_block", "iron_block", "copper_block", "gold_block", "redstone_block",
    "lapis_block", "diamond_block", "emerald_block", "netherite_block",
    "raw_iron_block", "raw_copper_block", "raw_gold_block",
    # 功能方块（撞上去走不动的那种）
    "crafting_table", "furnace", "blast_furnace", "smoker", "chest", "barrel",
    "anvil", "chipped_anvil", "damaged_anvil", "bookshelf", "bricks",
    "note_block", "jukebox", "target", "observer", "piston", "sticky_piston",
    "dispenser", "dropper", "hopper", "cauldron", "water_cauldron", "lava_cauldron",
    "beehive", "bee_nest", "hay_block", "melon", "pumpkin", "carved_pumpkin",
    "jack_o_lantern", "slime_block", "honey_block", "honeycomb_block",
    "redstone_lamp", "lodestone", "respawn_anchor", "beacon", "conduit", "bell",
    "sponge", "wet_sponge", "ladder", "scaffolding", "iron_bars", "chain",
    "tnt", "cobweb",
]

# 1.18.2 全部原版矿石（含深板岩变种）。`forge:ores` 让 mod 矿石也能命中。
ORES = [
    "coal_ore", "deepslate_coal_ore", "iron_ore", "deepslate_iron_ore",
    "copper_ore", "deepslate_copper_ore", "gold_ore", "deepslate_gold_ore",
    "redstone_ore", "deepslate_redstone_ore", "lapis_ore", "deepslate_lapis_ore",
    "diamond_ore", "deepslate_diamond_ore", "emerald_ore", "deepslate_emerald_ore",
    "nether_quartz_ore", "nether_gold_ore", "ancient_debris",
]

WATER_BLOCKS = ["water", "bubble_column"]
LAVA_BLOCKS = ["lava"]
HAZARD_BLOCKS = [
    "fire", "soul_fire", "magma_block", "campfire", "soul_campfire",
    "cactus", "sweet_berry_bush", "wither_rose", "powder_snow", "lava",
    "pointed_dripstone", "scaffolding",
]

# ---------------------------------------------------------------------------
# 射线几何
# ---------------------------------------------------------------------------

# ---- 墙体：只测水平 1 格，8 个方位 ----
# 走路撞墙需要的是"这一步迈出去会不会撞"，1 格就够；测 2 格纯属浪费命令。
# 位 i = 相对朝向 yaw 偏移 WALL_DIRS[i] 度处 1 格内是否固体。
# 用 `scoreboard players add 1<<i` 累加成掩码 —— 一个 score 传 8 个布尔，
# 这是这条通道能压到 24 条命令的关键。
WALL_DIRS = [0, 45, 90, 135, 180, -135, -90, -45]

# ---- 原木：8 方向 × 3 档，由远到近、正前最后，留下最近那一格 ----
WOOD_DIRS = [(-135, -3), (-90, -2), (-45, -1), (45, 1), (90, 2), (135, 3), (180, 4), (0, 0)]
WOOD_TIERS = [12, 6, 3]
# 近档额外扫三个俯仰角，覆盖树干的下 / 中 / 上。
# 玩家砍树时眼高那格一被砍掉，水平射线就再也命中不了树干 ——
# 只打一条水平射线的话，AI 会以为"树没了"，其实还有五六格。
WOOD_NEAR_TIER = 3
WOOD_NEAR_PITCH = [-45, 0, 30]

# ---- 矿石：看不了那么远，地下视野基本为零，8 格足够 ----
ORE_DIRS = [(-135, -3), (-90, -2), (-45, -1), (0, 0), (45, 1), (90, 2), (135, 3), (180, 4)]
ORE_TIERS = [8, 4, 2]
ORE_NEAR_TIER = 2
# 挖矿是看着脚下前方挖的，矿石出现在视线偏下；三档俯仰覆盖台阶 / 竖井 / 平巷
ORE_NEAR_PITCH = [0, 20, 45]


def write(rel: str, text: str) -> None:
    p = BRIDGE / rel
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(text, encoding="utf-8")


def write_json(rel: str, obj) -> None:
    write(rel, json.dumps(obj, ensure_ascii=False, indent=2) + "\n")


def hostiles_tag() -> dict:
    """敌对实体标签。

    若召唤师起源包（已隔离到 archive/origins）存在则复用其敌对清单，保证口径一致；
    否则只用工程内置的原版敌对清单。无论哪种情况都能正常工作。
    """
    src = ROOT / "archive" / "origins" / "datapacks" / "summoner_origin" / "data" / "summoner" / "tags" / "entity_types" / "hunt_targets.json"
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


def block_tag(entries: list[str], extra: list | None = None) -> dict:
    """方块 tag。`#` 开头的按 tag 引用，其余加 `minecraft:` 前缀。

    全部带 `required: false`：某个方块在本版本不存在时只记一条警告，
    不会让整个数据包加载失败 —— 而这个包一旦加载失败，AI 就彻底瞎了。
    """
    values: list = []
    for e in entries:
        eid = e if e.startswith("#") else f"minecraft:{e}"
        values.append({"id": eid, "required": False})
    if extra:
        values.extend(extra)
    return {"replace": False, "values": values}


# ---------------------------------------------------------------------------
# 快通道
# ---------------------------------------------------------------------------

def sense_fast_function() -> str:
    """每 2 tick 跑一次。只报"动作一变它就得变"的量。"""
    lines = [
        "# 快通道：每 2 tick（100ms）由 pulse_fast 调用。",
        "# 只报瞄准闭环与碰撞判定要用的量，命令数压到 24 条以内。",
        "",
        "# 掩码清零：8 个方位各占一位",
        "scoreboard players set wal trit_bridge 0",
        "# 正前方是不是「一格高的台阶」（前面有固体、它上面没有）",
        "scoreboard players set stp trit_bridge 0",
        "# 俯仰角 ×10（-900..900）。快通道报它，是为了让瞄准闭环的等待",
        "# 从「等 1 秒」变成「等 100 毫秒」——三值只给方向，没有角度就是开环瞎转。",
        "execute store result score pit trit_bridge run data get entity @s Rotation[1] 10",
        "",
        "# 八个方位 1 格内是否有固体：撞墙 / 绕行判定",
    ]
    for i, deg in enumerate(WALL_DIRS):
        bit = 1 << i
        lines.append(
            f"execute at @s rotated ~{deg} 0 if block ^ ^ ^1 #trit_bridge:solid "
            f"run scoreboard players add wal trit_bridge {bit}"
        )

    lines.append("")
    lines.append("# 正前方是台阶还是墙：一格高能跳过去，两格高只能绕。")
    lines.append("# 少了这一条，AI 会把每一个土坎都当成死路 —— 表现出来就是"
                 "「明明能走的地方它站住不动」。")
    lines.append(
        "execute at @s rotated ~0 0 positioned ^ ^ ^1 if block ~ ~ ~ #trit_bridge:solid "
        "unless block ~ ~1 ~ #trit_bridge:solid run scoreboard players set stp trit_bridge 1"
    )

    lines.append("")
    lines.append("# 准星正对的方块（沿视线，含俯仰）：挖矿的目标就是它")
    lines.append("# 判定顺序 = 低优先级先 set、高优先级后 set，后写的赢")
    lines.extend(_classify_lines("b1", "^ ^ ^1"))
    lines.append("")
    lines.append("# 脚下那格：站在什么上面。挖矿最怕的就是挖塌自己站的地方")
    lines.extend(_classify_lines("bd", "~ ~-1 ~"))

    tell = [
        {"text": "<TRITF wal="},
        {"score": {"name": "wal", "objective": "trit_bridge"}},
        {"text": " b1="},
        {"score": {"name": "b1", "objective": "trit_bridge"}},
        {"text": " bd="},
        {"score": {"name": "bd", "objective": "trit_bridge"}},
        {"text": " s="},
        {"score": {"name": "stp", "objective": "trit_bridge"}},
        {"text": " p="},
        {"score": {"name": "pit", "objective": "trit_bridge"}},
        {"text": ">"},
    ]
    lines.append("")
    lines.append("tellraw @s " + json.dumps(tell, ensure_ascii=False, separators=(",", ":")))
    return "\n".join(lines) + "\n"


def _classify_lines(score: str, coords: str) -> list[str]:
    """把一个位置的方块归成 7 类之一，写进 score。"""
    order = [
        (CLS_SOLID, "#trit_bridge:solid"),
        (CLS_LOG, "#minecraft:logs"),
        (CLS_ORE, "#trit_bridge:ores"),
        (CLS_WATER, "#trit_bridge:water"),
        (CLS_LAVA, "#trit_bridge:lava"),
        (CLS_HAZARD, "#trit_bridge:hazard"),
    ]
    # 先清零，再按优先级从低到高覆盖
    out = [f"scoreboard players set {score} trit_bridge {CLS_EMPTY}"]
    for cls, sel in order:
        out.append(
            f"execute at @s if block {coords} {sel} "
            f"run scoreboard players set {score} trit_bridge {cls}"
        )
    return out


# ---------------------------------------------------------------------------
# 慢通道
# ---------------------------------------------------------------------------

def sense_function() -> str:
    parts = [
        ("hp", "Health", 10),
        ("food", "foodLevel", 1),
        ("px", "Pos[0]", 10),
        ("py", "Pos[1]", 10),
        ("pz", "Pos[2]", 10),
    ]
    lines = [
        "# 慢通道：每 20 tick 由 pulse 调用一次。",
        "# 报变化慢、但测起来贵的量：敌人数、距离档、资源方位、昼night。",
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

    # 游戏内时间，用来判昼夜（13000..23000 算夜里）
    lines.append("")
    lines.append("execute store result score tod trit_bridge run time query daytime")
    # 俯仰角快通道也报，这里保留一份：万一用户没重装数据包，
    # 只有慢通道的老包也能让新的 Rust 拿到角度 —— 版本错配不该变成瞎转。
    lines.append("execute store result score pit trit_bridge run data get entity @s Rotation[1] 10")

    # ---- 原木 ----
    lines.append("")
    lines.append("# 原木：8 方向 × 3 档，由远到近、正前最后，留下最近那一格")
    lines.append("scoreboard players set wdt trit_bridge 0")
    lines.append("scoreboard players set wdr trit_bridge 0")
    for tier in WOOD_TIERS:
        pitches = WOOD_NEAR_PITCH if tier == WOOD_NEAR_TIER else [0]
        for pitch in pitches:
            for deg, wd in WOOD_DIRS:
                probe = f"execute at @s rotated ~{deg} {pitch} if block ^ ^ ^{tier} #minecraft:logs"
                lines.append(f"{probe} run scoreboard players set wdt trit_bridge {tier}")
                lines.append(f"{probe} run scoreboard players set wdr trit_bridge {wd}")

    # ---- 矿石 ----
    lines.append("")
    lines.append("# 矿石：8 方向 × 3 档。地下视野极差，探测半径收到 8 格")
    lines.append("scoreboard players set odt trit_bridge 0")
    lines.append("scoreboard players set odr trit_bridge 0")
    for tier in ORE_TIERS:
        pitches = ORE_NEAR_PITCH if tier == ORE_NEAR_TIER else [0]
        for pitch in pitches:
            for deg, od in ORE_DIRS:
                probe = f"execute at @s rotated ~{deg} {pitch} if block ^ ^ ^{tier} #trit_bridge:ores"
                lines.append(f"{probe} run scoreboard players set odt trit_bridge {tier}")
                lines.append(f"{probe} run scoreboard players set odr trit_bridge {od}")

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
        {"text": " w="},
        {"score": {"name": "wdt", "objective": "trit_bridge"}},
        {"text": " wd="},
        {"score": {"name": "wdr", "objective": "trit_bridge"}},
        {"text": " o="},
        {"score": {"name": "odt", "objective": "trit_bridge"}},
        {"text": " od="},
        {"score": {"name": "odr", "objective": "trit_bridge"}},
        {"text": " t="},
        {"score": {"name": "tod", "objective": "trit_bridge"}},
        {"text": " p="},
        {"score": {"name": "pit", "objective": "trit_bridge"}},
        {"text": ">"},
    ]
    lines.append("")
    lines.append("tellraw @s " + json.dumps(tell, ensure_ascii=False, separators=(",", ":")))
    return "\n".join(lines) + "\n"


FAST_TICKS = 2
SLOW_TICKS = 20


def main() -> None:
    write_json("pack.mcmeta", {
        "pack": {"pack_format": 9,
                 "description": "trit-agent 传感器：快慢双通道状态流，供外部读取"}
    })

    # 函数标签必须带 "replace": false —— 1.18 的标签默认行为是**替换**，
    # 不写这个字段就等于把整个 minecraft:tick / minecraft:load 清空只留我们这条，
    # 整合包里其它模组的 tick 函数会被静默踢掉（它们的资源/行为就废了）。
    # squad 包的同名标签也是这个规矩，两边必须一致。
    write_json("data/minecraft/tags/functions/load.json",
               {"replace": False, "values": ["trit_bridge:load"]})
    write_json("data/minecraft/tags/functions/tick.json",
               {"replace": False, "values": ["trit_bridge:tick"]})

    # -----------------------------------------------------------------------
    # 补丁：给整合包的一个坏引用填空
    #
    # 整合包里有 206 个 TheAnnoyingVillagers 副本，其中 (205) 的
    # data/minecraft/tags/functions/tick.json 引用了 `annoying_villagers_use_block:cmd`，
    # 但这个函数**在所有 206 个 jar 里都不存在**。
    #
    # MC 1.18 的标签加载是 all-or-nothing：任何一个条目解析失败，
    # **整个 minecraft:tick 标签就作废**（日志：
    #   Couldn't load tag minecraft:tick as it is missing following
    #   references: annoying_villagers_use_block:cmd (from ...(205).jar)）
    # 标签一作废，挂在这个标签上的 trit_bridge:tick 和 squad:tick 就都不会跑，
    # tellraw 一行都不出 —— 于是日志里搜 TRIT 是 0 条，AI 全是 hp=?，
    # 按三值原则「未知不猜」就站桩不动。传感器本身完全没问题，是它**没被调用**。
    #
    # 修法：在这里补一个同名空函数，把缺失的引用补上，标签就能正常加载。
    # 不动整合包的任何 jar（那是别人的东西），代价只是 7 个字节。
    # 那个引用本来就没实现，补个空壳等于「让它本来就想做的事变成什么都不做」。
    write("data/annoying_villagers_use_block/functions/cmd.mcfunction", "\n".join([
        "# 空函数：只为填上整合包里一个指向不存在函数的引用。",
        "#",
        "# TheAnnoyingVillagersByPugilist_Steve_4_28 (205).jar 的",
        "# data/minecraft/tags/functions/tick.json 引用了本函数，但那个 jar 里",
        "# 并没有实现它。MC 的标签加载是 all-or-nothing，坏一条 = 整个",
        "# minecraft:tick 标签作废 = 我们的传感器 tick 也不跑 = AI 瞎。",
        "# 本函数存在的唯一意义就是让那个引用能解析。",
        "",
    ]))

    write("data/trit_bridge/functions/load.mcfunction", "\n".join([
        "# 首次加载：建记分板",
        "scoreboard objectives add trit_bridge dummy",
        "",
        "# 「总开关」默认打开。",
        "#",
        "# 默认值原来是 0，理由是「tellraw 刷聊天框」。但 AI 的感知**完全**依赖",
        "# 这一条通道：开关一关，tellraw 一行都不出，日志里搜 TRIT 是 0 条，于是",
        "# 墙的方位、敌人距离、脚下是什么全是 None —— 按三值原则「未知不猜」，",
        "# AI 就站桩不动。这正是「AI 看不见墙壁 / 不自动移动」的来源。",
        "# 刷屏的代价（每秒一行）远小于瞎掉的代价，所以默认改成开。",
        "#",
        "# 注意：必须用 #inited 记「已经给过默认值」，不能每次都 `set on 1` ——",
        "# 否则你手动关掉以后，一次 /reload 就又给打开了，看着像设置没生效。",
        "execute unless score #inited trit_bridge matches 1 run scoreboard players set on trit_bridge 1",
        "scoreboard players set #inited trit_bridge 1",
        "",
        "# fo = fast on。快通道保持默认关：它每 2 tick 刷一行，只在采集这种需要",
        "# 100ms 闭环的场合由 Rust 侧临时打开，用完即关。",
        "execute unless score #inited trit_bridge matches 1 run scoreboard players set fo trit_bridge 0",
        "",
        "# 以下都是每 tick 重算的中间量，重复清零无害",
        "scoreboard players set tf trit_bridge 0",
        "scoreboard players set t trit_bridge 0",
        "scoreboard players set wal trit_bridge 0",
        "scoreboard players set b1 trit_bridge 0",
        "scoreboard players set bd trit_bridge 0",
        "scoreboard players set wdt trit_bridge 0",
        "scoreboard players set wdr trit_bridge 0",
        "scoreboard players set odt trit_bridge 0",
        "scoreboard players set odr trit_bridge 0",
        "scoreboard players set tod trit_bridge 0",
        "scoreboard players set pit trit_bridge 0",
        "",
    ]))

    # 两个独立计数器：快通道每 2 tick，慢通道每 20 tick。
    # 快通道的 2 能整除慢通道的 20，所以两者永远同拍，不会错位漂移。
    write("data/trit_bridge/functions/tick.mcfunction", "\n".join([
        f"# 快通道：每 {FAST_TICKS} tick（{FAST_TICKS * 50}ms）",
        "scoreboard players add tf trit_bridge 1",
        f"execute if score tf trit_bridge matches {FAST_TICKS}.. run function trit_bridge:pulse_fast",
        f"execute if score tf trit_bridge matches {FAST_TICKS}.. run scoreboard players set tf trit_bridge 0",
        "",
        f"# 慢通道：每 {SLOW_TICKS} tick（{SLOW_TICKS * 50}ms）",
        "scoreboard players add t trit_bridge 1",
        f"execute if score t trit_bridge matches {SLOW_TICKS}.. run function trit_bridge:pulse",
        f"execute if score t trit_bridge matches {SLOW_TICKS}.. run scoreboard players set t trit_bridge 0",
        "",
    ]))

    write("data/trit_bridge/functions/pulse_fast.mcfunction",
          "# 快通道要两个开关同时开着才跑：总开关 on + 按需开关 fo\n"
          "execute if score on trit_bridge matches 1 if score fo trit_bridge matches 1 "
          "as @a at @s run function trit_bridge:sense_fast\n")
    write("data/trit_bridge/functions/pulse.mcfunction",
          "execute if score on trit_bridge matches 1 as @a at @s run function trit_bridge:sense\n")

    write("data/trit_bridge/functions/sense_fast.mcfunction", sense_fast_function())
    write("data/trit_bridge/functions/sense.mcfunction", sense_function())
    write("data/trit_bridge/functions/on.mcfunction",
          "scoreboard players set on trit_bridge 1\n"
          "tellraw @a {\"text\":\"[trit] 传感器已开启\",\"color\":\"gray\"}\n")
    write("data/trit_bridge/functions/off.mcfunction",
          "scoreboard players set on trit_bridge 0\n"
          "scoreboard players set fo trit_bridge 0\n"
          "tellraw @a {\"text\":\"[trit] 传感器已关闭\",\"color\":\"gray\"}\n")
    # 快通道开关：故意不 tellraw —— 这个每秒会被调一次，回显只会更吵。
    write("data/trit_bridge/functions/fast_on.mcfunction",
          "scoreboard players set on trit_bridge 1\n"
          "scoreboard players set fo trit_bridge 1\n")
    write("data/trit_bridge/functions/fast_off.mcfunction",
          "scoreboard players set fo trit_bridge 0\n")

    # ---- 实体标签 ----
    write_json("data/trit_bridge/tags/entity_types/hostiles.json", hostiles_tag())
    write_json("data/trit_bridge/tags/entity_types/explosives.json",
               {"replace": False,
                "values": [{"id": f"minecraft:{e}", "required": False} for e in EXPLOSIVES]})

    # ---- 方块标签 ----
    write_json("data/trit_bridge/tags/blocks/solid.json", block_tag(SOLID_TAGS + SOLID_BLOCKS))
    write_json("data/trit_bridge/tags/blocks/ores.json",
               # 注意别再自己加 `minecraft:` —— block_tag 会加。写重了会变成
               # `minecraft:minecraft:coal_ore`，游戏不报错，只在后台记一行
               # "Couldn't read tag list"，然后整个 tag 变空：
               # 表现是 AI 在矿洞里睁眼瞎，而不是"数据包坏了"。
               block_tag(ORES,
                         extra=[{"id": "#forge:ores", "required": False},
                                {"id": "#c:ores", "required": False}]))
    write_json("data/trit_bridge/tags/blocks/water.json", block_tag(WATER_BLOCKS))
    write_json("data/trit_bridge/tags/blocks/lava.json", block_tag(LAVA_BLOCKS))
    write_json("data/trit_bridge/tags/blocks/hazard.json", block_tag(HAZARD_BLOCKS))

    n_fast = len(sense_fast_function().splitlines())
    n_slow = len(sense_function().splitlines())
    n_cmd_fast = sum(1 for l in sense_fast_function().splitlines()
                     if l and not l.startswith("#"))
    n_cmd_slow = sum(1 for l in sense_function().splitlines()
                     if l and not l.startswith("#"))
    per_tick = n_cmd_fast / FAST_TICKS + n_cmd_slow / SLOW_TICKS

    print(f"trit_bridge 生成完毕")
    print(f"  敌对实体 {len(hostiles_tag()['values'])} 个，爆炸源 {len(EXPLOSIVES)} 个")
    print(f"  距离档 {DIST_TIERS}（只报档位下界，精确距离由 Rust 侧按档使用）")
    print(f"  快通道 {n_cmd_fast} 条 / {FAST_TICKS} tick = {n_cmd_fast / FAST_TICKS:.1f} 条每 tick")
    print(f"  慢通道 {n_cmd_slow} 条 / {SLOW_TICKS} tick = {n_cmd_slow / SLOW_TICKS:.1f} 条每 tick")
    print(f"  合计约 {per_tick:.1f} 条每 tick")
    print(f"  固体 {len(SOLID_TAGS)} 个 tag + {len(SOLID_BLOCKS)} 个方块，"
          f"矿石 {len(ORES)} 种")
    for p in sorted(BRIDGE.rglob("*")):
        if p.is_file():
            print("  ", p.relative_to(ROOT))


if __name__ == "__main__":
    main()
