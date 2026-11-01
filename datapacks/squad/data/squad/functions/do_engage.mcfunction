# 交战：把 16 格内最近的敌对目标写进成员的 AngryAt。
#
# 这是「操控宠物」在 1.18.2 里的**物理上限**：wolf / iron_golem 的 Goal 会读
# AngryAt + AngerTime，这是唯一能真正改变它们行为的两个 NBT 字段。
# 中间怎么走、打哪一下管不了 —— 那是原版 AI 的事，没有注入点。
#
# AngerTime 600 = 30 秒，每 0.4 秒续一次期，所以只要意图还在就不会掉。
#
# 先 data merge 造出 AngryAt 再 data modify 填值：data modify 只能写已存在的
# 路径，而狼/傀儡刚生成时根本没有 AngryAt。写成 modify 在前的话，每次都被
# 静默拒绝 —— 不报错、不写日志，看起来就是「AI 叫它们上，它们不动」。
execute as @e[tag=trit_hound] at @s if entity @e[type=#squad:hostiles,distance=..16,limit=1] run data merge entity @s {AngryAt:[I;0,0,0,0],AngerTime:600}
execute as @e[tag=trit_hound] at @s if entity @e[type=#squad:hostiles,distance=..16,limit=1] run data modify entity @s AngryAt set from entity @e[type=#squad:hostiles,distance=..16,limit=1,sort=nearest] UUID

execute as @e[tag=trit_golem] at @s if entity @e[type=#squad:hostiles,distance=..16,limit=1] run data merge entity @s {AngryAt:[I;0,0,0,0],AngerTime:600}
execute as @e[tag=trit_golem] at @s if entity @e[type=#squad:hostiles,distance=..16,limit=1] run data modify entity @s AngryAt set from entity @e[type=#squad:hostiles,distance=..16,limit=1,sort=nearest] UUID
