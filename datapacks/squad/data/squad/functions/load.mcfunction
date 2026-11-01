# 记分板：squad_mode 是「共同意识」的意图寄存器
# 0 = 待命  1 = 交战  2 = 集合  3 = 召回
#
# 这些 objective 名重复 add 只会输出一行红字，不影响加载，
# 所以不用做存在性检查（1.18 也没有可靠的判断手段）。
scoreboard objectives add squad_mode dummy
scoreboard objectives add squad_timer dummy
scoreboard players set #mode squad_mode 0
scoreboard players set #timer squad_timer 0

tellraw @a {"text":"[squad] 共同意识已加载。","color":"green"}
tellraw @a {"text":"  /function squad:recruit  收编身边的傀儡和狼（16 格内）","color":"gray"}
tellraw @a {"text":"  /function squad:hold|engage|regroup|recall  手动下令","color":"gray"}
