# 开 = 持续刷新仇恨；关 = 主动把仇恨清干净。
#
# 关掉时那句 hunt_clear 不能省。hunt_apply 写进去的是 AngryAt + AngerTime:400，
# 也就是「恨这个人，恨 20 秒」。开关关成 0 只是停止**续期**，已经写进去的
# 那 400 tick 还在实体身上，狼会接着把当前这一口咬完 —— 表现就是
# 「我把自动攻击关了，它还在咬村民」。必须显式清零才停得下来。
scoreboard players set #hunt_timer summoner_timer 0
execute if score #hunt_switch summoner_config matches 1 run function summoner:hunt_apply
execute unless score #hunt_switch summoner_config matches 1 run function summoner:hunt_clear
