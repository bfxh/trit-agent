#> 用法：/function npc_origins:apply_tank
#> 给 8 格内的所有非玩家实体打上"巨人"标记（抗性 II + 生命恢复）
tag @e[distance=..8,type=!player,type=!item] add npc_tank
tellraw @s {"text":"[NPC起源] 已为附近 NPC 赋予：巨人（抗性+回复）","color":"gold"}
