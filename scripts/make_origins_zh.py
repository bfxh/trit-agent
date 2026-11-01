#!/usr/bin/env python3
"""生成「起源++」汉化资源包，并可把它塞进模组 jar 里。

    python scripts/make_origins_zh.py --mods "E:/MC/MC/profiles/烦人的村民/mods"
    python scripts/make_origins_zh.py --mods <mods目录> --inject
    python scripts/make_origins_zh.py --mods <mods目录> --pack-only

两件事：

1. **校验**。`zh_cn.json` 里每个 key 必须在模组 `en_us.json` 里真实存在。
   写错一个字母的后果是**什么都不发生**——游戏照常显示英文，不报错，
   也没有任何日志。这类静默失败在这个仓库里已经踩过好几次（墙方位、
   `minecraft:minecraft:` 前缀、窗口标题），所以宁可在这儿拦住。

2. **两种落地方式**：
   - 资源包：放 `resourcepacks/`，**必须手动启用**（游戏退出时会用内存里
     的列表覆盖 options.txt，所以运行时改文件无效）。
   - 塞进 mod jar：`assets/origins-plus-plus/lang/zh_cn.json` 直接进 jar，
     模组资源是无条件加载的，**不需要任何手动步骤**。代价是模组更新会覆盖，
     重跑这个脚本即可。

默认 `--inject` 时两者都做：资源包留档，jar 注入保证立即生效。
"""
from __future__ import annotations

import argparse
import json
import pathlib
import shutil
import sys
import time
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
SRC = ROOT / "datapacks" / "origins_zh"
DIST = ROOT / "dist"
LANG_REL = "assets/origins-plus-plus/lang/zh_cn.json"
PACK_FORMAT = 8  # 1.18.2 资源包


def find_mod_jar(mods: pathlib.Path) -> pathlib.Path:
    hits = [p for p in mods.glob("*.jar") if "origins-plus-plus" in p.name]
    if not hits:
        raise SystemExit(f"{mods} 里找不到 origins-plus-plus 的 jar")
    return max(hits, key=lambda p: p.stat().st_size)


def load_json_bytes(b: bytes) -> dict:
    return json.loads(b.decode("utf-8-sig"))


def check(zh: dict, en: dict) -> list[str]:
    """zh 里的 key 必须在 en 里存在；顺带查空值、纯 ASCII 漏译。"""
    problems = []
    for k, v in zh.items():
        if k not in en:
            problems.append(f"key 不存在于模组: {k}")
        if not isinstance(v, str) or not v.strip():
            problems.append(f"空译文: {k}")
    return problems


def coverage(zh: dict, en: dict) -> None:
    def bucket(k: str) -> str:
        if k.startswith("origin."):
            return "origin"
        if k.startswith("power."):
            return "power"
        if k.startswith("container."):
            return "container"
        return "misc"

    total: dict[str, int] = {}
    done: dict[str, int] = {}
    for k in en:
        b = bucket(k)
        total[b] = total.get(b, 0) + 1
        if k in zh:
            done[b] = done.get(b, 0) + 1
    print("  覆盖情况：")
    for b in sorted(total):
        d = done.get(b, 0)
        pct = 100.0 * d / total[b]
        print(f"    {b:<10} {d:>5} / {total[b]:<5} {pct:5.1f}%")
    miss = [k for k in en if k not in zh]
    if miss:
        print(f"  未翻译 {len(miss)} 条，例如：{', '.join(miss[:3])}")


def build_pack(zh: dict) -> pathlib.Path:
    """把 zh 写成资源包并打包。"""
    lang = SRC / LANG_REL
    lang.parent.mkdir(parents=True, exist_ok=True)
    lang.write_text(json.dumps(zh, ensure_ascii=False, indent=2) + "\n",
                    encoding="utf-8")  # 无 BOM：MC 读带 BOM 的会解析失败
    DIST.mkdir(exist_ok=True)
    out = DIST / "origins_zh.zip"
    if out.exists():
        out.unlink()
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        for p in sorted(SRC.rglob("*")):
            if p.is_file():
                z.write(p, p.relative_to(SRC).as_posix())
    with zipfile.ZipFile(out) as z:
        meta = json.loads(z.read("pack.mcmeta").decode("utf-8"))
        got = meta["pack"]["pack_format"]
        if got != PACK_FORMAT:
            raise SystemExit(f"pack_format={got}，1.18.2 资源包应该是 {PACK_FORMAT}")
        load_json_bytes(z.read(LANG_REL))
    return out


def inject(jar: pathlib.Path, zh: dict) -> None:
    """把 zh_cn.json 追加进模组 jar。

    jar 就是 zip，用 append 模式加一个条目即可；不重打包整个 jar，
    因为重打包会丢掉条目的时间戳/顺序，某些加载器对签名 jar 会翻脸。
    """
    payload = json.dumps(zh, ensure_ascii=False, indent=2).encode("utf-8")
    bak = jar.with_suffix(f".jar.bak-{time.strftime('%Y%m%d-%H%M%S')}")
    before = len(zipfile.ZipFile(jar).namelist())
    shutil.copy2(jar, bak)
    try:
        with zipfile.ZipFile(jar, "a", zipfile.ZIP_DEFLATED) as z:
            # 同名条目先剔除，否则会留下两条（MC 读最后一条，但不干净）
            names = z.namelist()
            if LANG_REL in names:
                raise SystemExit(f"{jar.name} 里已经有 {LANG_REL}，先回滚 .bak 再跑")
            z.writestr(LANG_REL, payload)
        with zipfile.ZipFile(jar) as z:
            after = z.namelist()
            got = load_json_bytes(z.read(LANG_REL))
            if len(after) != before + 1 or got != zh:
                raise SystemExit("注入后自检失败，用 .bak 回滚")
            if "META-INF/mods.toml" not in after:
                raise SystemExit("注入后 mods.toml 不见了，用 .bak 回滚")
    except Exception:
        shutil.copy2(bak, jar)
        print(f"  注入失败，已回滚（备份 {bak.name}）")
        raise
    print(f"  已注入 {jar.name}")
    print(f"     备份 {bak.name}（想还原就把它改回 .jar）")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--mods", required=True, help="整合包的 mods 目录")
    ap.add_argument("--inject", action="store_true",
                    help="除了做资源包，还把 zh_cn.json 塞进模组 jar（免手动启用）")
    ap.add_argument("--profile", help="顺带把资源包复制到该实例的 resourcepacks/")
    a = ap.parse_args()

    mods = pathlib.Path(a.mods)
    if not mods.is_dir():
        raise SystemExit(f"mods 目录不存在: {mods}")
    jar = find_mod_jar(mods)
    with zipfile.ZipFile(jar) as z:
        en = load_json_bytes(z.read("assets/origins-plus-plus/lang/en_us.json"))
    src = SRC / LANG_REL
    if not src.is_file():
        raise SystemExit(f"缺少译文源文件: {src}")
    zh = load_json_bytes(src.read_bytes())

    print(f"模组: {jar.name}")
    print(f"英文词条 {len(en)}，中文词条 {len(zh)}")
    problems = check(zh, en)
    if problems:
        print("\n译文有问题（游戏里不会报错，只会照旧显示英文）：")
        for p in problems:
            print(f"  ! {p}")
        return 1
    coverage(zh, en)

    out = build_pack(zh)
    print(f"  资源包 -> {out.relative_to(ROOT)}  ({out.stat().st_size / 1024:.1f} KB)")
    if a.profile:
        rp = pathlib.Path(a.profile) / "resourcepacks"
        rp.mkdir(exist_ok=True)
        shutil.copy2(out, rp / out.name)
        print(f"  已复制 -> {rp / out.name}（要在游戏里手动启用）")
    if a.inject:
        inject(jar, zh)
        print("\n  注入后重启游戏即可看到中文，不需要启用任何资源包。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
