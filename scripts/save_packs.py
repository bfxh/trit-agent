#!/usr/bin/env python3
"""查看 / 修改存档的数据包启用列表（level.dat 的 DataPacks.Enabled）。

    python scripts/save_packs.py status  "E:/MC/MC/profiles/烦人的村民"
    python scripts/save_packs.py enable  "E:/MC/MC/profiles/烦人的村民" --save "新的世界"
    python scripts/save_packs.py enable  "E:/MC/MC/profiles/烦人的村民" --all

为什么要这个脚本：把 zip 拷进 `saves/<世界>/datapacks/` **不等于启用**。
MC 只加载 `level.dat` 里 `DataPacks.Enabled` 列出的包，新拷进去的默认关闭，
而且**不报错、不提示**——表现就是「起源列表里没有这个起源」「trit_bridge 一条
数据都推不出来」，看着像模组坏了，其实是包没开。

`status` 是只读的，游戏开着也能跑。`enable` 会写 level.dat，**必须先退出游戏**
（游戏把 level.dat 映射在内存里，退出时会整个覆盖写回，运行期改磁盘等于白改）。
"""
from __future__ import annotations

import argparse
import gzip
import os
import pathlib
import shutil
import struct
import sys
import time

TAG_END, TAG_BYTE, TAG_SHORT, TAG_INT, TAG_LONG = 0, 1, 2, 3, 4
TAG_FLOAT, TAG_DOUBLE, TAG_BYTE_ARRAY, TAG_STRING = 5, 6, 7, 8
TAG_LIST, TAG_COMPOUND, TAG_INT_ARRAY, TAG_LONG_ARRAY = 9, 10, 11, 12

# 资源包（没有 data/ 的）单独处理，不进 datapacks/。当前状态机工程无资源包。
RESOURCE_PACKS: set[str] = set()


class Reader:
    """NBT 读取器：返回值，同时记下每个节点在缓冲区里的字节区间。

    `spans[("Data","DataPacks","Enabled")]` 就是那个列表的 `[start, end)`。
    写回时只把这个区间原地替换掉，其余字节一个都不动——不做整树重序列化，
    因为 NBT 里 `byte` / `int` / `long` 在 Python 侧都是同一坨数字，重写时
    类型信息会丢（`hardcore`、`Difficulty` 都是 byte，写成 int 游戏就报错）。
    """

    def __init__(self, buf: bytes) -> None:
        self.b = buf
        self.i = 0
        self.spans: dict[tuple[str, ...], tuple[int, int]] = {}

    def u1(self) -> int:
        v = self.b[self.i]
        self.i += 1
        return v

    def u2(self) -> int:
        v = struct.unpack_from(">H", self.b, self.i)[0]
        self.i += 2
        return v

    def u4(self) -> int:
        v = struct.unpack_from(">i", self.b, self.i)[0]
        self.i += 4
        return v

    def name(self) -> str:
        n = self.u2()
        s = self.b[self.i:self.i + n].decode("utf-8", "replace")
        self.i += n
        return s

    def payload(self, t: int, path: tuple[str, ...] = ()):
        start = self.i
        if t == TAG_BYTE:
            v = self.u1()
            v = v - 256 if v >= 128 else v
        elif t == TAG_SHORT:
            v = self.u2()
        elif t == TAG_INT:
            v = self.u4()
        elif t == TAG_LONG:
            v = struct.unpack_from(">q", self.b, self.i)[0]
            self.i += 8
        elif t == TAG_FLOAT:
            v = struct.unpack_from(">f", self.b, self.i)[0]
            self.i += 4
        elif t == TAG_DOUBLE:
            v = struct.unpack_from(">d", self.b, self.i)[0]
            self.i += 8
        elif t == TAG_BYTE_ARRAY:
            n = self.u4()
            v = self.b[self.i:self.i + n]
            self.i += n
        elif t == TAG_STRING:
            v = self.name()
        elif t == TAG_LIST:
            et = self.u1()
            n = self.u4()
            v = [self.payload(et, path + (f"[{k}]",)) for k in range(n)]
        elif t == TAG_COMPOUND:
            v = {}
            while True:
                tt = self.u1()
                if tt == TAG_END:
                    break
                k = self.name()
                v[k] = self.payload(tt, path + (k,))
        elif t == TAG_INT_ARRAY:
            n = self.u4()
            v = [struct.unpack_from(">i", self.b, self.i + 4 * k)[0] for k in range(n)]
            self.i += 4 * n
        elif t == TAG_LONG_ARRAY:
            n = self.u4()
            v = [struct.unpack_from(">q", self.b, self.i + 8 * k)[0] for k in range(n)]
            self.i += 8 * n
        else:
            raise ValueError(f"未知 tag 类型 {t} @ {start}")
        self.spans[path] = (start, self.i)
        return v


def load_level(path: pathlib.Path) -> tuple[bytes, dict, Reader]:
    raw = gzip.decompress(path.read_bytes())
    r = Reader(raw)
    t = r.u1()
    if t != TAG_COMPOUND:
        raise SystemExit(f"{path} 不是 NBT 根 compound（type={t}）")
    r.name()
    root = r.payload(TAG_COMPOUND)
    return raw, root, r


def find_enabled_span(reader: Reader) -> tuple[int, int]:
    """取 `Data.DataPacks.Enabled` 这个 list 的字节区间 [start, end)。"""
    span = reader.spans.get(("Data", "DataPacks", "Enabled"))
    if span is None:
        raise SystemExit("找不到 Data.DataPacks.Enabled")
    return span


def encode_enabled(names: list[str]) -> bytes:
    out = bytearray()
    out += struct.pack(">Bi", TAG_STRING, len(names))
    for s in names:
        b = s.encode("utf-8")
        out += struct.pack(">H", len(b)) + b
    return bytes(out)


def patches(profile: pathlib.Path) -> list[tuple[str, pathlib.Path]]:
    out = []
    for s in sorted((profile / "saves").glob("*")):
        if (s / "level.dat").is_file():
            out.append((s.name, s / "level.dat"))
    return out


def custom_packs(world_dir: pathlib.Path) -> list[str]:
    """存档里真正能当数据包用的 zip。

    资源包（只有 assets/、没有 data/）放进 datapacks 只会多个空包，
    还会在日志里留一行警告，所以这里直接滤掉。
    """
    dp = world_dir / "datapacks"
    if not dp.is_dir():
        return []
    return sorted(p.name for p in dp.iterdir()
                  if p.suffix == ".zip" and p.stem not in RESOURCE_PACKS)


def cmd_status(profile: pathlib.Path) -> int:
    rows = []
    for name, lv in patches(profile):
        _, root, _r = load_level(lv)
        d = root.get("Data", root)
        enabled = d.get("DataPacks", {}).get("Enabled", [])
        enabled = [e for e in enabled if e.startswith("file/")]
        files = custom_packs(lv.parent)
        rows.append((name, files, enabled))
    if not rows:
        print("没有找到存档")
        return 1
    for name, files, enabled in rows:
        print(f"=== {name}")
        if not files:
            print("    数据包目录为空")
            continue
        for f in files:
            on = f"file/{f}" in enabled
            print(f"    {'[开]' if on else '[关]'} {f}")
        for e in enabled:
            if e[len("file/"):] not in files:
                print(f"    [!] {e} 已启用但文件不存在")
    return 0


def game_running() -> bool:
    try:
        out = os.popen('tasklist /FI "IMAGENAME eq javaw.exe" /NH').read()
    except Exception:
        return False
    return "javaw" in out.lower()


def cmd_enable(profile: pathlib.Path, only: str | None, all_saves: bool,
               force: bool = False) -> int:
    if game_running() and not force:
        print("游戏还在运行（javaw.exe）。现在写 level.dat 会被退出时的存档整个覆盖掉，")
        print("先完全退出游戏，再跑这条命令。")
        return 2
    targets = patches(profile)
    if only:
        targets = [t for t in targets if t[0] == only]
        if not targets:
            print(f"没有叫 {only!r} 的存档")
            return 1
    elif not all_saves:
        print("要么 --save <存档名>，要么 --all")
        return 1

    rc = 0
    for name, lv in targets:
        raw, root, rd = load_level(lv)
        d = root.get("Data", root)
        cur = list(d.get("DataPacks", {}).get("Enabled", []))
        add = [f"file/{f}" for f in custom_packs(lv.parent) if f"file/{f}" not in cur]
        if not add:
            print(f"=== {name}  已是最新，跳过")
            continue
        new = cur + add
        start, end = find_enabled_span(rd)
        patched = raw[:start] + encode_enabled(new) + raw[end:]

        # 写前自检：解回来必须能读、Enabled 必须对、其他关键字段必须还在
        probe = Reader(patched)
        pt = probe.u1()
        probe.name()
        proot = probe.payload(TAG_COMPOUND)
        pd = proot.get("Data", proot)
        got = pd.get("DataPacks", {}).get("Enabled", [])
        if got != new:
            print(f"=== {name}  自检失败：写回后 Enabled 不一致，放弃")
            rc = 1
            continue
        for k in ("LevelName", "Version", "LastPlayed", "GameRules"):
            if k in d and k not in pd:
                print(f"=== {name}  自检失败：{k} 丢了，放弃")
                rc = 1
                break
        else:
            bak = lv.with_suffix(f".dat.bak-{time.strftime('%Y%m%d-%H%M%S')}")
            shutil.copy2(lv, bak)
            lv.write_bytes(gzip.compress(patched, mtime=0))
            print(f"=== {name}  启用 {len(add)} 个：{', '.join(add)}")
            print(f"     备份 {bak.name}")
    return rc


def cmd_disable(profile: pathlib.Path, save: str) -> int:
    """把所有 file/ 包关掉（排查「是不是我的包把世界搞崩了」用）。"""
    if game_running():
        print("游戏在运行，先退出")
        return 2
    for name, lv in patches(profile):
        if name != save:
            continue
        raw, root, rd = load_level(lv)
        d = root.get("Data", root)
        cur = list(d.get("DataPacks", {}).get("Enabled", []))
        new = [e for e in cur if not e.startswith("file/")]
        if new == cur:
            print(f"=== {name}  本来就没有自定义包")
            return 0
        start, end = find_enabled_span(rd)
        patched = raw[:start] + encode_enabled(new) + raw[end:]
        bak = lv.with_suffix(f".dat.bak-{time.strftime('%Y%m%d-%H%M%S')}")
        shutil.copy2(lv, bak)
        lv.write_bytes(gzip.compress(patched, mtime=0))
        print(f"=== {name}  已关掉 {len(cur) - len(new)} 个，备份 {bak.name}")
        return 0
    print(f"没有叫 {save!r} 的存档")
    return 1


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("action", choices=["status", "enable", "disable"])
    ap.add_argument("profile")
    ap.add_argument("--save", metavar="NAME")
    ap.add_argument("--all", action="store_true")
    ap.add_argument("--force", action="store_true",
                    help="即使检测到 javaw.exe 也照写（只在测试副本上用，真存档会白改）")
    a = ap.parse_args()
    p = pathlib.Path(a.profile)
    if not p.is_dir():
        raise SystemExit(f"实例目录不存在: {p}")
    if a.action == "status":
        return cmd_status(p)
    if a.action == "enable":
        return cmd_enable(p, a.save, a.all, a.force)
    return cmd_disable(p, a.save or "")


if __name__ == "__main__":
    sys.exit(main())
