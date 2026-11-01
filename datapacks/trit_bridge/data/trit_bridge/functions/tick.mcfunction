# 计时器，每 20 tick（1 秒）触发一次 pulse
scoreboard players add t trit_bridge 1
execute if score t trit_bridge matches 20.. run function trit_bridge:pulse
execute if score t trit_bridge matches 20.. run scoreboard players set t trit_bridge 0
