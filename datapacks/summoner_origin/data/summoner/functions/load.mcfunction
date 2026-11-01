# 初始化记分板（每次 /reload 或进世界执行，重复执行安全）
scoreboard objectives add summoner_timer dummy
scoreboard objectives add summoner_config dummy

# 默认开启「主动攻击村民/敌对NPC」
execute unless score #hunt_switch summoner_config matches 0 run scoreboard players set #hunt_switch summoner_config 1
