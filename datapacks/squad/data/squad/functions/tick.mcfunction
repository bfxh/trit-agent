# 每 tick 进一次，但真正干活是每 8 tick（0.4 秒）一次。
#
# 为什么不每 tick 都跑：do_engage 每条命令都要做一次半径 16 的实体扫描，
# 成员 5 只就是 10 次扫描/tick = 200 次/秒。0.4 秒的指令延迟完全够用，
# 而卡顿是立刻能感觉到的。
scoreboard players add #timer squad_timer 1
execute if score #timer squad_timer matches 8.. run function squad:step
