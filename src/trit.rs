//! 三值基础类型与逻辑。
//!
//! 与 `bfxh/trit` 的定义保持一致：
//! - `Tri::Neg` = -1，`Tri::Zero` = 0（未知 / 弃权），`Tri::Pos` = +1
//! - 逻辑用 **K3（Kleene 三值）**：`Unknown` 参与运算时，结论无法确定就返回 `Unknown`，
//!   **绝不把 Unknown 猜成 Pos**。这是整个状态机的安全底座。

use std::fmt;

/// 平衡三进制的一个 trit。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash, Default)]
pub enum Tri {
    /// -1：反对 / 后退 / 明确否定
    Neg,
    /// 0：未知 / 弃权 / 保持原状
    #[default]
    Zero,
    /// +1：支持 / 前进 / 明确肯定
    Pos,
}

impl Tri {
    /// 从 -1/0/1 构造；越界值一律落到 `Zero`（不猜）。
    pub fn from_i8(v: i8) -> Tri {
        match v.signum() {
            -1 => Tri::Neg,
            0 => Tri::Zero,
            _ => Tri::Pos,
        }
    }

    /// 从任意整数构造（只看符号）。
    pub fn from_i64(v: i64) -> Tri {
        Tri::from_i8(v.signum().clamp(-1, 1) as i8)
    }

    pub fn to_i8(self) -> i8 {
        match self {
            Tri::Neg => -1,
            Tri::Zero => 0,
            Tri::Pos => 1,
        }
    }

    pub fn to_i32(self) -> i32 {
        self.to_i8() as i32
    }

    pub fn to_f32(self) -> f32 {
        self.to_i8() as f32
    }

    pub fn is_known(self) -> bool {
        self != Tri::Zero
    }

    /// K3 否定：`Unknown` 的否定仍是 `Unknown`。
    pub fn not(self) -> Tri {
        match self {
            Tri::Neg => Tri::Pos,
            Tri::Zero => Tri::Zero,
            Tri::Pos => Tri::Neg,
        }
    }

    /// K3 合取：只有一方是 Neg 就立刻 Neg；否则只要有一方 Unknown 就 Unknown。
    pub fn and(self, rhs: Tri) -> Tri {
        match (self, rhs) {
            (Tri::Neg, _) | (_, Tri::Neg) => Tri::Neg,
            (Tri::Zero, _) | (_, Tri::Zero) => Tri::Zero,
            (Tri::Pos, Tri::Pos) => Tri::Pos,
        }
    }

    /// K3 析取：只有一方是 Pos 就立刻 Pos；否则只要有一方 Unknown 就 Unknown。
    pub fn or(self, rhs: Tri) -> Tri {
        match (self, rhs) {
            (Tri::Pos, _) | (_, Tri::Pos) => Tri::Pos,
            (Tri::Zero, _) | (_, Tri::Zero) => Tri::Zero,
            (Tri::Neg, Tri::Neg) => Tri::Neg,
        }
    }

    /// K3 蕴含（Ł3 的 `a → b` 在这里用 `¬a ∨ b` 的 Kleene 版本）。
    pub fn implies(self, rhs: Tri) -> Tri {
        self.not().or(rhs)
    }

    /// 三值多数表决（等价于 `(a∧b)∨(b∧c)∨(c∧a)`）。
    pub fn majority(a: Tri, b: Tri, c: Tri) -> Tri {
        a.and(b).or(b.and(c)).or(c.and(a))
    }

    /// 符号字符，便于日志与回放。
    pub fn ch(self) -> char {
        match self {
            Tri::Neg => '-',
            Tri::Zero => '0',
            Tri::Pos => '+',
        }
    }
}

impl From<bool> for Tri {
    fn from(b: bool) -> Tri {
        if b {
            Tri::Pos
        } else {
            Tri::Neg
        }
    }
}

impl From<i8> for Tri {
    fn from(v: i8) -> Tri {
        Tri::from_i8(v)
    }
}

impl std::ops::Neg for Tri {
    type Output = Tri;
    fn neg(self) -> Tri {
        self.not()
    }
}

impl fmt::Display for Tri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.ch())
    }
}

/// 平衡三进制 <-> 十进制。用于把一串 trit 打包成紧凑的策略编号 /
/// 记分板值，方便和游戏内 `/scoreboard` 对齐。
pub fn to_balanced(mut v: i64) -> Vec<Tri> {
    if v == 0 {
        return vec![Tri::Zero];
    }
    let mut out = Vec::new();
    while v != 0 {
        // 余数取到 [-1,0,1]
        let r = ((v % 3) + 3) % 3; // 0,1,2
        let (trit, carry) = match r {
            0 => (Tri::Zero, 0),
            1 => (Tri::Pos, 1),
            _ => (Tri::Neg, 2), // r==2 -> -1，向高位进 1
        };
        out.push(trit);
        v = (v - trit.to_i64()) / 3;
        let _ = carry;
    }
    out
}

/// `to_balanced` 的逆运算。
pub fn from_balanced(trits: &[Tri]) -> i64 {
    let mut acc: i64 = 0;
    let mut p: i64 = 1;
    for t in trits {
        acc += t.to_i64() * p;
        p *= 3;
    }
    acc
}

impl Tri {
    pub fn to_i64(self) -> i64 {
        self.to_i8() as i64
    }
}

/// 一串 trit 的紧凑文本形式，例如 `+0-+`。日志与回放都用它。
pub fn trits_str(trits: &[Tri]) -> String {
    trits.iter().map(|t| t.ch()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn k3_not_is_involution() {
        for t in [Tri::Neg, Tri::Zero, Tri::Pos] {
            assert_eq!(t.not().not(), t);
        }
        assert_eq!(Tri::Zero.not(), Tri::Zero);
    }

    #[test]
    fn k3_truth_tables() {
        // and：Neg 吸收
        assert_eq!(Tri::Neg.and(Tri::Zero), Tri::Neg);
        assert_eq!(Tri::Pos.and(Tri::Zero), Tri::Zero);
        assert_eq!(Tri::Zero.and(Tri::Zero), Tri::Zero);
        // or：Pos 吸收
        assert_eq!(Tri::Pos.or(Tri::Zero), Tri::Pos);
        assert_eq!(Tri::Neg.or(Tri::Zero), Tri::Zero);
        assert_eq!(Tri::Neg.or(Tri::Neg), Tri::Neg);
        // 关键：Unknown 不会被猜成 Pos
        assert_ne!(Tri::Zero.and(Tri::Zero), Tri::Pos);
    }

    #[test]
    fn de_morgan_holds() {
        for a in [Tri::Neg, Tri::Zero, Tri::Pos] {
            for b in [Tri::Neg, Tri::Zero, Tri::Pos] {
                assert_eq!(a.and(b).not(), a.not().or(b.not()));
                assert_eq!(a.or(b).not(), a.not().and(b.not()));
            }
        }
    }

    #[test]
    fn majority_matches_vote() {
        assert_eq!(Tri::majority(Tri::Pos, Tri::Pos, Tri::Neg), Tri::Pos);
        assert_eq!(Tri::majority(Tri::Neg, Tri::Neg, Tri::Pos), Tri::Neg);
        assert_eq!(Tri::majority(Tri::Pos, Tri::Neg, Tri::Zero), Tri::Zero);
    }

    #[test]
    fn balanced_roundtrip() {
        for v in -1000i64..=1000 {
            let t = to_balanced(v);
            assert_eq!(from_balanced(&t), v, "value={v} trits={}", trits_str(&t));
        }
        // 5 = -1*1 + -1*3 + 1*9
        assert_eq!(trits_str(&to_balanced(5)), "--+");
    }
}
