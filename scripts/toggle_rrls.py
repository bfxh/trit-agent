#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""禁用/恢复整合包里的 rrls 模组（崩溃真凶）。

## 为什么需要这个

「烦人的村民」整合包会崩在 `Minecraft.handleKeybinds`（Minecraft.java:1836），
栈全是原版帧，看不出是谁干的。字节码级追下来，元凶是整合包自带的
**rrls**（Remove Reloading Screen，作者 dimadencep，modrinth:
https://modrinth.com/mod/rrls）。

它用 mixin 的 `@Redirect` 劫持了 `Minecraft.overlay` 字段的读取：

    // rrls-common.mixins.json -> MinecraftClientMixin
    @Redirect(method = {"tick", "handleInputEvents"},
              at = @At(value = "FIELD", target = "...Minecraft;overlay:..."))
    private Overlay fixOverlayUsage(Minecraft mc) {
        return Rrls.tryGetOverlay(mc.overlay);
    }

而 `Rrls.tryGetOverlay` 的逻辑是（字节码原文）：

    if (overlay instanceof SplashAccessor && ((SplashAccessor) overlay).isAttached())
        return null;      // ← 这里
    return overlay;

原版 `Minecraft.runTick()` 里靠 `overlay == null` 判断「现在不该处理按键」。
rrls 在加载界面还挂着的时候返回 `null`，把这条判断**反转**了：

  正常：加载界面挂着 -> overlay != null -> 跳过 handleKeybinds
  有 rrls：加载界面挂着 -> overlay == null -> **照样跑 handleKeybinds**
                                      -> this.player 还是 null -> NPE 崩溃

所以崩溃发生在**进世界之前**，而且和我们的 AI 无关
（第二次崩溃时守护进程根本没启动，一个按键都没注入过）。

## 为什么不能靠 rrls.toml 的 enabled = false

`config/rrls.toml` 里确实有 `enabled = true`，看着像开关。但那是
**AutoConfig 的配置项**，只影响模组自己的配置逻辑 ——
`fixOverlayUsage` 这个 `@Redirect` 方法体里**没有任何 enabled 判断**
（字节码里只有一条 `getfield f_91081_` -> `tryGetOverlay` -> `areturn`），
mixin 该注入还是注入，字段该劫持还是劫持。

**唯一可靠的处置是让这个 jar 不被 Forge 加载。**

## 本脚本做什么

Forge 只加载 `mods/` 下以 `.jar` 结尾的文件。把目标 jar 改名成
`<原名>.jar.disabled`，Forge 就看不见它了 —— 可随时改回来。

用法：
    python scripts/toggle_rrls.py            # 查看当前状态
    python scripts/toggle_rrls.py disable    # 禁用（防崩溃）
    python scripts/toggle_rrls.py enable     # 恢复

**建议先备份**：脚本会把原 jar 复制一份到 `mods/.trit-agent-backup/`，
再改名。不是删除，所以随时能还原。
"""

import os
import shutil
import sys
import zipfile

# 整合包实例目录（MC 启动器 profile）
PROFILE = r"E:\MC\MC\profiles\烦人的村民"
MODS = os.path.join(PROFILE, "mods")
BACKUP = os.path.join(MODS, ".trit-agent-backup")

# 目标 jar —— 里面**只有** rrls（com.github.dimadencep.*），
# 禁用它不会连带别的功能
TARGET = "TheAnnoyingVillagersByPugilist_Steve_4_28 (34).jar"

# 判定「这个 jar 含 rrls」的依据：里面有 rrls 的 mixin 配置
RRLS_MARKER = "rrls-common.mixins.json"


def find_rrls_jars():
    """扫 mods/，找出所有含 rrls 的 jar。

    不写死文件名 —— 整合包更新后编号会变（(34) 可能变成 (35)），
    写死就失效了。

    **必须连 `.jar.disabled` 一起扫**：禁用之后文件名不再是 `.jar` 结尾，
    只扫 `.jar` 的话，禁用之后就「找不到」了 —— 连恢复功能都失效
    （这正是第一版脚本的 bug：disable 成功后再 enable 报「没找到」）。
    """
    if not os.path.isdir(MODS):
        return []
    hits = []
    for name in sorted(os.listdir(MODS)):
        # `.jar` 和 `.jar.disabled` 都算
        if not (name.lower().endswith(".jar")
                or name.lower().endswith(".jar.disabled")):
            continue
        path = os.path.join(MODS, name)
        try:
            with zipfile.ZipFile(path) as z:
                names = z.namelist()
        except (zipfile.BadZipFile, OSError):
            continue
        if RRLS_MARKER in names:
            hits.append(name)
    return hits


def state_of(name):
    """返回 (状态, 实际路径)。

    状态取值：active（加载中，有害）/ disabled（已禁用，安全）
    """
    # name 可能本来就带 `.disabled` 后缀（find_rrls_jars 扫到了禁用态的）
    base = name[:-len(".disabled")] if name.lower().endswith(".disabled") else name
    active = os.path.join(MODS, base)
    off = os.path.join(MODS, base + ".disabled")
    if os.path.isfile(active):
        return "active", active
    if os.path.isfile(off):
        return "disabled", off
    return "missing", None


def show():
    hits = find_rrls_jars()
    if not hits:
        print("没在 mods/ 里找到含 rrls 的 jar。")
        print("（整合包更新过的话，rrls 可能已经被上游修好了 —— "
              "那就更不用管。）")
        return 0

    print("含 rrls 的 jar（这些是崩溃真凶）：")
    for name in hits:
        st, path = state_of(name)
        mark = "有害(会加载)" if st == "active" else "已禁用"
        print("  [{}] {}".format(mark, name))
    print()
    print("禁用:  python scripts/toggle_rrls.py disable")
    print("恢复:  python scripts/toggle_rrls.py enable")
    return 0


def disable():
    hits = find_rrls_jars()
    if not hits:
        print("没找到含 rrls 的 jar，无需处理。")
        return 0

    os.makedirs(BACKUP, exist_ok=True)
    changed = 0
    for name in hits:
        st, path = state_of(name)
        if st == "disabled":
            print("已经是禁用状态: {}".format(os.path.basename(path)))
            continue
        if st != "active":
            continue

        base = os.path.basename(path)
        # 先备份（不是删除，可随时还原）
        bak = os.path.join(BACKUP, base)
        if not os.path.isfile(bak):
            shutil.copy2(path, bak)
            print("已备份 -> {}".format(bak))

        # 改名：Forge 只加载 .jar 结尾的文件
        os.rename(path, path + ".disabled")
        print("已禁用: {}".format(base))
        changed += 1

    if changed:
        print()
        print("搞定。下次启动游戏就不会再崩在 handleKeybinds 了。")
        print("代价：加载/资源重载时会短暂显示原版的进度条界面（正常现象）。")
    return 0


def enable():
    hits = find_rrls_jars()
    if not hits:
        print("没找到含 rrls 的 jar，无需处理。")
        return 0

    changed = 0
    for name in hits:
        st, path = state_of(name)
        if st == "active":
            print("本来就是启用的: {}".format(os.path.basename(path)))
            continue
        if st != "disabled":
            continue
        os.rename(path, os.path.join(MODS, os.path.basename(path)[:-len(".disabled")]))
        print("已恢复: {}".format(os.path.basename(path)))
        changed += 1

    if changed:
        print()
        print("注意：rrls 又回来了 —— 崩溃可能重现。")
        print("如果还崩，就再 disable 一次。")
    return 0


def main():
    action = sys.argv[1].lower() if len(sys.argv) > 1 else "status"
    if action in ("status", "show", ""):
        return show()
    if action == "disable":
        return disable()
    if action == "enable":
        return enable()
    print("未知参数: {}".format(action))
    print("可用: status / disable / enable")
    return 1


if __name__ == "__main__":
    sys.exit(main())
