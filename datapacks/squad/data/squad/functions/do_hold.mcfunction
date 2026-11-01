# 待命：停手，但不锁住它们的自然行为。
#
# `if data entity @s AngryAt` 这个前置检查不能省：直接 `data remove` 在
# 路径不存在时会失败并往执行者聊天框丢红字，每 8 tick 刷一次，几秒钟就
# 把聊天框糊满了。加上 if data 就只在真的愤怒时清一次。
execute as @e[tag=trit_hound] if data entity @s AngryAt run data remove entity @s AngryAt
execute as @e[tag=trit_golem] if data entity @s AngryAt run data remove entity @s AngryAt
