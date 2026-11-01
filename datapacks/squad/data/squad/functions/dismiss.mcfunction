# 解散：先停手再摘 tag。
#
# 只摘 tag 不清仇恨的话，它们会带着 30 秒的怒气继续追出去 ——
# 看起来就像「解散没生效」。顺序不能反。
#
# 仇恨 = AngryAt（恨谁）+ AngerTime（恨多久），两个都得清。
# AngerTime 用 data merge 写 0，因为 merge 在字段不存在时也能建；
# AngryAt 用 `if data` 挡一下再 remove，否则路径不存在时每 8 tick
# 往聊天框刷一行红字。
execute as @e[tag=trit_hound] run data merge entity @s {AngerTime:0}
execute as @e[tag=trit_golem] run data merge entity @s {AngerTime:0}
execute as @e[tag=trit_hound] if data entity @s AngryAt run data remove entity @s AngryAt
execute as @e[tag=trit_golem] if data entity @s AngryAt run data remove entity @s AngryAt
tag @e[tag=trit_hound] remove trit_hound
tag @e[tag=trit_golem] remove trit_golem
tellraw @a {"text":"[小队] 已解散，成员回到原版 AI","color":"gray"}
