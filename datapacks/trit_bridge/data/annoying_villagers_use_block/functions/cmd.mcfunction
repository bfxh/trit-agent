# 空函数：只为填上整合包里一个指向不存在函数的引用。
#
# TheAnnoyingVillagersByPugilist_Steve_4_28 (205).jar 的
# data/minecraft/tags/functions/tick.json 引用了本函数，但那个 jar 里
# 并没有实现它。MC 的标签加载是 all-or-nothing，坏一条 = 整个
# minecraft:tick 标签作废 = 我们的传感器 tick 也不跑 = AI 瞎。
# 本函数存在的唯一意义就是让那个引用能解析。
