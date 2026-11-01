//! 行为库：每个行为只投自己管得着的那几个通道，互相不直接调用。
//!
//! 「聪明」不体现在单个行为里，而体现在**仲裁**：一个行为可以投 `Zero`
//! 来表达「我反对但我说不出该往哪走」，权重够大就会把其他行为的 Pos 压成弃权。
//! 例如血量低又没交火时，`Regen` 用一条高权重弃权票把 `Patrol` 的前进压掉，
//! 角色就地站住回血 —— 这比在代码里写 `if low_hp { return }` 干净得多。

use crate::config::{Config, Mode};
use crate::fsm::{Channel, Phase, Vote, WorldView};
use crate::trit::Tri;

pub struct Ctx {
    pub w: WorldView,
    pub phase: Phase,
    pub now_ms: u64,
    pub dt_ms: u64,
}

pub trait Behavior {
    fn name(&self) -> &'static str;
    fn vote(&mut self, c: &Ctx) -> Vec<Vote>;
}

/// 交战：按节奏挥击、跳劈、左右换边。
pub struct Combat {
    next_attack: u64,
    attacks: u32,
    jump_until: u64,
    flip_at: u64,
    side: Tri,
    interval: u64,
    crit_every: u32,
    flip_ms: u64,
}

impl Combat {
    pub fn new(cfg: &Config) -> Combat {
        Combat {
            next_attack: 0,
            attacks: 0,
            jump_until: 0,
            flip_at: 0,
            side: Tri::Pos,
            interval: cfg.attack_interval_ms,
            crit_every: cfg.jump_crit_every,
            flip_ms: cfg.strafe_flip_ms,
        }
    }
}

impl Behavior for Combat {
    fn name(&self) -> &'static str {
        "combat"
    }

    fn vote(&mut self, c: &Ctx) -> Vec<Vote> {
        if c.phase != Phase::Engage {
            return Vec::new();
        }
        let mut v = Vec::with_capacity(4);

        if c.now_ms >= self.flip_at {
            self.side = self.side.not();
            self.flip_at = c.now_ms + self.flip_ms;
        }
        v.push(Vote::new(Channel::Strafe, self.side, 2, "combat"));

        if c.now_ms >= self.next_attack {
            v.push(Vote::new(Channel::Attack, Tri::Pos, 2, "combat"));
            self.next_attack = c.now_ms + self.interval;
            self.attacks = self.attacks.wrapping_add(1);
            if self.crit_every > 0 && self.attacks % self.crit_every == 0 {
                // 跳劈：跳起来在下落时命中
                self.jump_until = c.now_ms + 140;
            }
        }
        if c.now_ms < self.jump_until {
            v.push(Vote::new(Channel::Jump, Tri::Pos, 2, "combat_crit"));
        }

        // 交战时不扫视，只做小幅盯人
        v.push(Vote::new(Channel::Yaw, self.side, 1, "combat_track"));
        v
    }
}

/// 巡逻：一路向前，定期跳，周期性左右扫视。
pub struct Patrol {
    jump_at: u64,
    sweep_at: u64,
    side: Tri,
    jump_ms: u64,
    /// hold 模式下不前进，只扫视
    advance: bool,
    /// follow 模式不扫视，一路直走
    scan: bool,
}

impl Patrol {
    pub fn new(cfg: &Config) -> Patrol {
        Patrol {
            jump_at: 0,
            sweep_at: 0,
            side: Tri::Pos,
            jump_ms: cfg.patrol_jump_ms,
            advance: cfg.mode != Mode::Hold,
            scan: cfg.mode != Mode::Follow,
        }
    }
}

impl Behavior for Patrol {
    fn name(&self) -> &'static str {
        "patrol"
    }

    fn vote(&mut self, c: &Ctx) -> Vec<Vote> {
        if c.phase != Phase::Idle {
            return Vec::new();
        }
        let mut v = Vec::with_capacity(3);
        if self.advance {
            v.push(Vote::new(Channel::Forward, Tri::Pos, 3, "patrol"));
            if c.now_ms >= self.jump_at {
                v.push(Vote::new(Channel::Jump, Tri::Pos, 2, "patrol_step"));
                self.jump_at = c.now_ms + self.jump_ms;
            }
        }
        if self.scan {
            if c.now_ms >= self.sweep_at {
                self.side = self.side.not();
                self.sweep_at = c.now_ms + 2200;
            }
            v.push(Vote::new(Channel::Yaw, self.side, 2, "patrol_scan"));
        }
        v
    }
}

/// 撤退：边退边看，偶尔跳一下脱离。
pub struct Flee {
    jump_at: u64,
    side: Tri,
    flip_at: u64,
}

impl Flee {
    pub fn new() -> Flee {
        Flee {
            jump_at: 0,
            side: Tri::Pos,
            flip_at: 0,
        }
    }
}

impl Behavior for Flee {
    fn name(&self) -> &'static str {
        "flee"
    }

    fn vote(&mut self, c: &Ctx) -> Vec<Vote> {
        if c.phase != Phase::Retreat {
            return Vec::new();
        }
        let mut v = vec![
            Vote::new(Channel::Forward, Tri::Neg, 4, "flee"),
            Vote::new(Channel::Strafe, self.side, 1, "flee_juke"),
        ];
        if c.now_ms >= self.flip_at {
            self.side = self.side.not();
            self.flip_at = c.now_ms + 700;
        }
        if c.now_ms >= self.jump_at {
            v.push(Vote::new(Channel::Jump, Tri::Pos, 2, "flee_hop"));
            self.jump_at = c.now_ms + 520;
        }
        v
    }
}

/// 待命扫视：没有交战时的观察行为。
pub struct IdleScan {
    sweep_at: u64,
    side: Tri,
}

impl IdleScan {
    pub fn new() -> IdleScan {
        IdleScan {
            sweep_at: 0,
            side: Tri::Pos,
        }
    }
}

impl Behavior for IdleScan {
    fn name(&self) -> &'static str {
        "idle_scan"
    }

    fn vote(&mut self, c: &Ctx) -> Vec<Vote> {
        if c.phase != Phase::Idle {
            return Vec::new();
        }
        if c.now_ms >= self.sweep_at {
            self.side = self.side.not();
            self.sweep_at = c.now_ms + 1600;
        }
        vec![Vote::new(Channel::Yaw, self.side, 2, "idle_scan")]
    }
}

/// 吃东西：血量低且没在打的时候，周期性右键。
pub struct Eat {
    next: u64,
}

impl Eat {
    pub fn new() -> Eat {
        Eat { next: 0 }
    }
}

impl Behavior for Eat {
    fn name(&self) -> &'static str {
        "eat"
    }

    fn vote(&mut self, c: &Ctx) -> Vec<Vote> {
        let hungry = matches!(c.w.hp, Some(h) if h < 0.55);
        if !hungry || c.w.contact {
            return Vec::new();
        }
        if c.now_ms >= self.next {
            self.next = c.now_ms + 900;
            return vec![Vote::new(Channel::Use, Tri::Pos, 3, "eat")];
        }
        Vec::new()
    }
}

/// 回血：低血、无敌情时，用高权重弃权票把移动压掉。
pub struct Regen;

impl Behavior for Regen {
    fn name(&self) -> &'static str {
        "regen"
    }

    fn vote(&mut self, c: &Ctx) -> Vec<Vote> {
        if c.w.contact || c.phase != Phase::Idle {
            return Vec::new();
        }
        let low = matches!(c.w.hp, Some(h) if h < 0.6);
        if !low {
            return Vec::new();
        }
        vec![
            Vote::abstain(Channel::Forward, 6, "regen_stand"),
            Vote::abstain(Channel::Strafe, 4, "regen_stand"),
        ]
    }
}

/// 把所有行为装在一起，按配置启用。
pub struct Behaviors {
    pub combat: Combat,
    pub patrol: Patrol,
    pub flee: Flee,
    pub scan: IdleScan,
    pub eat: Eat,
    pub regen: Regen,
}

impl Behaviors {
    pub fn new(cfg: &Config) -> Behaviors {
        Behaviors {
            combat: Combat::new(cfg),
            patrol: Patrol::new(cfg),
            flee: Flee::new(),
            scan: IdleScan::new(),
            eat: Eat::new(),
            regen: Regen,
        }
    }

    pub fn vote(&mut self, c: &Ctx) -> Vec<Vote> {
        let mut out = Vec::with_capacity(8);
        out.extend(self.combat.vote(c));
        out.extend(self.patrol.vote(c));
        out.extend(self.flee.vote(c));
        out.extend(self.scan.vote(c));
        out.extend(self.eat.vote(c));
        out.extend(self.regen.vote(c));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsm::Machine;

    fn ctx(phase: Phase, now: u64, hp: f32, contact: bool) -> Ctx {
        Ctx {
            w: WorldView {
                hp: Some(hp),
                contact,
                threat: if contact { Tri::Pos } else { Tri::Neg },
                ..Default::default()
            },
            phase,
            now_ms: now,
            dt_ms: 5,
        }
    }

    #[test]
    fn regen_suppresses_patrol_via_unknown_votes() {
        let cfg = Config::default();
        let mut b = Behaviors::new(&cfg);
        let mut m = Machine::new();
        m.set_armed(true);
        let all = [true; 7];
        // 低血、无交火：Patrol 想前进(3)，Regen 弃权(6) 应把它压掉
        let c = ctx(Phase::Idle, 1000, 0.3, false);
        let votes = b.vote(&c);
        let d = m.step(&c.w, &votes, &all);
        assert_eq!(d.phase, Phase::Idle);
        assert_eq!(d.dirs[Channel::Forward.index()], Tri::Zero);
    }

    #[test]
    fn healthy_patrol_advances() {
        let mut cfg = Config::default();
        cfg.mode = Mode::Patrol;
        let mut b = Behaviors::new(&cfg);
        let mut m = Machine::new();
        m.set_armed(true);
        let all = [true; 7];
        let c = ctx(Phase::Idle, 1000, 1.0, false);
        let votes = b.vote(&c);
        let d = m.step(&c.w, &votes, &all);
        assert_eq!(d.dirs[Channel::Forward.index()], Tri::Pos);
    }

    #[test]
    fn combat_attacks_on_rhythm() {
        let cfg = Config::default();
        let mut b = Behaviors::new(&cfg);
        let mut m = Machine::new();
        m.set_armed(true);
        let all = [true; 7];
        let c = ctx(Phase::Engage, 0, 1.0, true);
        let votes = b.vote(&c);
        let d = m.step(&c.w, &votes, &all);
        assert_eq!(d.dirs[Channel::Attack.index()], Tri::Pos);
    }
}
