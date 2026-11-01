# 解散：先停手再摘 tag。
#
# 只摘 tag 不清 AngryAt 的话，它们会带着 30 秒的怒气继续追出去 ——
# 看起来就像「解散没生效」。顺序不能反。
execute as @e[tag=trit_hound] if data entity @s AngryAt run data remove entity @s AngryAt
execute as @e[tag=trit_golem] if data entity @s AngryAt run data remove entity @s AngryAt
tag @e[tag=trit_hound] remove trit_hound
tag @e[tag=trit_golem] remove trit_golem
tellraw @a {"text":"[squad] 已解散，成员回到原版 AI","color":"gray"}
