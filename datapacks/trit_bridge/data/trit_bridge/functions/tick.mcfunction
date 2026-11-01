# 快通道：每 2 tick（100ms）
scoreboard players add tf trit_bridge 1
execute if score tf trit_bridge matches 2.. run function trit_bridge:pulse_fast
execute if score tf trit_bridge matches 2.. run scoreboard players set tf trit_bridge 0

# 慢通道：每 20 tick（1000ms）
scoreboard players add t trit_bridge 1
execute if score t trit_bridge matches 20.. run function trit_bridge:pulse
execute if score t trit_bridge matches 20.. run scoreboard players set t trit_bridge 0
