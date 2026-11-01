# 入口：设置意图寄存器并立刻执行一次（tick 里每 0.4 秒会继续执行同一个意图）
scoreboard players set #mode squad_mode 2
function squad:do_regroup
tellraw @a {"text":"[squad] 意图 → 集合","color":"yellow"}
