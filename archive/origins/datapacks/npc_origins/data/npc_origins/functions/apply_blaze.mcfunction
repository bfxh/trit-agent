#> 用法：站在 NPC 旁边执行 /function npc_origins:apply_blaze
#> 给 8 格内的所有非玩家实体打上"烈焰人"标记
tag @e[distance=..8,type=!player,type=!item] add npc_blaze
tellraw @s {"text":"[NPC起源] 已为附近 NPC 赋予：烈焰人（抗火+力量）","color":"gold"}
