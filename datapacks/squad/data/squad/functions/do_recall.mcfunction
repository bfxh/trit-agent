# 召回：先停手，再拉回身边（6 格内不再拉）。
#
# 顺序重要：必须先把仇恨清掉再 tp。反过来的话成员会一边被 tp 回来
# 一边继续追出去 —— 因为它还记着仇，而且 AngerTime 还有几十秒。
#
# 清仇恨必须 AngryAt + AngerTime 一起清，理由同 do_hold：Angerable 的愤怒
# 状态只认 AngerTime，只删 AngryAt 会留下一个「没有目标但仍在愤怒」的狼。
execute as @e[tag=trit_hound] run data merge entity @s {AngerTime:0}
execute as @e[tag=trit_golem] run data merge entity @s {AngerTime:0}
execute as @e[tag=trit_hound] if data entity @s AngryAt run data remove entity @s AngryAt
execute as @e[tag=trit_golem] if data entity @s AngryAt run data remove entity @s AngryAt
execute as @e[tag=trit_hound] at @s unless entity @p[distance=..6] run tp @s @p
execute as @e[tag=trit_golem] at @s unless entity @p[distance=..6] run tp @s @p
