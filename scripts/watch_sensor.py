#!/usr/bin/env python3
"""实时盯传感器：一旦日志里出现 <TRIT ...> 就报「活了」。

    python scripts/watch_sensor.py            # 盯 60 秒
    python scripts/watch_sensor.py --seconds 300

为什么单独做一个脚本：本机没法替玩家敲游戏内命令，所以「命令敲了没有、
敲了之后传感器到底吐没吐包」这一段必须由玩家自己确认。这个脚本把
「读日志找 TRIT」这步自动化，玩家只要看一眼屏幕就知道结果。

判定标准：
  - 出现 `<TRIT ` 且后面跟着 `hp=`  -> 传感器**活了**（慢通道）
  - 出现 `<TRITF `                  -> 快通道也活了（需要 Rust 侧开 fo）
  - 只出现 `<TRIT>` 空的             -> 有人在聊天框手打了这个字符串，
                                        不算传感器输出（这是本脚本最容易
                                        误判的地方，所以单独区分）
"""
from __future__ import annotations

import argparse
import pathlib
import re
import sys
import time

DEFAULT_LOG = r"E:/MC/MC/profiles/烦人的村民/logs/latest.log"

# 慢通道：<TRIT hp=200 food=17 x=1 y=64 z=1 h=0 ...>
RE_SLOW = re.compile(r"<TRIT\s+hp=")
# 快通道：<TRITF wal=5 b1=2 bd=3 p=220>
RE_FAST = re.compile(r"<TRITF\s+wal=")
# 玩家在聊天框手打的空壳
RE_EMPTY = re.compile(r"<TRIT\s*>")


def scan(path: pathlib.Path) -> tuple[int, int, int]:
    """返回 (慢通道行数, 快通道行数, 空壳行数)。"""
    slow = fast = empty = 0
    try:
        data = path.read_bytes()
    except OSError:
        return 0, 0, 0
    text = data.decode("utf-8", errors="replace")
    for line in text.splitlines():
        if RE_SLOW.search(line):
            slow += 1
        elif RE_FAST.search(line):
            fast += 1
        elif RE_EMPTY.search(line):
            empty += 1
    return slow, fast, empty


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--log", default=DEFAULT_LOG)
    ap.add_argument("--seconds", type=int, default=60)
    a = ap.parse_args()
    path = pathlib.Path(a.log)

    if not path.is_file():
        print(f"日志不存在: {path}")
        return 2

    print(f"盯 {path}，最多 {a.seconds} 秒")
    print("现在去游戏里敲：")
    print('    /datapack enable "file/trit_bridge.zip"')
    print('    /datapack enable "file/squad.zip"')
    print("（进到要玩的世界再敲，命令是全局生效的）")
    print()
    print("传感器活了的标志：聊天框开始每秒刷一行 <TRIT hp=... ...>")
    print()

    base_slow, base_fast, base_empty = scan(path)
    print(f"起始基线：慢 {base_slow} / 快 {base_fast} / 空壳 {base_empty}")
    if base_slow:
        print(">>> 已经有慢通道数据了，传感器是活的。")
    print()

    end = time.time() + a.seconds
    try:
        while time.time() < end:
            time.sleep(1.0)
            slow, fast, empty = scan(path)
            ds, df = slow - base_slow, fast - base_fast
            if ds > 0 or df > 0:
                print()
                print("=" * 56)
                print(f">>> 传感器活了！新出现 慢 {ds} 行 / 快 {df} 行")
                print("=" * 56)
                # 打印最近一条看看字段齐不齐
                for line in path.read_bytes().decode("utf-8", "replace").splitlines():
                    if RE_SLOW.search(line):
                        print("样例:", line.strip()[:220])
                        break
                print()
                print("下一步：")
                print("  1. 关掉这个窗口（它只是监视器）")
                print("  2. 双击 trit-agent 目录下的 启动AI.bat")
                print("  3. 切回游戏窗口，按 F8 让 AI 接管")
                print("  4. AI 会自动开始巡逻（mode=patrol）")
                return 0
    except KeyboardInterrupt:
        pass

    slow, fast, empty = scan(path)
    print()
    print(f"{a.seconds} 秒到了：慢 {slow - base_slow} 行 / 快 {fast - base_fast} 行"
          f" / 空壳 {empty - base_empty} 行")
    if empty > base_empty:
        print()
        print("看到的是**空壳** <TRIT> —— 那是有人在聊天框里手打了这三个字符，")
        print("不是传感器输出。传感器仍然没工作。")
    print()
    print("传感器没活的话，按这个顺序查：")
    print("  1. 命令敲了没？必须带引号和 .zip：")
    print('       /datapack enable "file/trit_bridge.zip"')
    print("  2. 敲完有没有报错？（截图看聊天框）")
    print("  3. /datapack list 里 file/trit_bridge.zip 在 enabled 那一组吗？")
    print("  4. 日志里还有没有 'Couldn't load tag minecraft:tick'？")
    print("     有的话是整合包某个 jar 的坏引用废了整个 tick 标签，")
    print("     跑 python scripts/check_env.py 看 [5] 那一项。")
    return 1


if __name__ == "__main__":
    sys.exit(main())
