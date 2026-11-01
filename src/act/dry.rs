//! 空实现：只记录动作次数，用于 `sim` / `selftest` / CI —— 不开游戏也能验证决策链。

use super::{Button, Input};
use std::collections::BTreeMap;

#[derive(Default, Debug)]
pub struct DryInput {
    pub log: Vec<String>,
    pub counts: BTreeMap<String, u32>,
    pub focused: bool,
    pub released: u32,
}

impl DryInput {
    pub fn new() -> DryInput {
        DryInput {
            focused: true,
            ..Default::default()
        }
    }
    fn bump(&mut self, k: &str) {
        *self.counts.entry(k.to_string()).or_insert(0) += 1;
    }
    pub fn count(&self, k: &str) -> u32 {
        *self.counts.get(k).unwrap_or(&0)
    }
}

impl Input for DryInput {
    fn key(&mut self, vk: u16, down: bool) {
        self.bump(if down { "key_down" } else { "key_up" });
        self.log.push(format!("key {vk:#04x} {}", if down { "down" } else { "up" }));
    }
    fn mouse_move(&mut self, dx: i32, dy: i32) {
        if dx == 0 && dy == 0 {
            return;
        }
        self.bump("mouse_move");
        self.log.push(format!("move {dx} {dy}"));
    }
    fn button(&mut self, b: Button, down: bool) {
        self.bump(if down { "btn_down" } else { "btn_up" });
        self.log.push(format!("{b:?} {}", if down { "down" } else { "up" }));
    }
    fn release_all(&mut self) {
        self.released += 1;
        self.bump("release_all");
    }
    fn focus(&mut self) -> bool {
        self.focused
    }
    fn click_center(&mut self) {
        self.bump("click_center");
    }
}
