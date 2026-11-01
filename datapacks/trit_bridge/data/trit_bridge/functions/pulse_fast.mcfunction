# 快通道要两个开关同时开着才跑：总开关 on + 按需开关 fo
execute if score on trit_bridge matches 1 if score fo trit_bridge matches 1 as @a at @s run function trit_bridge:sense_fast
