#> 用法：/function npc_origins:apply_ender
#> 给 8 格内的所有非玩家实体打上"末影"标记（夜视 + 速度 I）
tag @e[distance=..8,type=!player,type=!item] add npc_ender
tellraw @s {"text":"[NPC起源] 已为附近 NPC 赋予：末影（夜视+速度）","color":"gold"}
