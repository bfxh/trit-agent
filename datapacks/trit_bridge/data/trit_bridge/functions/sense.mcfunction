# 传感器：每 20 tick 由 pulse 调用一次，读一帧状态写进聊天
scoreboard players set hst trit_bridge 0
execute store result score hp trit_bridge run data get entity @s Health 10
execute store result score food trit_bridge run data get entity @s foodLevel 1
execute store result score px trit_bridge run data get entity @s Pos[0] 10
execute store result score py trit_bridge run data get entity @s Pos[1] 10
execute store result score pz trit_bridge run data get entity @s Pos[2] 10
execute as @e[type=#trit_bridge:hostiles,limit=24,distance=..16] run scoreboard players add hst trit_bridge 1
tellraw @s [{"text":"<TRIT hp="},{"score":{"name":"hp","objective":"trit_bridge"}},{"text":" food="},{"score":{"name":"food","objective":"trit_bridge"}},{"text":" x="},{"score":{"name":"px","objective":"trit_bridge"}},{"text":" y="},{"score":{"name":"py","objective":"trit_bridge"}},{"text":" z="},{"score":{"name":"pz","objective":"trit_bridge"}},{"text":" h="},{"score":{"name":"hst","objective":"trit_bridge"}},{"text":">"}]
