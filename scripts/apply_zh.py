#!/usr/bin/env python3
"""按序号把译文回填成 key -> 译文 的分片。

    python scripts/apply_zh.py --list .work/names.tsv --in .work/zh_001.txt --out 001-names

为什么这么绕：

* 1050 条能力名 + 1059 条能力描述，如果每条都手写 `power.origins-plus-plus.
  xxx/yyy.name` 这种 key，光 key 就两万多字符，而且**抄错一个字母等于静默
  失效**（游戏照旧英文，不报错不写日志）。
* 所以先把英文按固定顺序导出成 TSV（`序号<TAB>key<TAB>英文`），翻译时只写
  `序号<TAB>中文`。序号对得上就对得上，对不上直接报错，不可能错位。
* 每条都核：序号在范围内、key 在英文表里、译文非空、不是纯 ASCII。
"""
from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
PARTS = ROOT / "datapacks" / "origins_zh" / "_parts"
CJK = re.compile(r"[\u4e00-\u9fff]")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--list", required=True, help="TSV：序号<TAB>key<TAB>英文")
    ap.add_argument("--in", dest="src", required=True, help="译文：序号<TAB>中文")
    ap.add_argument("--out", required=True, help="输出分片名（不含 .json）")
    ap.add_argument("--keep-english", action="store_true",
                    help="允许整条保留英文（专有名词/拟声词）")
    a = ap.parse_args()

    rows: dict[int, str] = {}
    for line in pathlib.Path(a.list).read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        i, key, _en = line.split("\t", 2)
        rows[int(i)] = key

    out: dict[str, str] = {}
    bad: list[str] = []
    lines = pathlib.Path(a.src).read_text(encoding="utf-8").splitlines()
    for ln in lines:
        if not ln.strip():
            continue
        if "\t" not in ln:
            bad.append(f"没有 TAB 分隔: {ln[:60]}")
            continue
        n, txt = ln.split("\t", 1)
        n = n.strip()
        if not n.isdigit():
            bad.append(f"序号不是数字: {ln[:60]}")
            continue
        idx = int(n)
        if idx not in rows:
            bad.append(f"序号越界: {idx}")
            continue
        txt = txt.strip()
        if not txt:
            bad.append(f"空译文: {idx}")
            continue
        # TSV 是按行存的，原文里的换行用 ⏎ 占位；游戏里 tooltip 的换行必须还原
        txt = txt.replace("\u23ce", "\n")
        if not CJK.search(txt) and not a.keep_english:
            bad.append(f"没有汉字: {idx} -> {txt[:40]}")
            continue
        out[rows[idx]] = txt

    if bad:
        print(f"有问题的行 {len(bad)} 条：")
        for b in bad[:15]:
            print("  !", b)
        if len(bad) > 15:
            print(f"  ... 还有 {len(bad) - 15} 条")
        return 1

    PARTS.mkdir(parents=True, exist_ok=True)
    p = PARTS / f"{a.out}.json"
    p.write_text(json.dumps(out, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    print(f"回填 {len(out)} 条 -> {p.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
