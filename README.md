# trit-agent

用三值决策状态机替你在 Minecraft 里操作角色。Windows / 零依赖 Rust / 不需要给游戏装 mod。

配套的起源数据包（召唤师、NPC 能力、传感器）也在这个仓库里。

```
三值决策层   每 5ms 一拍（默认 200Hz，可到 2000Hz）
    ↓ 投票 + 裁决（迟滞 / 弃权）
输入注入层   SendInput 直接打进游戏窗口（键鼠，不是模拟点击）
    ↓
感知回填层   logs/latest.log ← 聊天框 ← 数据包 tellraw
```

## 为什么是三值

状态机不产出「做/不做」两档，而是每个通道产出 `-1 / 0 / +1`：

| 值 | 含义 | 行为 |
|---|---|---|
| `+1` | 明确要 | 执行 |
| `-1` | 明确反对 | 反向执行 |
| `0` | **不知道** | 弃权，交还给人 |

三条硬规则（`src/fsm.rs`）：

1. **没人投有效票 → 松手**。不会因为「默认值」乱动。
2. **未知票权重 > 已知票权重 → 弃权**。这是整个东西的安全底线：
   没感知到敌人时它不会瞎打，血量未知时它不会硬冲。
3. **迟滞**：从 `0` 转到 `+1` 要过 `阈值`，从 `-1` 翻到 `+1` 要过 `阈值 + 迟滞`。
   鼠标抖动、日志延迟导致的来回横跳会被吃掉。

一个反直觉的地方：**阶段转移表不能直接用 K3 的 `and` 写**。
`威胁 AND 血量健康` 在 K3 里会得到 `Neg`（因为 `Neg` 在合取里吸收），
得出「满血 → 不交战」的错结论。所以转移写成 3×3 表，见 `Machine::next_phase`。

## 从 NPC 那里偷来的战术

整合包里的 `hostile_humans`、`enhancedai`、`improvedmobs`、`humancompanions`
这几个 mod 的 NPC 打得很像人，但它们的 Goal 类里没有一处是三值的 ——
全是 `if (distanceToSqr < 49.0)` 这种硬编码阈值。我把这些 Goal 反编译，
把常量池里的数值一个个挖出来（`(N = x²)` 表示原作者写的是 `distanceToSqr`），
然后用三值重写了一遍。见 `src/npc.rs`。

| 移植过来的 | 来源 Goal | 原始数值 | 三值化之后 |
|---|---|---|---|
| 交战距离带 | `hostile_humans/MeleeAttackGoal` + `enhancedai/EARangedBowAttackGoal` | 3 格 / 49.0 = 7² 格 | 远 `+` 追 / **刚好 `0` 站住** / 贴脸 `-` 退半步 |
| 爆炸物规避 | `AvoidCreeperGoal` / `AvoidTNTGoal` | 10.0 / 3.0 格 | `+` 必须躲 / `-` 明确安全 / `0` 无感知 |
| 举盾格挡 | `enhancedai/ShieldingGoal` / `RaiseShieldGoal` | 30 tick = 1.5 秒 | 受伤 + 近身才 `+` |
| 卡住拆墙 | `improvedmobs/BlockBreakGoal` / `DiggingGoal` | 0.5 / 1.1 / 4.0 | 卡住 → 低头 `+` 挥击 `+` |
| 进食安全半径 | `hostile_humans/FoodItemGoal` | 36.0 = 6² 格 | 6 格内有东西就不吃 |
| 巡逻半径 | `humancompanions/PatrolGoal` | radius 12 | 超出半径 → 停，只转头 |

**最关键的是第一行中间那一档。** 原版 Goal 只有「靠近」和「开火」两个分支，
没有「位置刚好、保持住」这个中间态，所以 NPC 会一路挤到你脸上互相卡位。
三值化之后这一段是 `0`，用高权重弃权票表达「位置正好，别推我」——
它天然压掉巡逻和其他前进票，又不会像 `-` 那样把人推回去。

`cargo run --release -- sim` 会把这条曲线跑给你看：

```
tick  phase   move strafe yaw pitch atk jump use  说明
0     idle     0     0     -    0    0   0    0   无敌人
2000  engage   +     -     +    0    +   0    0   敌人 ≤32 格 → 追
5000  engage   0     +     +    0    0   0    0   敌人 ≤6 格 → 站住（原版没有这一档）
7000  engage   -     +     +    0    0   0    0   敌人 ≤3 格 → 退半步
8000  engage   -     +     +    0    0   0    0   敌人在 10 格 + 苦力怕贴脸 → 规避压过追击
12000 engage   +     +     +    +    +   0    0   被墙挡住：想追但走不动 → 低头砍墙  [stuck]
```

`move` 列三段 `+` / `0` / `-` 就是这次移植的全部意义。

四条战术都能单独关，不想要哪条就关哪条（`trit-agent.toml`）：

```toml
tac_range  = true   # 交战距离带
tac_hazard = true   # 爆炸物规避
tac_guard  = true   # 受伤近身举盾（手上要是盾；拿食物会变成进食）
tac_breach = true   # 卡住低头砍墙
```

游戏内也能切：`!tac off`、`!tac guard`、`!status` 会显示 `tac=+-++` 这样的四位状态。

## 实测

同一台机器，release 构建，`cargo run --release -- selftest`：

```
目标 200 Hz  · 实际 200.5 Hz · 401 拍 / 2.000s
目标 1000 Hz · 实际 1000.5 Hz · 2001 拍 / 2.000s
```

单拍开销是「构建 WorldView + 6 个行为投票 + 7 通道裁决 + 输入去重」，
不含系统调用。真正拖慢的是 Windows 定时器精度（默认 15.6ms），
所以启动时调了 `timeBeginPeriod(1)`。

## 用法

```bash
cargo build --release

# 先确认能找到窗口和日志
./target/release/trit-agent.exe probe --profile "E:/MC/MC/profiles/烦人的村民"

# 接管
./target/release/trit-agent.exe run --profile "E:/MC/MC/profiles/烦人的村民"
```

首次运行会生成 `trit-agent.toml`。

### 热键

| 键 | 作用 |
|---|---|
| `F8` | 开关 AI。关 = 立刻松开所有键，控制权回你 |
| `F9` | 急停（关闭 + 强制松手） |
| `F6` | 单独开关「攻击」这一项次操作 |
| `F7` | 单独开关「移动」这一项次操作 |

### 游戏内聊天指令

AI 开着的时候直接打字，不用切窗口：

```
!ai off              交出控制权
!status              看当前阶段/血量
!mode patrol         切换 hold / patrol / follow
!attack off          只关攻击，其他照常
!move off     !look off     !jump off     !use off
!tac off             关掉四条移植战术；!tac guard 只翻格挡
!threat              手动标记「有威胁」
!calm                手动标记「安全」
!panic               急停
```

每一项「次操作」都能单独摘掉，摘掉后那一路通道永久 `Zero`，AI 不再碰它。

### 第三视角

`third_person_on_start = true`（默认）时，接管启动会先聚焦窗口再注入 F5，
所以是背视第三人称开局。

原版没有「默认视角」这个设置，所以不通过 agent 启动时仍然是第一人称 ——
这是注入的，不是配置项。按几次由 `perspective_presses` 控制（1 背视 / 2 前视）。

## 数据包

```
datapacks/
  summoner_origin/   召唤师起源：3 只战狼 + 铁卫，召唤物会主动打村民和整合包敌对 NPC
  npc_origins/       给 NPC 挂「起源式」效果（打 tag 生效，默认无副作用）
  trit_bridge/       传感器：把血量/坐标/周围敌人数写进聊天，供状态机读
  summoner_lang/     召唤师起源的中文名称（资源包）
```

安装：

```bash
python scripts/build_datapacks.py --install "E:/MC/MC/profiles/烦人的村民"
```

进游戏 `/reload`，然后：

```
/function trit_bridge:on      # 开传感器（默认关，因为会占聊天框）
/function trit_bridge:off
```

传感器输出长这样，每秒一行：

```
<TRIT hp=185 food=17 x=123 y=-601 z=88 h=3 d=6 c=0>
```

| 字段 | 含义 |
|---|---|
| `h` | 32 格内敌对实体数（上限 32） |
| `d` | 最近敌人的距离档：`3 / 6 / 10 / 16 / 32`，0 = 视野内没有 |
| `c` | 最近爆炸源（苦力怕/TNT/末影水晶/火球）的距离档，同上 |

距离档是「≤N 格」，用五次半径判定从远到近覆盖取最近那档，比遍历实体算精确
距离便宜得多。Rust 侧按档内代表值使用，且**故意取偏小一侧**——
「以为更近」只会多退半步，「以为更远」才会莽上去。

`d` 和 `c` 是后加的字段，旧版数据包不带也能跑（按 0 处理，等价于「没感知」）。

## 诚实的边界

**它能做的**：稳定的战斗节奏（挥击 / 跳劈 / 换边）、卡血撤退、低血站桩回血、
死亡后点重生、巡逻扫视、随时接管与交还。

**它做不到的**：

- **没有视觉，也不知道敌人方位**。数据包只给「最近敌人多远」，不给方向。
  所以视角不会朝敌人转（`Yaw` 只在待命时才扫视），也不会主动走位绕后。
- 不开数据包时它只有「日志里写了什么」，也就是死亡消息和你打的字。
  这时候 `assume_threat = false`（默认）会让它站着不动 —— 这是刻意的，
  猜错方向的代价比不动大。
- **反应延迟 ≥ 1 个传感器周期**（默认 1 秒）。这是「日志 → 状态机」这条链的
  物理下限，高频节拍抹不掉它。想要更低只能写 mod 直接读内存。
- **不认路、不会寻路、不会挖矿**。没有 `baritone` 那种路径规划。
- **鼠标灵敏度要手调**。MC 的「每像素转角」跟灵敏度设置强相关，
  默认 `look_gain = 1.35` 是按灵敏度 0.5 估的。转太快就调小，转太慢就调大。
- **手眼要分开看**：AI 会抢你的鼠标和键盘。按 `F8` 关掉才是安全的对话姿势。

**没做的验证**：本机没在真游戏里跑过端到端（这机器同时开游戏和编译会打架）。
已验证的是：55 个测试（K3 真值表、平衡三进制往返、迟滞不抖、弃权优先、
日志解析、端到端日志→决策）、release 构建 0 警告、高频实测数据、数据包 JSON 校验。
第一次实机如果有问题，看 `trit-agent-run.log`（每 100ms 一行决策编码），
或者 `docs/BOUNDARIES.md` 里记的那几个已经踩过的坑。

## 和 `bfxh/trit` 的关系

同一套三值语义，不同的落点。`trit` 把三值用在逻辑求值、打包、GPU 内核上；
这里把三值用在**实时决策**上：`Unknown` 不参与猜测，只用来表达「弃权」。

## 许可

MIT
