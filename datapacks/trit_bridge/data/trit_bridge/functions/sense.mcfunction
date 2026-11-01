# 传感器：每 20 tick 由 pulse 调用一次，读一帧状态写进聊天
scoreboard players set hst trit_bridge 0
# 距离档：从远到近依次覆盖，最终留下最近那一档（0 = 视野内没有）
scoreboard players set dst trit_bridge 0
scoreboard players set xst trit_bridge 0
execute store result score hp trit_bridge run data get entity @s Health 10
execute store result score food trit_bridge run data get entity @s foodLevel 1
execute store result score px trit_bridge run data get entity @s Pos[0] 10
execute store result score py trit_bridge run data get entity @s Pos[1] 10
execute store result score pz trit_bridge run data get entity @s Pos[2] 10
execute as @e[type=#trit_bridge:hostiles,limit=32,distance=..32] run scoreboard players add hst trit_bridge 1
execute if entity @e[type=#trit_bridge:hostiles,limit=1,distance=..32] run scoreboard players set dst trit_bridge 32
execute if entity @e[type=#trit_bridge:hostiles,limit=1,distance=..16] run scoreboard players set dst trit_bridge 16
execute if entity @e[type=#trit_bridge:hostiles,limit=1,distance=..10] run scoreboard players set dst trit_bridge 10
execute if entity @e[type=#trit_bridge:hostiles,limit=1,distance=..6] run scoreboard players set dst trit_bridge 6
execute if entity @e[type=#trit_bridge:hostiles,limit=1,distance=..3] run scoreboard players set dst trit_bridge 3
execute if entity @e[type=#trit_bridge:explosives,limit=1,distance=..32] run scoreboard players set xst trit_bridge 32
execute if entity @e[type=#trit_bridge:explosives,limit=1,distance=..16] run scoreboard players set xst trit_bridge 16
execute if entity @e[type=#trit_bridge:explosives,limit=1,distance=..10] run scoreboard players set xst trit_bridge 10
execute if entity @e[type=#trit_bridge:explosives,limit=1,distance=..6] run scoreboard players set xst trit_bridge 6
execute if entity @e[type=#trit_bridge:explosives,limit=1,distance=..3] run scoreboard players set xst trit_bridge 3
# 原木：8 方向 × 3 档，由远到近、正前最后，留下最近那一格
scoreboard players set wdt trit_bridge 0
scoreboard players set wdr trit_bridge 0
execute store result score tod trit_bridge run time query daytime
execute store result score pit trit_bridge run data get entity @s Rotation[1] 10
execute at @s rotated ~-135 0 if block ^ ^ ^12 #minecraft:logs run scoreboard players set wdt trit_bridge 12
execute at @s rotated ~-135 0 if block ^ ^ ^12 #minecraft:logs run scoreboard players set wdr trit_bridge -3
execute at @s rotated ~-90 0 if block ^ ^ ^12 #minecraft:logs run scoreboard players set wdt trit_bridge 12
execute at @s rotated ~-90 0 if block ^ ^ ^12 #minecraft:logs run scoreboard players set wdr trit_bridge -2
execute at @s rotated ~-45 0 if block ^ ^ ^12 #minecraft:logs run scoreboard players set wdt trit_bridge 12
execute at @s rotated ~-45 0 if block ^ ^ ^12 #minecraft:logs run scoreboard players set wdr trit_bridge -1
execute at @s rotated ~45 0 if block ^ ^ ^12 #minecraft:logs run scoreboard players set wdt trit_bridge 12
execute at @s rotated ~45 0 if block ^ ^ ^12 #minecraft:logs run scoreboard players set wdr trit_bridge 1
execute at @s rotated ~90 0 if block ^ ^ ^12 #minecraft:logs run scoreboard players set wdt trit_bridge 12
execute at @s rotated ~90 0 if block ^ ^ ^12 #minecraft:logs run scoreboard players set wdr trit_bridge 2
execute at @s rotated ~135 0 if block ^ ^ ^12 #minecraft:logs run scoreboard players set wdt trit_bridge 12
execute at @s rotated ~135 0 if block ^ ^ ^12 #minecraft:logs run scoreboard players set wdr trit_bridge 3
execute at @s rotated ~180 0 if block ^ ^ ^12 #minecraft:logs run scoreboard players set wdt trit_bridge 12
execute at @s rotated ~180 0 if block ^ ^ ^12 #minecraft:logs run scoreboard players set wdr trit_bridge 4
execute at @s rotated ~0 0 if block ^ ^ ^12 #minecraft:logs run scoreboard players set wdt trit_bridge 12
execute at @s rotated ~0 0 if block ^ ^ ^12 #minecraft:logs run scoreboard players set wdr trit_bridge 0
execute at @s rotated ~-135 0 if block ^ ^ ^6 #minecraft:logs run scoreboard players set wdt trit_bridge 6
execute at @s rotated ~-135 0 if block ^ ^ ^6 #minecraft:logs run scoreboard players set wdr trit_bridge -3
execute at @s rotated ~-90 0 if block ^ ^ ^6 #minecraft:logs run scoreboard players set wdt trit_bridge 6
execute at @s rotated ~-90 0 if block ^ ^ ^6 #minecraft:logs run scoreboard players set wdr trit_bridge -2
execute at @s rotated ~-45 0 if block ^ ^ ^6 #minecraft:logs run scoreboard players set wdt trit_bridge 6
execute at @s rotated ~-45 0 if block ^ ^ ^6 #minecraft:logs run scoreboard players set wdr trit_bridge -1
execute at @s rotated ~45 0 if block ^ ^ ^6 #minecraft:logs run scoreboard players set wdt trit_bridge 6
execute at @s rotated ~45 0 if block ^ ^ ^6 #minecraft:logs run scoreboard players set wdr trit_bridge 1
execute at @s rotated ~90 0 if block ^ ^ ^6 #minecraft:logs run scoreboard players set wdt trit_bridge 6
execute at @s rotated ~90 0 if block ^ ^ ^6 #minecraft:logs run scoreboard players set wdr trit_bridge 2
execute at @s rotated ~135 0 if block ^ ^ ^6 #minecraft:logs run scoreboard players set wdt trit_bridge 6
execute at @s rotated ~135 0 if block ^ ^ ^6 #minecraft:logs run scoreboard players set wdr trit_bridge 3
execute at @s rotated ~180 0 if block ^ ^ ^6 #minecraft:logs run scoreboard players set wdt trit_bridge 6
execute at @s rotated ~180 0 if block ^ ^ ^6 #minecraft:logs run scoreboard players set wdr trit_bridge 4
execute at @s rotated ~0 0 if block ^ ^ ^6 #minecraft:logs run scoreboard players set wdt trit_bridge 6
execute at @s rotated ~0 0 if block ^ ^ ^6 #minecraft:logs run scoreboard players set wdr trit_bridge 0
execute at @s rotated ~-135 -45 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdt trit_bridge 3
execute at @s rotated ~-135 -45 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdr trit_bridge -3
execute at @s rotated ~-90 -45 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdt trit_bridge 3
execute at @s rotated ~-90 -45 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdr trit_bridge -2
execute at @s rotated ~-45 -45 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdt trit_bridge 3
execute at @s rotated ~-45 -45 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdr trit_bridge -1
execute at @s rotated ~45 -45 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdt trit_bridge 3
execute at @s rotated ~45 -45 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdr trit_bridge 1
execute at @s rotated ~90 -45 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdt trit_bridge 3
execute at @s rotated ~90 -45 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdr trit_bridge 2
execute at @s rotated ~135 -45 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdt trit_bridge 3
execute at @s rotated ~135 -45 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdr trit_bridge 3
execute at @s rotated ~180 -45 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdt trit_bridge 3
execute at @s rotated ~180 -45 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdr trit_bridge 4
execute at @s rotated ~0 -45 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdt trit_bridge 3
execute at @s rotated ~0 -45 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdr trit_bridge 0
execute at @s rotated ~-135 0 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdt trit_bridge 3
execute at @s rotated ~-135 0 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdr trit_bridge -3
execute at @s rotated ~-90 0 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdt trit_bridge 3
execute at @s rotated ~-90 0 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdr trit_bridge -2
execute at @s rotated ~-45 0 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdt trit_bridge 3
execute at @s rotated ~-45 0 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdr trit_bridge -1
execute at @s rotated ~45 0 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdt trit_bridge 3
execute at @s rotated ~45 0 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdr trit_bridge 1
execute at @s rotated ~90 0 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdt trit_bridge 3
execute at @s rotated ~90 0 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdr trit_bridge 2
execute at @s rotated ~135 0 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdt trit_bridge 3
execute at @s rotated ~135 0 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdr trit_bridge 3
execute at @s rotated ~180 0 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdt trit_bridge 3
execute at @s rotated ~180 0 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdr trit_bridge 4
execute at @s rotated ~0 0 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdt trit_bridge 3
execute at @s rotated ~0 0 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdr trit_bridge 0
execute at @s rotated ~-135 30 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdt trit_bridge 3
execute at @s rotated ~-135 30 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdr trit_bridge -3
execute at @s rotated ~-90 30 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdt trit_bridge 3
execute at @s rotated ~-90 30 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdr trit_bridge -2
execute at @s rotated ~-45 30 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdt trit_bridge 3
execute at @s rotated ~-45 30 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdr trit_bridge -1
execute at @s rotated ~45 30 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdt trit_bridge 3
execute at @s rotated ~45 30 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdr trit_bridge 1
execute at @s rotated ~90 30 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdt trit_bridge 3
execute at @s rotated ~90 30 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdr trit_bridge 2
execute at @s rotated ~135 30 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdt trit_bridge 3
execute at @s rotated ~135 30 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdr trit_bridge 3
execute at @s rotated ~180 30 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdt trit_bridge 3
execute at @s rotated ~180 30 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdr trit_bridge 4
execute at @s rotated ~0 30 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdt trit_bridge 3
execute at @s rotated ~0 30 if block ^ ^ ^3 #minecraft:logs run scoreboard players set wdr trit_bridge 0
tellraw @s [{"text":"<TRIT hp="},{"score":{"name":"hp","objective":"trit_bridge"}},{"text":" food="},{"score":{"name":"food","objective":"trit_bridge"}},{"text":" x="},{"score":{"name":"px","objective":"trit_bridge"}},{"text":" y="},{"score":{"name":"py","objective":"trit_bridge"}},{"text":" z="},{"score":{"name":"pz","objective":"trit_bridge"}},{"text":" h="},{"score":{"name":"hst","objective":"trit_bridge"}},{"text":" d="},{"score":{"name":"dst","objective":"trit_bridge"}},{"text":" c="},{"score":{"name":"xst","objective":"trit_bridge"}},{"text":" w="},{"score":{"name":"wdt","objective":"trit_bridge"}},{"text":" wd="},{"score":{"name":"wdr","objective":"trit_bridge"}},{"text":" t="},{"score":{"name":"tod","objective":"trit_bridge"}},{"text":" p="},{"score":{"name":"pit","objective":"trit_bridge"}},{"text":">"}]
