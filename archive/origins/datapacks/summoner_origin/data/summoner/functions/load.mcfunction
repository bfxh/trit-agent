# 初始化记分板（每次 /reload 或进世界执行，重复执行安全）
scoreboard objectives add summoner_timer dummy
scoreboard objectives add summoner_config dummy

# 默认开启「主动攻击村民/敌对NPC」。
# 用 #hunt_inited 记「已经给过默认值」，不是直接判断 #hunt_switch 是不是 0 ——
# 否则你手动 /scoreboard players set #hunt_switch summoner_config 0 关掉以后，
# 一次 /reload 就又给你打开了，看着像设置没生效。
execute unless score #hunt_inited summoner_config matches 1 run scoreboard players set #hunt_switch summoner_config 1
scoreboard players set #hunt_inited summoner_config 1
