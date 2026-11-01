//! 终端里的中文对齐。
//!
//! `format!("{:<8}", "砍伐")` 会按 **char 数** 补空格，而中文在等宽字体里
//! 占两列。所以 2 个汉字到手只有 6 个空格的宽度补偿，整列会往左缩一半，
//! 表格看起来像被啃过。
//!
//! 这里按**显示列宽**算：CJK / 全角标点 / emoji 记 2 列，其余记 1 列。
//! 只覆盖实际会用到的区间，不做完整的 Unicode East Asian Width 表 ——
//! 多出来的那几千个码位对"表格对齐"没有帮助，只是把查询表撑大。

/// 一个字符占几列。
pub fn char_width(c: char) -> usize {
    match c as u32 {
        // 韩文 / 中日韩统一表意文字 / 全角形式 / 平假名片假名 / 注音
        0x1100..=0x115F
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1F64F
        | 0x1F900..=0x1F9FF
        | 0x20000..=0x3FFFD => 2,
        _ => 1,
    }
}

/// 字符串占几列。
pub fn width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

/// 按列宽截断。截断点落在宽字符中间时整字丢掉，不留半个字。
pub fn truncate(s: &str, cols: usize) -> String {
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = char_width(c);
        if w + cw > cols {
            break;
        }
        out.push(c);
        w += cw;
    }
    out
}

/// 左对齐补到 `cols` 列。
pub fn lpad(s: &str, cols: usize) -> String {
    let s = truncate(s, cols);
    let pad = cols.saturating_sub(width(&s));
    let mut out = s;
    out.extend(std::iter::repeat_n(' ', pad));
    out
}

/// 右对齐补到 `cols` 列。
pub fn rpad(s: &str, cols: usize) -> String {
    let s = truncate(s, cols);
    let pad = cols.saturating_sub(width(&s));
    let mut out = String::new();
    out.extend(std::iter::repeat_n(' ', pad));
    out.push_str(&s);
    out
}

/// 居中对齐补到 `cols` 列（奇数余量时右边多一列）。
pub fn cpad(s: &str, cols: usize) -> String {
    let s = truncate(s, cols);
    let pad = cols.saturating_sub(width(&s));
    let left = pad / 2;
    let mut out = String::new();
    out.extend(std::iter::repeat_n(' ', left));
    out.push_str(&s);
    out.extend(std::iter::repeat_n(' ', pad - left));
    out
}

/// 把一行按 `|` 分隔的表格行算好宽度。返回每列的最大显示宽度。
pub fn column_widths(rows: &[Vec<&str>]) -> Vec<usize> {
    let n = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut w = vec![0usize; n];
    for r in rows {
        for (i, cell) in r.iter().enumerate() {
            w[i] = w[i].max(width(cell));
        }
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cjk_counts_as_two_columns() {
        assert_eq!(width("abc"), 3);
        assert_eq!(width("砍伐"), 4);
        assert_eq!(width("砍a"), 3);
        assert_eq!(width(""), 0);
        // 全角括号、全角逗号都是 2 列
        assert_eq!(width("（，）"), 6);
    }

    #[test]
    fn padding_uses_display_columns_not_chars() {
        // 这是本模块存在的唯一理由：`{:<6}` 会给「砍伐」补 4 个空格，
        // 显示出来却是 8 列宽；这里补 2 个才是真的 6 列。
        assert_eq!(lpad("砍伐", 6), "砍伐  ");
        assert_eq!(width(&lpad("砍伐", 6)), 6);
        assert_eq!(lpad("砍伐", 6).chars().count(), 4, "符号数 4 个");
        assert_eq!(width(&rpad("ab", 5)), 5);
        assert_eq!(rpad("ab", 5), "   ab");
        // 超宽的会被截到目标宽度（不补也不溢出）
        assert_eq!(width(&lpad("砍伐砍伐", 4)), 4);
    }

    #[test]
    fn truncate_never_splits_a_wide_char() {
        assert_eq!(truncate("砍伐砍伐", 5), "砍伐");
        assert_eq!(width(&truncate("砍伐砍伐", 5)), 4);
        assert_eq!(truncate("abc", 2), "ab");
        assert_eq!(truncate("abcd", 9), "abcd");
    }

    #[test]
    fn column_widths_takes_max_per_column() {
        let rows = vec![vec!["砍伐", "ok"], vec!["接近", "no_wood"]];
        assert_eq!(column_widths(&rows), vec![4, 7]);
    }
}
