# 快通道：每 2 tick（100ms）由 pulse_fast 调用。
# 只报瞄准闭环与碰撞判定要用的量，命令数压到 24 条以内。

# 掩码清零：8 个方位各占一位
scoreboard players set wal trit_bridge 0
# 正前方是不是「一格高的台阶」（前面有固体、它上面没有）
scoreboard players set stp trit_bridge 0
# 俯仰角 ×10（-900..900）。快通道报它，是为了让瞄准闭环的等待
# 从「等 1 秒」变成「等 100 毫秒」——三值只给方向，没有角度就是开环瞎转。
execute store result score pit trit_bridge run data get entity @s Rotation[1] 10

# 八个方位 1 格内是否有固体：撞墙 / 绕行判定
execute at @s rotated ~0 0 if block ^ ^ ^1 #trit_bridge:solid run scoreboard players add wal trit_bridge 1
execute at @s rotated ~45 0 if block ^ ^ ^1 #trit_bridge:solid run scoreboard players add wal trit_bridge 2
execute at @s rotated ~90 0 if block ^ ^ ^1 #trit_bridge:solid run scoreboard players add wal trit_bridge 4
execute at @s rotated ~135 0 if block ^ ^ ^1 #trit_bridge:solid run scoreboard players add wal trit_bridge 8
execute at @s rotated ~180 0 if block ^ ^ ^1 #trit_bridge:solid run scoreboard players add wal trit_bridge 16
execute at @s rotated ~-135 0 if block ^ ^ ^1 #trit_bridge:solid run scoreboard players add wal trit_bridge 32
execute at @s rotated ~-90 0 if block ^ ^ ^1 #trit_bridge:solid run scoreboard players add wal trit_bridge 64
execute at @s rotated ~-45 0 if block ^ ^ ^1 #trit_bridge:solid run scoreboard players add wal trit_bridge 128

# 正前方是台阶还是墙：一格高能跳过去，两格高只能绕。
# 少了这一条，AI 会把每一个土坎都当成死路 —— 表现出来就是「明明能走的地方它站住不动」。
execute at @s rotated ~0 0 positioned ^ ^ ^1 if block ~ ~ ~ #trit_bridge:solid unless block ~ ~1 ~ #trit_bridge:solid run scoreboard players set stp trit_bridge 1

# 准星正对的方块（沿视线，含俯仰）：挖矿的目标就是它
# 判定顺序 = 低优先级先 set、高优先级后 set，后写的赢
scoreboard players set b1 trit_bridge 0
execute at @s if block ^ ^ ^1 #trit_bridge:solid run scoreboard players set b1 trit_bridge 3
execute at @s if block ^ ^ ^1 #minecraft:logs run scoreboard players set b1 trit_bridge 1
execute at @s if block ^ ^ ^1 #trit_bridge:ores run scoreboard players set b1 trit_bridge 2
execute at @s if block ^ ^ ^1 #trit_bridge:water run scoreboard players set b1 trit_bridge 4
execute at @s if block ^ ^ ^1 #trit_bridge:lava run scoreboard players set b1 trit_bridge 5
execute at @s if block ^ ^ ^1 #trit_bridge:hazard run scoreboard players set b1 trit_bridge 6

# 脚下那格：站在什么上面。挖矿最怕的就是挖塌自己站的地方
scoreboard players set bd trit_bridge 0
execute at @s if block ~ ~-1 ~ #trit_bridge:solid run scoreboard players set bd trit_bridge 3
execute at @s if block ~ ~-1 ~ #minecraft:logs run scoreboard players set bd trit_bridge 1
execute at @s if block ~ ~-1 ~ #trit_bridge:ores run scoreboard players set bd trit_bridge 2
execute at @s if block ~ ~-1 ~ #trit_bridge:water run scoreboard players set bd trit_bridge 4
execute at @s if block ~ ~-1 ~ #trit_bridge:lava run scoreboard players set bd trit_bridge 5
execute at @s if block ~ ~-1 ~ #trit_bridge:hazard run scoreboard players set bd trit_bridge 6

tellraw @s [{"text":"<TRITF wal="},{"score":{"name":"wal","objective":"trit_bridge"}},{"text":" b1="},{"score":{"name":"b1","objective":"trit_bridge"}},{"text":" bd="},{"score":{"name":"bd","objective":"trit_bridge"}},{"text":" s="},{"score":{"name":"stp","objective":"trit_bridge"}},{"text":" p="},{"score":{"name":"pit","objective":"trit_bridge"}},{"text":">"}]
