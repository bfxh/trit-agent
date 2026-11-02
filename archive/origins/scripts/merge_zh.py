#!/usr/bin/env python3
"""把 `datapacks/origins_zh/_parts/*.json` 合并进 zh_cn.json，并逐条校验。

    python scripts/merge_zh.py --mods "E:/MC/MC/profiles/烦人的村民/mods"

为什么要有这个脚本：

* 2000 多条译文不可能一次写完，分片写（每个文件一批）后合并，重跑幂等。
* **key 打错一个字母 = 静默失效**：游戏照旧显示英文，不报错、不写日志。
  所以每条都要对着模组 `en_us.json` 核一遍 key 是否存在，不存在的直接
  报出来并且拒绝合并——宁可在这儿炸，也不要交付一堆没生效的译文。
* 顺带查「译文里一个汉字都没有」的条目（典型的复制粘贴漏改）。
"""
from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
PARTS = ROOT / "datapacks" / "origins_zh" / "_parts"
LANG = ROOT / "datapacks" / "origins_zh" / "assets" / "origins-plus-plus" / "lang" / "zh_cn.json"

CJK = re.compile(r"[\u4e00-\u9fff]")


def load_en(mods: pathlib.Path) -> dict:
    hits = [p for p in mods.glob("*.jar") if "origins-plus-plus" in p.name]
    if not hits:
        raise SystemExit(f"{mods} 里找不到 origins-plus-plus 的 jar")
    jar = max(hits, key=lambda p: p.stat().st_size)
    with zipfile.ZipFile(jar) as z:
        return json.loads(z.read("assets/origins-plus-plus/lang/en_us.json").decode("utf-8-sig"))


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--mods", required=True)
    ap.add_argument("--allow-unknown", action="store_true",
                    help="只警告不拦截（默认拦截）")
    a = ap.parse_args()

    en = load_en(pathlib.Path(a.mods))
    base = json.loads(LANG.read_text(encoding="utf-8")) if LANG.is_file() else {}

    files = sorted(PARTS.glob("*.json"))
    if not files:
        raise SystemExit(f"{PARTS} 里没有分片")

    merged = dict(base)
    unknown: list[str] = []
    ascii_only: list[str] = []
    for f in files:
        try:
            d = json.loads(f.read_text(encoding="utf-8"))
        except json.JSONDecodeError as e:
            print(f"! {f.name} 不是合法 JSON: {e}")
            return 1
        if not isinstance(d, dict):
            print(f"! {f.name} 顶层不是对象")
            return 1
        for k, v in d.items():
            if k not in en:
                unknown.append(f"{f.name}: {k}")
                continue
            if not isinstance(v, str) or not v.strip():
                ascii_only.append(f"{f.name}: {k} 空译文")
                continue
            if not CJK.search(v):
                ascii_only.append(f"{f.name}: {k} -> {v[:40]}")
            merged[k] = v

    print(f"分片 {len(files)} 个，合并后 {len(merged)} / {len(en)} 条 "
          f"({100.0 * len(merged) / len(en):.1f}%)")
    if unknown:
        print(f"\nkey 在模组里不存在（{len(unknown)} 条，这些译文永远不会显示）：")
        for u in unknown[:20]:
            print("  !", u)
        if len(unknown) > 20:
            print(f"  ... 还有 {len(unknown) - 20} 条")
        if not a.allow_unknown:
            print("\n拒绝合并。改对 key 或加 --allow-unknown。")
            return 1
    if ascii_only:
        print(f"\n疑似没翻（{len(ascii_only)} 条）：")
        for u in ascii_only[:10]:
            print("  ?", u)

    LANG.write_text(json.dumps(merged, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    miss = [k for k in en if k not in merged]
    if miss:
        print(f"\n仍未翻译 {len(miss)} 条，例如：")
        for k in miss[:8]:
            print("   ", k, "=", en[k][:60])
    return 0


if __name__ == "__main__":
    sys.exit(main())
