# 集合：离玩家 12 格以上才拉过来。
#
# 阈值不能省。无条件 tp 会让成员每 0.4 秒往你身上贴一次，
# 表现是「宠物在你脚边疯狂抖动」—— 因为 tp 落地后又被挤开，再被拉回。
execute as @e[tag=trit_hound] at @s unless entity @p[distance=..12] run tp @s @p
execute as @e[tag=trit_golem] at @s unless entity @p[distance=..12] run tp @s @p
