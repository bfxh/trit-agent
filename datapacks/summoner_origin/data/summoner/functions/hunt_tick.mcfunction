# 5 tick 跑一次（每秒 4 次），避免每 tick 全图扫描
scoreboard players add #hunt_timer summoner_timer 1
execute if score #hunt_timer summoner_timer matches 5.. run function summoner:hunt_run
