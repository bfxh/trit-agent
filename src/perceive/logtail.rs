//! 尾随 `latest.log`。支持文件被替换/截断（重开游戏）后自动从头读。
//!
//! 这个文件在一个每秒刷新 10 次的通道上是热路径，所以两处写法跟"直观写法"
//! 不一样，都是踩出来的：
//!
//! 1. **整段读完再统一 `drain` 一次**，而不是每解析一行就 `drain` 一次。
//!    后者看起来更"流式"，但 `String::drain` 要把后面的字节往前搬 —— 一次
//!    poll 里解析 N 行就是 O(N²) 的搬运。日志突然刷屏（mod 报错、崩溃堆栈）
//!    的时候正好是最需要它不卡的时候。
//! 2. **复用读缓冲和事件缓冲**。每 50ms 分配一个 `Vec<u8>` + `Vec<LogEvent>`
//!    在总量上不算什么，但这是唯一一条每拍都跑的路径，省掉就省掉了。

use super::{classify, LogEvent};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub struct LogTail {
    path: PathBuf,
    pos: u64,
    buf: String,
    /// 复用的读缓冲，避免每次 poll 都新分配
    chunk: Vec<u8>,
}

impl LogTail {
    pub fn new(path: impl AsRef<Path>) -> LogTail {
        LogTail {
            path: path.as_ref().to_path_buf(),
            pos: 0,
            buf: String::with_capacity(8 * 1024),
            chunk: Vec::with_capacity(16 * 1024),
        }
    }

    /// 首次读取时直接跳到末尾：我们只关心启动之后发生的事。
    pub fn skip_to_end(&mut self) {
        if let Ok(md) = std::fs::metadata(&self.path) {
            self.pos = md.len();
        }
    }

    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 读出新增内容并解析成事件。无新内容返回空 vec。
    pub fn poll(&mut self) -> Vec<LogEvent> {
        let mut out = Vec::new();
        self.poll_into(&mut out);
        out
    }

    /// 同上，但复用调用方的缓冲。主循环走这条。
    pub fn poll_into(&mut self, out: &mut Vec<LogEvent>) {
        let md = match std::fs::metadata(&self.path) {
            Ok(m) => m,
            Err(_) => return,
        };
        let len = md.len();
        if len < self.pos {
            // 文件被截断 / 换成了新的一局
            self.pos = 0;
            self.buf.clear();
        }
        if len == self.pos {
            return;
        }

        let mut f = match File::open(&self.path) {
            Ok(f) => f,
            Err(_) => return,
        };
        if f.seek(SeekFrom::Start(self.pos)).is_err() {
            return;
        }
        self.chunk.clear();
        if f.read_to_end(&mut self.chunk).is_err() {
            return;
        }
        self.pos += self.chunk.len() as u64;

        match std::str::from_utf8(&self.chunk) {
            Ok(s) => self.buf.push_str(s),
            // 有非法字节（日志里偶发的乱码）就退化成有损转换，不能整段丢掉：
            // 丢的是整块日志，里面可能有几十帧传感器数据。
            Err(_) => self.buf.push_str(&String::from_utf8_lossy(&self.chunk)),
        }

        // 从头逐行消费，只记游标，循环结束后一次性 drain
        let mut consumed = 0usize;
        while let Some(rel) = self.buf[consumed..].find('\n') {
            let nl = consumed + rel;
            let raw = &self.buf[consumed..nl];
            let line = raw.strip_suffix('\r').unwrap_or(raw);
            consumed = nl + 1;
            if line.is_empty() {
                continue;
            }
            if let Some(ev) = classify(line) {
                out.push(ev);
            }
        }
        if consumed > 0 {
            self.buf.drain(..consumed);
        }
        // 防御：单行超长（一般是崩溃堆栈）直接丢掉，避免内存涨
        if self.buf.len() > 64 * 1024 {
            self.buf.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("trit-agent-test");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn tails_only_new_lines() {
        let p = tmp("tail.log");
        std::fs::write(&p, "old line\n").unwrap();

        let mut t = LogTail::new(&p);
        t.skip_to_end();
        assert!(t.poll().is_empty());

        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        writeln!(f, "[x] [Render thread/INFO]: [CHAT] <bfxh> hi").unwrap();
        drop(f);

        let ev = t.poll();
        assert_eq!(ev.len(), 1);
        assert!(matches!(ev[0], LogEvent::Chat { .. }));
    }

    #[test]
    fn handles_many_lines_in_one_poll() {
        // 一次读到几十行，靠游标消费而不是逐行 drain
        let p = tmp("many.log");
        std::fs::write(&p, "").unwrap();
        let mut t = LogTail::new(&p);
        t.skip_to_end();

        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        for i in 0..50 {
            writeln!(f, "[x] [INFO]: noise line {i}").unwrap();
            writeln!(f, "[x] [Render thread/INFO]: [CHAT] <TRITF wal={i} b1=3 bd=3 p=100>")
                .unwrap();
        }
        drop(f);

        let ev = t.poll();
        assert_eq!(ev.len(), 50, "每个快帧都要解析出来，不能被整段丢掉");
        // 顺序必须保持：错序的话后一帧会被前一帧覆盖，表现为「角度不更新」
        for (i, e) in ev.iter().enumerate() {
            match e {
                LogEvent::Sensor(p) => assert_eq!(p.wall, Some(i as u8)),
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn partial_line_waits_for_newline() {
        let p = tmp("partial.log");
        std::fs::write(&p, "").unwrap();
        let mut t = LogTail::new(&p);
        t.skip_to_end();

        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        write!(f, "[x] [Render thread/INFO]: [CHAT] <TRITF wal=7").unwrap();
        f.flush().unwrap();
        assert!(t.poll().is_empty(), "半行不能当成一帧");

        writeln!(f, " b1=1 bd=3 p=0>").unwrap();
        drop(f);
        let ev = t.poll();
        assert_eq!(ev.len(), 1);
        match &ev[0] {
            LogEvent::Sensor(p) => assert_eq!(p.wall, Some(7)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn truncation_restarts_from_zero() {
        let p = tmp("trunc.log");
        // 先写一段长得多的内容，再整体换成短的 —— 只有变短才认得出是"换了一局"。
        // 同长度的覆盖检测不了（文件系统只给了长度），这是已知限制：
        // 那种情况下的表现是读到半行，第 2 行之后自动恢复正常。
        std::fs::write(
            &p,
            "[x] [Render thread/INFO]: [CHAT] a very long line that is definitely longer\n\
             [x] [Render thread/INFO]: [CHAT] second line also long enough to matter\n",
        )
        .unwrap();
        let mut t = LogTail::new(&p);
        t.skip_to_end();
        assert!(t.poll().is_empty());

        // 游戏重开，文件被截短重写
        std::fs::write(&p, "[x] [CHAT] <TRITF wal=2 b1=1 bd=1 p=0>\n").unwrap();
        let ev = t.poll();
        assert_eq!(ev.len(), 1);
        match &ev[0] {
            LogEvent::Sensor(p) => assert_eq!(p.wall, Some(2)),
            other => panic!("{other:?}"),
        }
    }
}
