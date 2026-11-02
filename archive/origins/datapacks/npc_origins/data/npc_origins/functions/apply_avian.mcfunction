#> 用法：/function npc_origins:apply_avian
#> 给 8 格内的所有非玩家实体打上"疾风"标记（速度 II + 跳跃 II + 缓降）
tag @e[distance=..8,type=!player,type=!item] add npc_avian
tellraw @s {"text":"[NPC起源] 已为附近 NPC 赋予：疾风（速度+跳跃+缓降）","color":"gold"}
