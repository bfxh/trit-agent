# 收编：16 格内最近的 3 只狼 + 2 只铁傀儡打上 tag，从此听共同意识指挥。
#
# 这就是「我可以打开这个东西」的入口：打开 = 跑一次 recruit，
# 关掉 = 跑一次 dismiss。不需要装 mod，也不需要改世界设置（只要开了作弊）。
#
# 想收编别的生物（雪傀儡、猫、甚至村民）就往这里加一行，
# 条件是那个生物会读 AngryAt —— 原版只有 wolf / iron_golem / polar_bear 会读。
execute at @s run tag @e[type=minecraft:wolf,distance=..16,limit=3,sort=nearest] add trit_hound
execute at @s run tag @e[type=minecraft:iron_golem,distance=..16,limit=2,sort=nearest] add trit_golem

execute at @s run tellraw @a {"text":"[小队] 已收编：","color":"green"}
execute at @s run tellraw @a [{"selector":"@e[tag=trit_hound]"},{"text":" 只狼  +  "},{"selector":"@e[tag=trit_golem]"},{"text":" 只傀儡"}]
