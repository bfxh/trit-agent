# 待命：停手，但不锁住它们的自然行为。
#
# 两个字段都要清，只清一个等于没清：
#   AngryAt  = 记恨谁（UUID）
#   AngerTime= 还要恨多久（tick）
# 原版 Angerable 的 `hasAngerTime()` 只看 AngerTime 是否 > 0，挂在上面的
# 移动速度加成（ATTACKING_SPEED_BOOST）也只看这个。所以只删 AngryAt 的
# 话，狼不会再去追人，但仍会顶着「愤怒」状态跑得飞快，直到 30 秒后
# AngerTime 自然耗尽 —— 表现就是「待命按了，它还在那儿乱窜」。
#
# 顺序是 merge 在前：data merge 无条件可写，AngerTime 不存在时会创建；
# 而 data remove 路径不存在时会失败并往执行者聊天框丢红字，每 8 tick
# 刷一次，几秒钟就把聊天框糊满了，所以必须先用 `if data` 挡一道。
execute as @e[tag=trit_hound] run data merge entity @s {AngerTime:0}
execute as @e[tag=trit_golem] run data merge entity @s {AngerTime:0}
execute as @e[tag=trit_hound] if data entity @s AngryAt run data remove entity @s AngryAt
execute as @e[tag=trit_golem] if data entity @s AngryAt run data remove entity @s AngryAt
