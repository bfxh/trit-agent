# ---- 战狼：把 14 格内最近的「村民/敌对NPC」设成愤怒目标
execute as @e[type=minecraft:wolf,tag=summon_wolf] at @s if entity @e[type=#summoner:hunt_targets,distance=..14,limit=1] run data modify entity @s AngryAt set from entity @e[type=#summoner:hunt_targets,distance=..14,limit=1,sort=nearest] UUID
execute as @e[type=minecraft:wolf,tag=summon_wolf] at @s if entity @e[type=#summoner:hunt_targets,distance=..14,limit=1] run data merge entity @s {AngerTime:400}

# ---- 铁卫：同样处理
execute as @e[type=minecraft:iron_golem,tag=summon_guard] at @s if entity @e[type=#summoner:hunt_targets,distance=..14,limit=1] run data modify entity @s AngryAt set from entity @e[type=#summoner:hunt_targets,distance=..14,limit=1,sort=nearest] UUID
execute as @e[type=minecraft:iron_golem,tag=summon_guard] at @s if entity @e[type=#summoner:hunt_targets,distance=..14,limit=1] run data merge entity @s {AngerTime:400}
