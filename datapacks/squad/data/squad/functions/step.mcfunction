scoreboard players set #timer squad_timer 0
execute if score #mode squad_mode matches 0 run function squad:do_hold
execute if score #mode squad_mode matches 1 run function squad:do_engage
execute if score #mode squad_mode matches 2 run function squad:do_regroup
execute if score #mode squad_mode matches 3 run function squad:do_recall
