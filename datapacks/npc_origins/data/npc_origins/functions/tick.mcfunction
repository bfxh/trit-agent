#> npc_origins:tick —— 每 tick 刷新被标记 NPC 的"起源"能力
#> 时长 4 秒 + ambient=true，隐藏药水粒子，避免画面糊

# 烈焰人：抗火 + 力量
execute as @e[tag=npc_blaze] run effect give @s minecraft:fire_resistance 4 0 true
execute as @e[tag=npc_blaze] run effect give @s minecraft:strength 4 0 true

# 疾风：速度 II + 跳跃 II + 缓降（免摔）
execute as @e[tag=npc_avian] run effect give @s minecraft:speed 4 1 true
execute as @e[tag=npc_avian] run effect give @s minecraft:jump_boost 4 1 true
execute as @e[tag=npc_avian] run effect give @s minecraft:slow_falling 4 0 true

# 巨人：抗性 II + 生命恢复
execute as @e[tag=npc_tank] run effect give @s minecraft:resistance 4 1 true
execute as @e[tag=npc_tank] run effect give @s minecraft:regeneration 4 0 true

# 末影：夜视 + 速度 I
execute as @e[tag=npc_ender] run effect give @s minecraft:night_vision 24 0 true
execute as @e[tag=npc_ender] run effect give @s minecraft:speed 4 0 true
