//! trit-agent：三值决策状态机 + Minecraft 角色托管。
//!
//! 分层：
//! - [`trit`]：三值类型与 K3 逻辑、平衡三进制编解码
//! - [`fsm`]：投票 / 裁决（迟滞 + 弃权）/ 阶段机
//! - [`behaviors`]：行为库，每个行为独立投票
//! - [`npc`]：从 mob mod 的 AI Goal 反编译提炼的三值战术判据
//! - [`perceive`]：日志感知
//! - [`act`]：输入注入
//! - [`agent`]：把三层接起来跑高频循环

pub mod act;
pub mod agent;
pub mod behaviors;
pub mod config;
pub mod fsm;
pub mod npc;
pub mod perceive;
pub mod trit;
