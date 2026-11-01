//! 从整合包里的 mob mod 反编译出来的「NPC 战术」，三值化后复用。
//!
//! 这些 mod 的 Goal 类里没有一处是三值的 —— 全是 `if (distSqr < 49.0)`
//! 这种硬编码阈值。但它们把**「什么时候该退」写丢了**：只有“靠近/开火”两个
//! 分支，没有“位置刚好、保持住”这个中间态。于是 NPC 要么贴脸，要么站远程位
//! 僵住，中间那段只能靠互相撞。
//!
//! 这里把每个阈值升一格：阈值下方是 `Neg`（太近，要退），区间内是 `Zero`
//! （合适的距离，谁也别推我），上方是 `Pos`（太远，要进）。`Zero` 在这里
//! 不是“未知”，是**明确的“别动”**，用高权重投票就能压住巡逻/追击，
//! 这正是原版 Goal 缺的那一档。
//!
//! 数值全部来自反编译常量池，出处见各常量注释。`(N = x²)` 表示原作者写的是
//! `distanceToSqr`，所以原始字面量是平方数。

use crate::trit::Tri;

/// 近战有效距离。`hostile_humans/MeleeAttackGoal`，攻击判定 3 格。
pub const MELEE_RANGE: f32 = 3.0;

/// 远程停火距离。`enhancedai/EARangedBowAttackGoal` 字面量 49.0 = 7² —— 
/// 到 7 格就停止后撤、原地开火。
pub const RANGED_STOP: f32 = 7.0;

/// 追击上限。`hostile_humans/MeleeAttackGoal` 字面量 1024.0 = 32²，
/// 超了就放弃目标。这里拿来做“别追到天涯”的闸门。
pub const CHASE_CAP: f32 = 32.0;

/// 躲苦力怕。`hostile_humans/AvoidCreeperGoal` 字面量 10.0。
pub const CREEPER_FLEE: f32 = 10.0;

/// 躲 TNT。`hostile_humans/AvoidTNTGoal` 字面量 3.0 —— TNT 有引信倒计时，
/// 所以比苦力怕近得多才躲。
pub const TNT_FLEE: f32 = 3.0;

/// 进食安全半径。`hostile_humans/FoodItemGoal` 与 `TameItemGoal` 都写 36.0 = 6²，
/// 意思是 6 格内没威胁才低头吃东西。
pub const EAT_SAFE: f32 = 6.0;

/// 格挡持续。`enhancedai/ShieldingGoal` 字面量 30.0（tick）= 1.5 秒。
pub const SHIELD_TICKS: u64 = 30;

/// 低血线（占最大血量比例）。`humancompanions/LowHealthGoal` 没有暴露常量，
/// 但它和 `RaiseShieldGoal` 的 4.0/5.0 是一组：5 血以下开始求生。
/// 5/20 的常规血量 = 0.25，取 0.35 留一点余量给延迟。
pub const LOW_HP: f32 = 0.35;

/// 巡逻半径。`humancompanions/PatrolGoal` 的 radius 参数，默认 12。
pub const PATROL_RADIUS: f32 = 12.0;

/// 数据包只报距离档位（算精确距离要遍历实体太贵），这里还原成档内代表值。
///
/// 档位数字是**该档上界**（`distance=..N`），代表值故意取得偏小 —— 因为
/// 决策里「以为更近」只会让角色多退半步，「以为更远」才会让它莽上去。
/// 偏保守的一侧永远是安全的一侧。
///
/// 映射：3→1.8 / 6→4.5 / 10→8 / 16→13 / 32→24
pub fn tier_to_dist(tier: i32) -> Option<f32> {
    match tier {
        ..=0 => None,        // 没有目标
        1..=3 => Some(1.8),  // <=3 格：贴脸，该退
        4..=6 => Some(4.5),  // <=6 格：理想交战距离
        7..=10 => Some(8.0), // <=10 格：略远，追
        11..=16 => Some(13.0),
        17..=32 => Some(24.0),
        33.. => Some(48.0),  // 很远
    }
}

/// 距离三值尺：`Pos` 偏远、`Neg` 偏近、`Zero` 落在区间内。
///
/// 注意方向 —— 这是「距离尺」不是「危险尺」：值越大越远。判断危险要么把
/// 阈值反过来用（见 [`hazard_band`]），要么调用方自己 `.not()`。
/// 未知距离返回 `Zero`，但调用方必须自己决定要不要把它当成“别动”——
/// 本函数不替你猜。
pub fn band(dist: f32, near: f32, far: f32) -> Tri {
    if !dist.is_finite() {
        Tri::Zero
    } else if dist < near {
        Tri::Neg
    } else if dist <= far {
        Tri::Zero
    } else {
        Tri::Pos
    }
}

/// 交战距离判据：进到 `MELEE_RANGE` 就够，超过 `RANGED_STOP` 就该追。
///
/// 两个阈值之间是“打到但不用贴脸”的窗口——原版 Goal 里没有这个窗口，
/// 所以 NPC 会一直挤到你身上。
pub fn engage_band(dist: Option<f32>) -> Tri {
    match dist {
        Some(d) => band(d, MELEE_RANGE, RANGED_STOP),
        None => Tri::Zero,
    }
}

/// 危险源判据：`Pos` 必须躲，`Neg` 明确安全，`Zero` 无感知。
///
/// 阈值来自两个 Goal：苦力怕 10 格（`AvoidCreeperGoal`，因为苦力怕会追你，
/// 所以起跑线要远），TNT 3 格（`AvoidTNTGoal`，TNT 不会追，3 格起跑够用）。
/// 两个都判，取更严重的那个。
///
/// 完全没感知时返回 `Zero` 而不是 `Neg` —— 「没看到爆炸物」和「确定没有爆炸物」
/// 是两件事，前者不该让 AI 放心往前走。
pub fn hazard_band(creeper: Option<f32>, tnt: Option<f32>) -> Tri {
    if creeper.is_none() && tnt.is_none() {
        return Tri::Zero;
    }
    let mut out = Tri::Neg; // 有感知，且暂时都不近
    if let Some(d) = creeper {
        if d <= CREEPER_FLEE {
            out = out.or(Tri::Pos);
        }
    }
    if let Some(d) = tnt {
        if d <= TNT_FLEE {
            out = out.or(Tri::Pos);
        }
    }
    out
}

/// 冷却门：到点了给 `Pos`（可用），没到给 `Neg`（冷却中）。
///
/// `now` / `last` 是单调毫秒。`last == 0` 视为从未用过。
pub fn gate(now: u64, last: u64, cd_ms: u64) -> Tri {
    if last == 0 || now.saturating_sub(last) >= cd_ms {
        Tri::Pos
    } else {
        Tri::Neg
    }
}

/// 学习率式的降级：把连续读数压成三值，只有“持续往一个方向”才给结论。
///
/// 用途：`hostile_humans/AvoidCreeperGoal` 这类每 tick 重算的 Goal 容易抖动，
/// 原作者的解法是加 `path` 重算节流。这里用三值累积代替：短窗口内噪音互相抵消，
/// 净效应低于噪声带就退化成 `Zero`（不动），而不是来回擦键盘。
pub fn accumulate(samples: &[f32], noise: f32) -> Tri {
    if samples.is_empty() || !noise.is_finite() || noise <= 0.0 {
        return Tri::Zero;
    }
    let sum: f32 = samples.iter().filter(|v| v.is_finite()).sum();
    if sum >= noise {
        Tri::Pos
    } else if sum <= -noise {
        Tri::Neg
    } else {
        Tri::Zero
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_is_inclusive_at_far_edge() {
        assert_eq!(band(2.9, 3.0, 7.0), Tri::Neg);
        assert_eq!(band(3.0, 3.0, 7.0), Tri::Zero);
        assert_eq!(band(7.0, 3.0, 7.0), Tri::Zero);
        assert_eq!(band(7.1, 3.0, 7.0), Tri::Pos);
    }

    #[test]
    fn engage_band_has_a_hold_window() {
        // 原版 Goal 缺的就是这一段：3~7 格之间应该站住，不再往前挤
        assert_eq!(engage_band(Some(1.5)), Tri::Neg);
        assert_eq!(engage_band(Some(5.0)), Tri::Zero);
        assert_eq!(engage_band(Some(20.0)), Tri::Pos);
        assert_eq!(engage_band(None), Tri::Zero, "未知距离不许猜成前进");
    }

    #[test]
    fn creeper_far_is_negative() {
        assert_eq!(hazard_band(Some(20.0), None), Tri::Neg);
        assert_eq!(hazard_band(Some(8.0), None), Tri::Pos);
        assert_eq!(hazard_band(Some(1.0), None), Tri::Pos);
    }

    #[test]
    fn tnt_ignores_mid_range_but_not_close() {
        // 5 格外的 TNT 不构成危险（引信还在烧），3 格内一定跑
        assert_eq!(hazard_band(None, Some(5.0)), Tri::Neg);
        assert_eq!(hazard_band(None, Some(2.5)), Tri::Pos);
    }

    #[test]
    fn hazard_takes_the_worse_of_two() {
        // 苦力怕安全但 TNT 贴脸 → 仍然是危险
        assert_eq!(hazard_band(Some(15.0), Some(1.0)), Tri::Pos);
    }

    #[test]
    fn gate_respects_cooldown() {
        assert_eq!(gate(1000, 0, 500), Tri::Pos);
        assert_eq!(gate(1000, 800, 500), Tri::Neg);
        assert_eq!(gate(1400, 800, 500), Tri::Pos);
    }

    #[test]
    fn accumulate_rejects_noise() {
        assert_eq!(accumulate(&[1.0, -1.0, 1.0, -0.9], 2.0), Tri::Zero);
        assert_eq!(accumulate(&[1.0, 1.0, 1.0], 2.0), Tri::Pos);
        assert_eq!(accumulate(&[], 2.0), Tri::Zero);
    }

    #[test]
    fn unknown_distance_is_not_forward() {
        assert_ne!(engage_band(None), Tri::Pos);
        assert_eq!(tier_to_dist(0), None);
        assert_eq!(tier_to_dist(8), Some(8.0));
        assert_eq!(tier_to_dist(3), Some(1.8));
    }
}
