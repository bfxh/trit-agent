//! 尾随 `latest.log`。支持文件被替换/截断（重开游戏）后自动从头读。

use super::{classify, LogEvent};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub struct LogTail {
    path: PathBuf,
    pos: u64,
    buf: String,
}

impl LogTail {
    pub fn new(path: impl AsRef<Path>) -> LogTail {
        LogTail {
            path: path.as_ref().to_path_buf(),
            pos: 0,
            buf: String::new(),
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
        let md = match std::fs::metadata(&self.path) {
            Ok(m) => m,
            Err(_) => return Vec::new(),
        };
        let len = md.len();
        if len < self.pos {
            // 文件被截断 / 换成了新的一局
            self.pos = 0;
            self.buf.clear();
        }
        if len == self.pos {
            return Vec::new();
        }

        let mut f = match File::open(&self.path) {
            Ok(f) => f,
            Err(_) => return Vec::new(),
        };
        if f.seek(SeekFrom::Start(self.pos)).is_err() {
            return Vec::new();
        }
        let mut chunk = Vec::new();
        if f.read_to_end(&mut chunk).is_err() {
            return Vec::new();
        }
        self.pos += chunk.len() as u64;

        let text = String::from_utf8_lossy(&chunk);
        self.buf.push_str(&text);

        let mut out = Vec::new();
        while let Some(nl) = self.buf.find('\n') {
            let line = self.buf[..nl].trim_end_matches('\r').to_string();
            self.buf.drain(..=nl);
            if line.is_empty() {
                continue;
            }
            if let Some(ev) = classify(&line) {
                out.push(ev);
            }
        }
        // 防御：单行超长（一般是崩溃堆栈）直接丢掉，避免内存涨
        if self.buf.len() > 64 * 1024 {
            self.buf.clear();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn tails_only_new_lines() {
        let dir = std::env::temp_dir().join("trit-agent-test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("tail.log");
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
}
