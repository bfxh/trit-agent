# ---- 战狼：把 14 格内最近的「村民/敌对NPC」设成愤怒目标 ----
# 先用 data merge 把字段「造」出来，再 data modify 填值。
# 原因：data modify 只在字段已存在时才写得进去，狼/傀儡刚召唤出来时 NBT 里
# 根本没有 AngryAt，命令会静默失败 —— 表现就是「召出来不打人」，
# 而游戏不报错、聊天栏也不提示。这类静默失败在本项目里踩过太多次。
execute as @e[type=minecraft:wolf,tag=summon_wolf] at @s if entity @e[type=#summoner:hunt_targets,distance=..14,limit=1] run data merge entity @s {AngryAt:[I;0,0,0,0],AngerTime:400}
execute as @e[type=minecraft:wolf,tag=summon_wolf] at @s if entity @e[type=#summoner:hunt_targets,distance=..14,limit=1] run data modify entity @s AngryAt set from entity @e[type=#summoner:hunt_targets,distance=..14,limit=1,sort=nearest] UUID

# ---- 铁卫：同样处理 ----
execute as @e[type=minecraft:iron_golem,tag=summon_guard] at @s if entity @e[type=#summoner:hunt_targets,distance=..14,limit=1] run data merge entity @s {AngryAt:[I;0,0,0,0],AngerTime:400}
execute as @e[type=minecraft:iron_golem,tag=summon_guard] at @s if entity @e[type=#summoner:hunt_targets,distance=..14,limit=1] run data modify entity @s AngryAt set from entity @e[type=#summoner:hunt_targets,distance=..14,limit=1,sort=nearest] UUID

# ---- 跟不上的拉回来：主人跑远了就传送过去 ----
# 用「最近玩家」而不是 Owner 字段，Owner 万一没写进去也不影响跟随
execute as @e[type=minecraft:wolf,tag=summon_wolf] at @s unless entity @p[distance=..32] run tp @s @p
execute as @e[type=minecraft:iron_golem,tag=summon_guard] at @s unless entity @p[distance=..32] run tp @s @p
