#> 用法：/function npc_origins:clear_all
#> 清除所有实体身上的 NPC 起源标记（立刻失效，下次 tick 不再刷新效果）
tag @e remove npc_blaze
tag @e remove npc_avian
tag @e remove npc_tank
tag @e remove npc_ender
tellraw @s {"text":"[NPC起源] 已清除全部 NPC 起源标记","color":"yellow"}
