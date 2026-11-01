# 首次加载：建记分板
scoreboard objectives add trit_bridge dummy

# 「总开关」默认打开。
#
# 默认值原来是 0，理由是「tellraw 刷聊天框」。但 AI 的感知**完全**依赖
# 这一条通道：开关一关，tellraw 一行都不出，日志里搜 TRIT 是 0 条，于是
# 墙的方位、敌人距离、脚下是什么全是 None —— 按三值原则「未知不猜」，
# AI 就站桩不动。这正是「AI 看不见墙壁」的来源。
# 刷屏的代价（每秒一行）远小于瞎掉的代价，所以默认改成开。
#
# 注意：必须用 #inited 记「已经给过默认值」，不能每次都 `set on 1` ——
# 否则你手动关掉以后，一次 /reload 就又给打开了，看着像设置没生效。
# 和 summoner 包里 #hunt_switch 踩的是同一个坑。
execute unless score #inited trit_bridge matches 1 run scoreboard players set on trit_bridge 1
scoreboard players set #inited trit_bridge 1

# fo = fast on。快通道保持默认关：它每 2 tick 刷一行，只在采集这种需要
# 100ms 闭环的场合由 Rust 侧临时打开，用完即关。
execute unless score #inited trit_bridge matches 1 run scoreboard players set fo trit_bridge 0

# 以下都是每 tick 重算的中间量，重复清零无害
scoreboard players set tf trit_bridge 0
scoreboard players set t trit_bridge 0
scoreboard players set wal trit_bridge 0
scoreboard players set b1 trit_bridge 0
scoreboard players set bd trit_bridge 0
scoreboard players set wdt trit_bridge 0
scoreboard players set wdr trit_bridge 0
scoreboard players set odt trit_bridge 0
scoreboard players set odr trit_bridge 0
scoreboard players set tod trit_bridge 0
scoreboard players set pit trit_bridge 0
