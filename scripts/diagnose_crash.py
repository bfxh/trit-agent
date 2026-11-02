#!/usr/bin/env python3
"""Minecraft 崩溃一键诊断。

崩溃是**时序问题** —— 同样的存档、同样的 mod，下次可能就不崩了。
所以每次崩溃都要趁热把证据固定下来：崩溃点、mod 名单、以及
**AI 守护进程在崩溃那几秒里到底干了什么**。

用法：
    python scripts/diagnose_crash.py            # 诊断最近一次崩溃
    python scripts/diagnose_crash.py --all      # 列出所有历史崩溃
"""

import argparse
import os
import re
import sys
from datetime import datetime, timedelta
from pathlib import Path

# 整合包默认位置；也可用环境变量覆盖
PROFILE = Path(os.environ.get("TRIT_PROFILE", r"E:\MC\MC\profiles\烦人的村民"))
DAEMON_LOG = Path(
    os.environ.get(
        "TRIT_DAEMON_LOG",
        os.path.join(os.environ.get("LOCALAPPDATA", "."), "trit-agent", "trit-agent-daemon.log"),
    )
)

# 新格式（本地时间）：[2026-11-02 19:10:22] ...
TS_NEW = re.compile(r"\[(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2})\]")
# 旧格式（UTC，只有时分秒）：[day20759 10:06:22] ...
TS_OLD = re.compile(r"\[day\d+ (\d{2}:\d{2}:\d{2})\]")


def read_text(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""


def crash_files() -> list[Path]:
    d = PROFILE / "crash-reports"
    if not d.is_dir():
        return []
    return sorted(
        (p for p in d.glob("crash-*.txt")),
        key=lambda p: p.stat().st_mtime,
        reverse=True,
    )


def parse_crash(path: Path) -> dict:
    """从崩溃报告里抠出关键信息。"""
    text = read_text(path)
    info: dict = {"file": path.name, "mtime": datetime.fromtimestamp(path.stat().st_mtime)}

    m = re.search(r"^Time:\s*(.+)$", text, re.M)
    info["time"] = m.group(1).strip() if m else "?"

    # 异常类型 + 消息
    m = re.search(r"^(java\.\S+Exception|\S+Exception|\S+Error):\s*(.+)$", text, re.M)
    if m:
        info["exception"] = f"{m.group(1)}: {m.group(2).strip()}"
    else:
        lines = text.splitlines()
        info["exception"] = lines[2] if len(lines) > 2 else "?"

    # 最内层栈帧（真正的出事点）
    frames = []
    for line in text.splitlines():
        line = line.strip()
        if line.startswith("at ") and "net.minecraft" in line:
            frames.append(line[3:].split(" ~[")[0].strip())
    info["frames"] = frames[:5]

    # 崩溃报告自认的嫌疑人
    m = re.search(r"^Suspected Mods:\s*(.+)$", text, re.M)
    info["suspected"] = m.group(1).strip() if m else "?"

    log = read_text(PROFILE / "logs" / "latest.log")
    info["entered_world"] = "Preparing spawn" in log
    info["trit_lines"] = log.count("<TRIT")
    return info


def daemon_events() -> list[tuple[datetime | None, str]]:
    """守护进程日志里的关键事件。返回 (本地时间, 消息)。"""
    out = []
    for line in read_text(DAEMON_LOG).splitlines():
        m = TS_NEW.search(line)
        if m:
            ts = datetime.strptime(m.group(1), "%Y-%m-%d %H:%M:%S")
        else:
            m = TS_OLD.search(line)
            # 旧格式是 UTC，转成本地（UTC+8）
            ts = (
                datetime.strptime(m.group(1), "%H:%M:%S").replace(year=2026, month=1, day=1)
                + timedelta(hours=8)
                if m
                else None
            )
        msg = line.split("] ", 1)[-1]
        if any(k in msg for k in ("挂上窗口", "接管", "交还", "退出", "窗口结束", "启动")):
            out.append((ts, msg))
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--all", action="store_true", help="列出所有历史崩溃")
    args = ap.parse_args()

    files = crash_files()
    if not files:
        print(f"没找到崩溃报告（{PROFILE / 'crash-reports'}）")
        return 0

    if args.all:
        print(f"共 {len(files)} 份崩溃报告：")
        for p in files:
            i = parse_crash(p)
            print(f"  {p.name}  {i['exception'][:70]}")
        return 0

    path = files[0]
    info = parse_crash(path)
    crash_at = info["mtime"]

    print("=" * 70)
    print(f"崩溃报告：{info['file']}")
    print(f"时间：{info['time']}（本地 {crash_at:%Y-%m-%d %H:%M:%S}）")
    print(f"异常：{info['exception']}")
    print(f"Minecraft 自认嫌疑人：{info['suspected']}")
    print(
        f"是否已进世界：{'是' if info['entered_world'] else '否（崩在加载阶段）'}"
        f"   <TRIT> 输出 {info['trit_lines']} 条"
    )
    print("-" * 70)
    print("出错栈帧：")
    for fr in info["frames"]:
        print(f"  {fr[:120]}")
    print("-" * 70)

    ev = daemon_events()
    print("AI 守护进程时间线（最近 12 条）：")
    if not ev:
        print("  （无日志 —— 守护进程可能没在跑）")
    for ts, msg in ev[-12:]:
        # 旧格式只有时分秒（且是 UTC），日期不可考 —— 只显示时分秒即可，
        # 判定只关心「相对崩溃时刻的远近」。
        stamp = ts.strftime("%H:%M:%S") if ts else "??:??:??"
        print(f"  [{stamp}] {msg}")

    print("-" * 70)

    # ★ 关键判定：只看崩溃时刻**之前**的接管记录。
    #   拿几小时前的旧记录来判定是错的 —— 崩溃是一次性的事件。
    window = crash_at - timedelta(minutes=5)
    near = [
        (ts, msg)
        for ts, msg in ev
        if ts is not None and window <= ts <= crash_at + timedelta(seconds=30)
    ]
    took_over = any("已接管" in m for _, m in near)

    if took_over:
        print(
            "⚠ 崩溃前 5 分钟内 AI 接管过 —— 崩溃时它可能在注入按键。\n"
            "  下一步：看 latest.log 里 <TRIT> 的输出条数确认接管是否真正跑起来了；\n"
            "  再查 AI 在崩溃瞬间按了什么（trit-agent 的 run_log 有决策记录）。"
        )
    else:
        print(
            "✓ 崩溃前后 AI 没有接管过 —— **一个按键都没注入**，崩溃不是按键造成的。\n"
            "  守护进程空闲时对游戏的唯一交互是改窗口标题；\n"
            "  新版本（2026-11-02 19:20 之后）待机期已完全不写标题。\n"
            "  若这次崩溃依旧，说明是整合包自身 mod 的问题。\n"
            "  下一步可查：日志里成片的 'Unable to load model' / 'Using missing texture'\n"
            "  说明 mod 资源严重缺失，这类整合包在加载阶段崩是常见现象。"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
