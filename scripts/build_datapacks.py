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
import os
import pathlib
import re
import shutil
import subprocess
import sys
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
SRC = ROOT / "datapacks"
DIST = ROOT / "dist"

# 数据包 pack_format：1.18.2 是 9（1.18 / 1.18.1 才是 8）
DATA_PACK_FORMAT = 9
# 资源包 pack_format：1.18.2 是 8
RESOURCE_PACK_FORMAT = 8

# 起源校验要用到模组目录，默认跟 validate_origins.py 保持一致
DEFAULT_MODS = r"E:/MC/MC/profiles/烦人的村民/mods"

RESOURCE_PACKS = {"summoner_lang", "origins_zh"}


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


BLOCK_TAG_REF = re.compile(r"#([a-z0-9_]+):([a-z0-9_/]+)")


def validate_block_tags(name: str) -> list[str]:
    """检查 mcfunction 里 `#ns:tag` 的方块标签引用是否真实存在。

    和函数引用同理，但后果更隐蔽：写错一个字母，游戏不会报错，只会让
    `if block ... #trit_bridge:solid` 永远为假 —— 表现是「AI 看不见墙」，
    而不是「数据包坏了」。这种静默失败最耗时间，值得静态查一遍。

    只校验本命名空间的标签；`#minecraft:*` / `#forge:*` 这些看不了。
    """
    src = SRC / name
    data = src / "data"
    ns_name = name
    tdir = data / ns_name / "tags" / "blocks"
    have = {p.stem for p in tdir.glob("*.json")} if tdir.is_dir() else set()

    problems: list[str] = []
    for p in data.rglob("*.mcfunction"):
        text = p.read_text(encoding="utf-8", errors="replace")
        body = "\n".join(l for l in text.splitlines() if not l.lstrip().startswith("#"))
        for m in BLOCK_TAG_REF.finditer(body):
            if m.group(1) != ns_name:
                continue
            # `type=#ns:xxx` 是实体标签，不是方块标签，别误报
            if body[: m.start()].endswith("type="):
                continue
            if m.group(2) not in have:
                problems.append(f"{p.name} -> #{m.group(1)}:{m.group(2)}")
    return problems


def validate_tag_ids(name: str) -> list[str]:
    """检查 tag 文件里每个条目的 id 长得对不对。

    这条检查是有代价换来的：`block_tag()` 已经会补 `minecraft:` 前缀，某次
    调用点又自己补了一遍，于是生成了 `minecraft:minecraft:coal_ore`。
    **JSON 是合法的，方块也全都真实存在**，所以既没有解析错误也没有缺引用 ——
    游戏只在后台记一行 `Couldn't read tag list trit_bridge:ores`，然后把这个
    tag 当成空集合。表现出来是「AI 在矿洞里睁眼瞎」，而 `!gather` 一切正常。

    静态校验能抓这类错误的唯一办法，是直接检查 id 的**形状**：
    最多一个冒号、只有合法字符、tag 引用以 `#` 开头。
    """
    src = SRC / name
    data = src / "data"
    if not data.is_dir():
        return []

    id_re = re.compile(r"^#?[a-z0-9_.-]+:[a-z0-9_/.-]+$")
    problems: list[str] = []
    for p in data.rglob("tags/**/*.json"):
        try:
            j = json.loads(p.read_text(encoding="utf-8"))
        except Exception as e:  # 解析失败由 validate_json 报，这里不重复
            problems.append(f"{p.name}: JSON 解析失败 {e}")
            continue
        for i, v in enumerate(j.get("values", [])):
            eid = v.get("id") if isinstance(v, dict) else v
            if not isinstance(eid, str):
                problems.append(f"{p.name}[{i}]: 不是字符串也不是 {{id:...}}")
            elif not id_re.match(eid):
                problems.append(f"{p.name}[{i}]: id 形状不对 -> {eid!r}")
    return problems


def validate_nbt_paths(name: str) -> list[str]:
    """检查 `data modify ... <key> set` 之前有没有先把该 key 造出来。

    `data modify` 只能写**已经存在**的 NBT 路径。狼/铁傀儡刚召唤出来时 NBT 里
    没有 `AngryAt`、`Owner`，于是这条命令**静默失败** —— 不报错、不写日志、
    聊天栏也没提示，表现就是「召出来不打人 / 不跟随」。同类静默失败在这个
    仓库里已经有三起（墙方位、`minecraft:minecraft:` 前缀、窗口标题）。

    判据：同一条命令序列里，`data modify X <key> set` 之前必须出现过
    `data merge`，且那段 merge 里带了 `<key>:`。
    """
    src = SRC / name
    data = src / "data"
    if not data.is_dir():
        return []

    WATCHED = {"AngryAt", "Owner", "Tame"}
    mod_re = re.compile(r"data modify .*?\s(\w+)\s+set\b")
    problems: list[str] = []

    def scan(seq: list[tuple[str, str]]) -> None:
        merged: set[str] = set()
        for where, cmd in seq:
            if "data merge" in cmd:
                for k in WATCHED:
                    if f"{k}:" in cmd:
                        merged.add(k)
                continue
            m = mod_re.search(cmd)
            if m and m.group(1) in WATCHED and m.group(1) not in merged:
                problems.append(
                    f"{where}: `data modify ... {m.group(1)} set` 之前没有 "
                    f"`data merge` 建字段，这条会静默失败")

    for p in data.rglob("*.mcfunction"):
        rel = p.relative_to(data)
        lines = [(f"{rel}:{i}", ln) for i, ln in
                 enumerate(p.read_text(encoding="utf-8").splitlines(), 1)
                 if ln.strip() and not ln.lstrip().startswith("#")]
        scan(lines)

    # power 里的 execute_command 也是一条命令序列
    for p in data.rglob("*.json"):
        try:
            j = json.loads(p.read_text(encoding="utf-8"))
        except Exception:
            continue
        cmds: list[str] = []

        def walk(o) -> None:
            if isinstance(o, dict):
                if o.get("type") == "origins:execute_command" and isinstance(o.get("command"), str):
                    cmds.append(o["command"])
                for v in o.values():
                    walk(v)
            elif isinstance(o, list):
                for v in o:
                    walk(v)

        walk(j)
        if cmds:
            rel = p.relative_to(data)
            scan([(f"{rel}[cmd{i}]", c) for i, c in enumerate(cmds, 1)])
    return problems


# tellraw 里的两种片段：`{"text":"<TRIT hp="}` 和 `{"score":{"name":"hp",...}}`
TELLRAW_TEXT = re.compile(r'\{"text":"([^"]*)"\}')
TELLRAW_SCORE = re.compile(r'\{"score":\{"name":"([^"]*)"')


def _rust_known_fields() -> list[str]:
    """从 Rust 解析器里抠出它认得的字段名。

    不手写一份名单来对照 —— 手写名单会跟代码脱节，改名时两边一起过时、
    检查反而变成"两边都错所以通过"。直接从 `parse_patch` 的 match 分支里读，
    代码是唯一事实来源。
    """
    src = ROOT / "src" / "perceive" / "mod.rs"
    if not src.is_file():
        return []
    text = src.read_text(encoding="utf-8")
    start = text.find("match k {")
    if start < 0:
        return []
    # 12 空格缩进 = parse_patch 里 match k 的分支层，别把别的 match 也捞进来
    return re.findall(r'^\s{12}"([a-z0-9_]+)"\s*=>', text[start:], re.M)


def check_tellraw_line(line: str, where: str, known: set[str]) -> list[str]:
    """校验一行 tellraw 传感器输出，返回问题列表（空 = 没问题）。

    查两件事：

    1. text 里声明的协议字段名（`<TRIT hp=` 的 `hp`）必须在 Rust 解析器
       白名单里。拼错一个字母 = Rust 侧静默当「这条通道没报这个量」。
    2. `k=` 标签的个数必须等于 score 片段的个数。多一个标签就是日志里
       写着 `hp=xxx` 却没取值；多一个取值就是算了个没人读的数。

    **不查**标签和取值"是不是同一个量"：score 片段取的是记分板假人名
    （`px` / `hst` / `dst` 之类），名字随便起，跟协议字段名无关，
    比对没有意义 —— 一开始写了这条，结果 24 条全是假阳性。
    """
    labels: list[str] = []
    for m in TELLRAW_TEXT.finditer(line):
        body = m.group(1)
        if body.startswith("<TRIT"):
            body = body.split(" ", 1)[1] if " " in body else ""
        labels += re.findall(r"([a-z0-9_]+)=", body)
    scores = [m.group(1) for m in TELLRAW_SCORE.finditer(line)]

    problems: list[str] = []
    if known:
        for k in labels:
            if k not in known:
                problems.append(
                    f"{where}: 字段 `{k}` Rust 解析器不认识，会当「没感知」丢弃")
    if labels and len(labels) != len(scores):
        problems.append(
            f"{where}: 标签 {len(labels)} 个 / 取值 {len(scores)} 个，对不上 —— "
            f"标签: {labels}  取值: {scores}")
    if not scores:
        problems.append(f"{where}: tellraw 里没有任何 score 片段，这条行是死的")
    return problems


def validate_sensor_fields(name: str) -> list[str]:
    """校验 tellraw 传感器行：text 的 `k=` 标签必须和紧跟着的 score 字段同名，
    且字段名得是 Rust 解析器认得的。

    这里能出两种错，都是**静默**的：

    1. 标签和取值错位 —— `{"text":"<TRIT hp="}` 后面紧跟 `{"score":{"name":"food"}}`。
       日志里照样写着 `hp=170`，其实是饱食度。Rust 侧按 `hp` 解析，拿到的
       是错的数，AI 于是"血量 17 还很健康所以不撤退"。没有任何地方会报错。
    2. 字段名 Rust 侧不认识 —— `p=`、`wd=` 这些是后加的，拼错一个字母就会被
       当成"这条通道没报这个量"，按 `None` 处理，也就是"不知道"。而把
       "不知道"当成"没有"正是这个仓库踩过三次的坑。

    包名不匹配 `trit_*` 时也会跑一遍（白跑，没有 `<TRIT` 行），
    这样将来有人把传感器迁到别的包，检查不会安静地失效。
    """
    src = SRC / name
    known = set(_rust_known_fields())
    problems: list[str] = []

    for p in sorted(src.rglob("*.mcfunction")):
        rel = p.relative_to(src)
        for ln_no, line in enumerate(p.read_text(encoding="utf-8").splitlines(), 1):
            if "tellraw" not in line or "<TRIT" not in line:
                continue
            problems += check_tellraw_line(line, f"{rel}:{ln_no}", known)
    return problems


def selftest_sensor_fields() -> int:
    """让检查器自己被检查。构造三种坏样本，抓不到就是检查器坏了。"""
    known = set(_rust_known_fields())
    if not known:
        print("  [!!] 没从 src/perceive/mod.rs 读到任何字段，检查器形同虚设")
        return 1

    cases = [
        ("字段拼错（Rust 不认识）",
         'tellraw @s [{"text":"<TRIT hpp="},{"score":{"name":"hpp","objective":"trit_bridge"}}]>',
         "不认识"),
        ("整行没有 score 片段",
         'tellraw @s [{"text":"<TRIT hp="}]',
         "死的"),
        ("标签比取值多一个（写了标签没取值）",
         'tellraw @s [{"text":"<TRIT hp="},{"score":{"name":"hp","objective":"trit_bridge"}},'
         '{"text":" food="}]',
         "对不上"),
    ]
    ok = True
    for label, line, must in cases:
        probs = check_tellraw_line(line, "selftest", known)
        hit = any(must in p for p in probs)
        ok &= hit
        print(f"  [{'ok ' if hit else '!! '}] {label}: {'抓到' if hit else '漏了'}")

    # 好样本必须放行：sense.mcfunction 与 sense_fast.mcfunction 的真实行
    good = 0
    for f in (SRC / "trit_bridge").rglob("sense*.mcfunction"):
        for ln in f.read_text(encoding="utf-8").splitlines():
            if "tellraw" in ln and "<TRIT" in ln:
                if check_tellraw_line(ln, "selftest", known):
                    print(f"  [!! ] 好样本被误报: {f.name}")
                    ok = False
                else:
                    good += 1
    print(f"  [{'ok ' if good else '!! '}] 真实传感器行 {good} 条全部放行")
    ok &= good > 0
    print("自测通过。" if ok else "自测失败。")
    return 0 if ok else 1


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
            if not p.is_file():
                continue
            rel = p.relative_to(src).as_posix()
            if rel.startswith("_") or "/_" in rel:
                continue          # 下划线开头是工作目录（_parts 翻译分片等），不进包
            z.write(p, rel)
    with zipfile.ZipFile(out) as z:
        validate_json(z)
    return out


def check_format(path: pathlib.Path, want: int) -> tuple[bool, int]:
    with zipfile.ZipFile(path) as z:
        meta = json.loads(z.read("pack.mcmeta").decode("utf-8"))
    got = meta["pack"]["pack_format"]
    return got == want, got


def install(profile: pathlib.Path, only: set[str] | None = None) -> None:
    """把 DIST 里的数据包装进每个存档。

    `only` 只装指定的几个。排查问题时用得上：新加的数据包把世界搞崩了，
    要"只回滚一个、别的照装"，靠手删 zip 很容易删漏某个存档。
    """
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
            if only and name.name not in only:
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
    ap.add_argument("--only", metavar="NAMES",
                    help="只装这几个（逗号分隔），排查时用")
    ap.add_argument("--selftest", action="store_true",
                    help="先验证检查器自己能抓到错，再打包")
    args = ap.parse_args()

    if args.selftest:
        print("传感器字段检查器自测：")
        rc = selftest_sensor_fields()
        if rc == 0:
            print("（--selftest 只验证检查器，不打包；去掉参数才会打包）")
        sys.exit(rc)

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
        for prob in validate_block_tags(name):
            warnings.append(f"{name}: {prob}（方块标签）")
        for prob in validate_tag_ids(name):
            warnings.append(f"{name}: {prob}（tag 条目）")
        for prob in validate_nbt_paths(name):
            warnings.append(f"{name}: {prob}（NBT 路径）")
        for prob in validate_sensor_fields(name):
            warnings.append(f"{name}: {prob}（传感器字段）")

    if warnings:
        print("\n引用有问题（游戏里不会报错，只会静默失效，建议先修）：")
        for w in warnings:
            print(f"  ! {w}")

    if not ok:
        print("\n有数据包格式不对，别装。")
        sys.exit(1)

    # 起源包要按「模组真实注册的类型」校验一遍：Apoli 解析失败是静默的，
    # 表现出来是「起源选得到但技能栏空着」，或者列表里干脆没有这个种族。
    if "summoner_origin" in names:
        print()
        script = ROOT / "scripts" / "validate_origins.py"
        mods = pathlib.Path(os.environ.get("TRIT_MODS", DEFAULT_MODS))
        if not mods.is_dir():
            print(f"起源校验跳过：模组目录不存在（{mods}）")
        else:
            rc = subprocess.run([sys.executable, str(script), "--mods", str(mods),
                                 "--pack", str(SRC / "summoner_origin")]).returncode
            if rc != 0:
                print("  起源包校验没过，先修再装。")
                sys.exit(rc)

    if args.install:
        profile = pathlib.Path(args.install)
        if not profile.is_dir():
            raise SystemExit(f"实例目录不存在: {profile}")
        only = {x.strip() for x in args.only.split(",") if x.strip()} if args.only else None
        install(profile, only)

    print("\n提示：进入世界后执行 /reload；传感器默认关闭，需要时执行")
    print("      /function trit_bridge:on")


if __name__ == "__main__":
    main()
