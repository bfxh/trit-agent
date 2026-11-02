# 停止自动攻击：把战狼/铁卫身上残留的仇恨清干净。
#
# 对应 hunt_apply 写进去的两个字段：
#   AngryAt   —— 恨谁（UUID）
#   AngerTime —— 还要恨多久（tick）
# 两个都要清，少清一个都不算停手：
#   · 只删 AngryAt  -> 目标没了，但 AngerTime 还是 400，狼顶着「愤怒」状态
#                      跑满 20 秒（原版 Angerable.hasAngerTime() 只看这个）
#   · 只清 AngerTime -> 计时归零但目标还在，下一次续期（或原版 AI 一问）
#                      就又想起来，仇恨赖着不走
#
# 写法上的两个坑（本仓库踩过三次，都是静默失败）：
#   · data merge 能建新字段，所以 AngerTime 直接写 0 就行；
#   · data remove 只能删**已存在**的路径，路径不在会失败并往聊天框刷红字，
#     这里是每 5 tick 跑一次，不挡一下几秒就糊满屏，所以前面加 `if data`。
execute as @e[type=minecraft:wolf,tag=summon_wolf] run data merge entity @s {AngerTime:0}
execute as @e[type=minecraft:iron_golem,tag=summon_guard] run data merge entity @s {AngerTime:0}
execute as @e[type=minecraft:wolf,tag=summon_wolf] if data entity @s AngryAt run data remove entity @s AngryAt
execute as @e[type=minecraft:iron_golem,tag=summon_guard] if data entity @s AngryAt run data remove entity @s AngryAt
