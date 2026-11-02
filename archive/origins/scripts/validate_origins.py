#!/usr/bin/env python3
"""校验起源数据包：类型名是否真实注册、必填字段在不在、引用有没有落空。

为什么要有这个脚本：Apoli 解析 power 失败时是「静默降级」的 —— 控制台最多一行
`Could not read power xxx`，游戏里表现为「起源选得到但技能栏是空的」，或者
「起源列表里根本没有这个种族」。这两件事从游戏外部完全看不出来。

校验依据不是文档，而是**当前装的这版模组里真实注册的 id**：把 apoli 与
origins 里那些「注册表类」的常量池抠出来，用它们比对数据包。

    python scripts/validate_origins.py             # 校验本仓库的 summoner_origin
    python scripts/validate_origins.py --all       # 连模组自带的起源一起校验（自检用）
"""

from __future__ import annotations

import argparse
import io
import json
import pathlib
import re
import sys
import zipfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
DEFAULT_MODS = pathlib.Path(r"E:/MC/MC/profiles/烦人的村民/mods")
DEFAULT_PACK = ROOT / "datapacks" / "summoner_origin"

# 最短 2 个字符：`or` 就是 2 个，卡到 3 个会把 origings:or 误判成未注册
ID_RE = re.compile(r"^[a-z][a-z0-9_]{1,30}$")

# 注册了 id 的类。单看 apoli 的 ApoliPowers 不够：Origins 本体还注册了一批
# （water_breathing、ingredient 之类），漏掉就会把它们误报成未注册。
REGISTRY_HINTS = (
    "ApoliPowers", "ApoliEntityActions", "ApoliItemActions", "ApoliBlockActions",
    "ApoliBiEntityActions", "ApoliDefaultActions", "ApoliEntityConditions",
    "ApoliItemConditions", "ApoliBlockConditions", "ApoliDamageConditions",
    "ApoliFluidConditions", "ApoliBiomeConditions", "ApoliBiEntityConditions",
    "ApoliDefaultConditions", "OriginsPowerTypes", "OriginsEntityConditions",
    "OriginsItemConditions", "OriginsBlockConditions",
)

# 常量池里混进来的 Java 方法名。这份名单宁可短 ——
# 一开始把 and / choice / if_else 也当噪音剔了，结果 `origins:and` 被判成
# 未注册，属于把真 id 误杀。
NOISE = {
    "accept", "apply", "bootstrap", "create", "get", "identifier",
    "metafactory", "of", "optional", "register", "test", "this", "java",
}


def constant_strings(blob: bytes) -> set[str]:
    """从 class 常量池里抠可打印字符串。够用，不必真解析 class 结构。"""
    return {s.decode("latin1") for s in re.findall(rb"[ -~]{3,}", blob)}


def registries(mods: pathlib.Path) -> dict[str, set[str]]:
    jar = next((p for p in mods.glob("origins-forge-*.jar")), None)
    if jar is None:
        raise SystemExit(f"{mods} 里找不到 origins-forge-*.jar")

    blobs: dict[str, bytes] = {}
    with zipfile.ZipFile(jar) as z:
        for n in z.namelist():
            if n.endswith(".class"):
                blobs[n] = z.read(n)
        nested = [n for n in z.namelist()
                  if n.startswith("META-INF/jarjar/apoli-") and n.endswith(".jar")]
        if not nested:
            raise SystemExit("origins-forge 里没有内嵌 apoli jar，版本变了，脚本要改")
        with zipfile.ZipFile(io.BytesIO(z.read(nested[0]))) as apoli:
            for n in apoli.namelist():
                if n.endswith(".class"):
                    blobs.setdefault(n, apoli.read(n))

    groups: dict[str, set[str]] = {}
    for hint in REGISTRY_HINTS:
        for path, blob in blobs.items():
            if hint not in path.rsplit("/", 1)[-1]:
                continue
            ids = {s for s in constant_strings(blob) if ID_RE.match(s) and s not in NOISE}
            groups.setdefault(hint, set()).update(ids)

    merged: set[str] = set()
    for ids in groups.values():
        merged |= ids
    groups["（合计）"] = merged
    return groups


def iter_types(node):
    """递归吐出声明的所有 type 值。"""
    if isinstance(node, dict):
        if isinstance(node.get("type"), str):
            yield node["type"]
        for v in node.values():
            yield from iter_types(v)
    elif isinstance(node, list):
        for v in node:
            yield from iter_types(v)


def harvest(mods: pathlib.Path) -> set[str]:
    """模组自带数据里真实用到的 type。

    有些 id 在注册表类里抠不出来（旧版遗留、别名注册），但模组自己的数据
    一直在用 —— 拿它们兜底，避免把「官方自己都在用」的写法误报成错误。
    """
    types: set[str] = set()
    files_seen: set[str] = set()
    for jar in sorted(mods.glob("*.jar")):
        if not re.search(r"origin|apoli", jar.name, re.I):
            continue
        try:
            files = load_pack(jar)
        except Exception:
            continue
        files_seen |= set(files)
        for rel, d in files.items():
            if "/powers/" in rel or "/origins/" in rel:
                types |= {strip_ns(t) for t in iter_types(d) if not t.startswith("minecraft:")}
    return types, files_seen


def load_pack(path: pathlib.Path) -> dict[str, dict]:
    """把一个数据包（目录或 zip）读成 {相对路径: json}，只取 data/ 下的。"""
    files: dict[str, dict] = {}
    if path.is_dir():
        for p in sorted(path.rglob("*.json")):
            rel = p.relative_to(path).as_posix()
            if not rel.startswith("data/"):
                continue
            files[rel] = json.loads(p.read_text(encoding="utf-8"))
        return files
    with zipfile.ZipFile(path) as z:
        for n in z.namelist():
            if n.startswith("data/") and n.endswith(".json"):
                files[n] = json.loads(z.read(n).decode("utf-8-sig"))
    return files


def strip_ns(value: str) -> str:
    return value.split(":", 1)[1] if ":" in value else value


def walk_types(node, path: str, known: set[str], problems: list[str], where: str) -> None:
    """递归找所有 "type" 字段，检查它在注册表里是否存在。"""
    if isinstance(node, dict):
        t = node.get("type")
        if isinstance(t, str) and not t.startswith("minecraft:") and strip_ns(t) not in known:
            problems.append(f"{where}: {path or '顶层'} 的 type '{t}' 不在任何注册表里")
        for k, v in node.items():
            if k == "type":
                continue
            walk_types(v, f"{path}.{k}" if path else k, known, problems, where)
    elif isinstance(node, list):
        for i, v in enumerate(node):
            walk_types(v, f"{path}[{i}]", known, problems, where)


def as_list(v) -> list:
    """attribute 的 codec 两种都收：modifier 是单个对象，modifiers 是数组。"""
    if v is None:
        return []
    return v if isinstance(v, list) else [v]


def check_power(rel: str, d: dict, known: set[str], problems: list[str], warn: list[str]) -> None:
    short = strip_ns(str(d.get("type", "")))
    where = rel.split("/")[-1]

    if short == "attribute":
        mods = as_list(d.get("modifiers", d.get("modifier")))
        if not mods:
            problems.append(f"{where}: origins:attribute 既没有 modifiers 也没有 modifier")
        for i, m in enumerate(mods):
            if not isinstance(m, dict):
                problems.append(f"{where}: modifier[{i}] 不是对象")
                continue
            for field in ("attribute", "operation", "value"):
                if field not in m:
                    problems.append(f"{where}: modifier[{i}] 缺 '{field}'（codec 里是必填）")
    elif short == "active_self":
        if "entity_action" not in d:
            problems.append(f"{where}: origins:active_self 缺 entity_action")
        key = d.get("key")
        if isinstance(key, str) and not key.startswith("key."):
            warn.append(f"{where}: key '{d['key']}' 不像一个按键绑定 id")

    walk_types(d, "", known, problems, where)


def check_origin(rel: str, d: dict, problems: list[str], warn: list[str],
                 all_files: set[str], own_ns: set[str], global_files: set[str]) -> None:
    where = rel.split("/")[-1]
    powers = d.get("powers", [])
    if not isinstance(powers, list):
        problems.append(f"{where}: origin 的 powers 必须是数组（现在是 "
                        f"{type(powers).__name__}）")
        return
    has_ns_powers = {rel.split("/")[1] for rel in all_files if rel.startswith("data/") and "/powers/" in rel}
    for pid in powers:
        ns, _, p = str(pid).partition(":")
        target = f"data/{ns}/powers/{p}.json"
        if target in all_files:
            continue
        # 只有「本包自己就写了这个命名空间的 powers/」时，缺失才算错；
        # 否则是引用别的模组（origins++ 的起源大量引用 origins: 的 power）
        if ns in has_ns_powers:
            problems.append(f"{where}: 引用的 {pid} 找不到（应是 {target}）")
        elif target not in global_files:
            # 跨模组引用：只在这个 id 全盘都不存在时才算错
            problems.append(f"{where}: 引用的 {pid} 在所有已装模组里都不存在")

    impact = d.get("impact")
    if impact is not None and impact not in (0, 1, 2, 3):
        problems.append(f"{where}: impact={impact}，只能是 0/1/2/3")

    for field in ("name", "description"):
        if field in d and not isinstance(d[field], str):
            problems.append(f"{where}: {field} 必须是字符串")


def check_layer(rel: str, d: dict, problems: list[str], all_files: set[str],
                own_ns: set[str]) -> None:
    """起源层：把自己命名空间下的条目和起源文件对上。

    跨模组的条目（origins:human 之类）这里不管 —— 它们不在本包里，
    查了只会制造假错误。
    """
    where = rel.split("/")[-1]
    if d.get("replace") is True:
        problems.append(f"{where}: replace=true 会把别的模组已登记的起源整层顶掉，"
                        f"选择界面只剩自己的 —— 加起源要用 replace:false")
    entries = d.get("origins")
    if not isinstance(entries, list):
        problems.append(f"{where}: 层的 origins 必须是数组")
        return
    own_origins = {rel.split("/")[1] for rel in all_files if rel.startswith("data/") and "/origins/" in rel}
    for oid in entries:
        ns, _, p = str(oid).partition(":")
        if ns not in own_origins:
            continue
        if f"data/{ns}/origins/{p}.json" not in all_files:
            problems.append(f"{where}: 层里登记了 {oid}，但找不到 data/{ns}/origins/{p}.json")


def validate(label: str, files: dict[str, dict], known: set[str],
             global_files: set[str] | None = None) -> int:
    problems: list[str] = []
    warn: list[str] = []
    names = set(files)
    own_ns = {rel.split("/")[1] for rel in names if rel.startswith("data/")}
    n_power = n_origin = n_layer = 0

    for rel, d in files.items():
        if not isinstance(d, dict):
            continue
        if "/tags/" in rel:            # 标签不是 power，别拿 schema 套它
            continue
        if "/origin_layers/" in rel:
            n_layer += 1
            check_layer(rel, d, problems, names, own_ns)
        elif "/powers/" in rel:
            n_power += 1
            check_power(rel, d, known, problems, warn)
        elif "/origins/" in rel:
            n_origin += 1
            check_origin(rel, d, problems, warn, names, own_ns, global_files or names)

    used = {d["type"] for d in files.values()
            if isinstance(d, dict) and isinstance(d.get("type"), str)}

    print(f"=== {label}")
    print(f"    起源 {n_origin} 个，能力 {n_power} 个，层 {n_layer} 个，"
          f"用到 {len(used)} 种 power 类型")
    for w in warn:
        print(f"    [注意] {w}")
    for p in problems:
        print(f"    [错误] {p}")
    if not problems:
        print("    通过")
    return len(problems)


def selftest(known: set[str]) -> int:
    """故意造坏样本，确认校验器真的会报错。

    没有这一步的校验器是不可信的 —— validate_nbt_paths 当初就是先写了正例
    才发现自己漏判。
    """
    import tempfile

    cases = {
        "type 写错一个字母": ({"type": "origins:actve_self", "entity_action": {}}, True),
        "active_self 缺 entity_action": ({"type": "origins:active_self"}, True),
        "attribute 的 modifier 缺 value": (
            {"type": "origins:attribute",
             "modifier": {"attribute": "minecraft:generic.max_health",
                          "operation": "addition"}}, True),
        "合规的 active_self": (
            {"type": "origins:active_self",
             "entity_action": {"type": "origins:and", "actions": []}}, False),
    }
    bad = 0
    with tempfile.TemporaryDirectory() as td:
        root = pathlib.Path(td)
        for name, (power, expect_error) in cases.items():
            d = root / name.replace(" ", "_") / "data" / "t" / "powers"
            d.mkdir(parents=True)
            (d / "p.json").write_text(json.dumps(power, ensure_ascii=False), encoding="utf-8")
            files = load_pack(root / name.replace(" ", "_"))
            problems: list[str] = []
            for rel, doc in files.items():
                check_power(rel, doc, known, problems, [])
            got = bool(problems)
            flag = "ok " if got == expect_error else "FAIL"
            if got != expect_error:
                bad += 1
            print(f"  [{flag}] {name}: {'报错' if got else '通过'}")
    return bad


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--mods", default=str(DEFAULT_MODS))
    ap.add_argument("--pack", action="append", default=None,
                    help="要校验的数据包目录/zip，可重复；默认校验 summoner_origin")
    ap.add_argument("--all", action="store_true", help="额外校验模组自带的起源包（自检）")
    ap.add_argument("--selftest", action="store_true", help="用坏样本验证校验器本身")
    a = ap.parse_args()

    mods = pathlib.Path(a.mods)
    groups = registries(mods)
    known = groups.pop("（合计）")
    harvested, global_files = harvest(mods)
    before = len(known)
    known |= harvested
    print(f"注册表：{before} 个 id（来自 {len(groups)} 个注册类），"
          f"另有 {len(known) - before} 个只在模组自带数据里出现")
    print()

    if a.selftest:
        print("校验器自测：")
        bad = selftest(known)
        print("自测通过。" if bad == 0 else f"自测有 {bad} 项不符预期。")
        print()

    total = 0
    for p in (pathlib.Path(x) for x in (a.pack or [DEFAULT_PACK])):
        total += validate(p.name, load_pack(p), known, global_files)

    if a.all:
        for jar in sorted(mods.glob("*.jar")):
            if not re.search(r"origin|apoli", jar.name, re.I):
                continue
            try:
                files = load_pack(jar)
            except Exception as e:
                print(f"=== {jar.name}: 读不了（{type(e).__name__}），跳过")
                continue
            if not any("/powers/" in k or "/origins/" in k for k in files):
                continue
            total += validate(jar.name, files, known, global_files | set(files))

    print()
    print("全部通过。" if total == 0 else f"共 {total} 处问题。")
    return 0 if total == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
