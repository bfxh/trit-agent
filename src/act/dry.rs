//! 空实现：只记录动作次数，用于 `sim` / `selftest` / CI —— 不开游戏也能验证决策链。

use super::{Button, Input};
use std::collections::BTreeMap;

#[derive(Default, Debug)]
pub struct DryInput {
    pub log: Vec<String>,
    pub counts: BTreeMap<String, u32>,
    pub focused: bool,
    pub released: u32,
    /// 目标窗口是否在前台。测试里可以翻成 false 验证「失焦不动手」。
    pub foreground: bool,
    /// 最后挂上去的标题标记
    pub badge: Option<String>,
}

impl DryInput {
    pub fn new() -> DryInput {
        DryInput {
            focused: true,
            foreground: true,
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
    fn is_foreground(&mut self) -> bool {
        self.foreground
    }
    fn badge(&mut self, text: Option<&str>) {
        self.badge = text.map(|s| s.to_string());
    }
    fn refresh_badge(&mut self) {}
    fn set_foreground(&mut self, on: bool) {
        self.foreground = on;
    }
    fn last_badge(&self) -> Option<String> {
        self.badge.clone()
    }
    fn type_command(&mut self, cmd: &str) -> bool {
        // 不开游戏也能验证整条「意识 → 指令」链路
        self.bump("type_command");
        self.log.push(format!("cmd {cmd}"));
        true
    }
}
