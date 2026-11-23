//! 文字匹配（对应 `first_design.md` 1.1 文本目标）。
//!
//! 支持包含、精确、正则、模糊（编辑距离 ≤ k）四种模式，并做全半角与大小写归一化。
//! 归一化是逐字符的一对一映射，因此命中位置在原文中的字符下标保持不变。

use regex::Regex;
use vf_project::Quad;

#[derive(Debug, Clone, PartialEq)]
pub enum MatchMode {
    Contains,
    Exact,
    /// 正则。模式本身不做归一化。
    Regex,
    /// 编辑距离不超过 `max_edits` 的子串匹配。
    Fuzzy { max_edits: usize },
}

#[derive(Debug, thiserror::Error)]
pub enum TextError {
    #[error("正则表达式无效：{0}")]
    Regex(String),
    #[error("文字目标不能为空")]
    EmptyPattern,
}

/// 命中在原文中的字符区间 `[start, end)`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    pub start: usize,
    pub end: usize,
}

/// OCR 识别出的一行文字。`quad` 为原图坐标下的四边形（左上、右上、右下、左下）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OcrLine {
    pub quad: Quad,
    pub text: String,
    pub conf: f64,
}

/// OCR 引擎（文字检测 + 识别）。输入为 RGB24 字节。
pub trait OcrEngine {
    fn recognize(&mut self, width: usize, height: usize, rgb: &[u8]) -> Result<Vec<OcrLine>, String>;
}

/// 单字符归一化：全角 ASCII → 半角，全角空格 → 空格，大写 → 小写。
/// 只有映射后仍是单个字符时才转换小写，保证一对一。
pub fn normalize_char(c: char) -> char {
    let half = match c {
        '\u{3000}' => ' ',
        '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
        _ => c,
    };
    let mut lower = half.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(l), None) => l,
        _ => half,
    }
}

fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// 已编译的文字查询。
#[derive(Debug, Clone)]
pub struct TextQuery {
    mode: MatchMode,
    normalize: bool,
    pattern: Vec<char>,
    regex: Option<Regex>,
}

impl TextQuery {
    pub fn new(pattern: &str, mode: MatchMode, normalize: bool) -> Result<Self, TextError> {
        if pattern.is_empty() {
            return Err(TextError::EmptyPattern);
        }
        let (pattern_chars, regex) = match mode {
            MatchMode::Regex => {
                let re = Regex::new(pattern).map_err(|e| TextError::Regex(e.to_string()))?;
                (Vec::new(), Some(re))
            }
            _ => {
                let chars: Vec<char> = if normalize {
                    pattern.chars().map(normalize_char).collect()
                } else {
                    pattern.chars().collect()
                };
                (chars, None)
            }
        };
        Ok(Self { mode, normalize, pattern: pattern_chars, regex })
    }

    fn text_chars(&self, line: &str) -> Vec<char> {
        if self.normalize {
            line.chars().map(normalize_char).collect()
        } else {
            line.chars().collect()
        }
    }

    /// 在一行文字中查找第一个命中。
    pub fn find(&self, line: &str) -> Option<Hit> {
        let text = self.text_chars(line);
        match &self.mode {
            MatchMode::Contains => find_window(&text, &self.pattern, self.pattern.len()),
            MatchMode::Exact => {
                if text == self.pattern {
                    Some(Hit { start: 0, end: text.len() })
                } else {
                    None
                }
            }
            MatchMode::Regex => {
                let re = self.regex.as_ref()?;
                let s: String = text.iter().collect();
                let m = re.find(&s)?;
                let start = s[..m.start()].chars().count();
                let end = start + s[m.start()..m.end()].chars().count();
                Some(Hit { start, end })
            }
            MatchMode::Fuzzy { max_edits } => {
                let p = self.pattern.len();
                let lo = p.saturating_sub(*max_edits).max(1);
                let hi = p + max_edits;
                for len in lo..=hi {
                    if len > text.len() {
                        break;
                    }
                    for start in 0..=text.len() - len {
                        if levenshtein(&text[start..start + len], &self.pattern) <= *max_edits {
                            return Some(Hit { start, end: start + len });
                        }
                    }
                }
                None
            }
        }
    }
}

fn find_window(text: &[char], pattern: &[char], len: usize) -> Option<Hit> {
    if len == 0 || len > text.len() {
        return None;
    }
    (0..=text.len() - len)
        .find(|&s| text[s..s + len] == *pattern)
        .map(|s| Hit { start: s, end: s + len })
}

/// 按字符比例估算子串在四边形中的位置。
///
/// OCR 只给出整行的四边形，子串位置是近似值：假设字符在行内均匀分布。
/// 这对等宽或接近等宽的文字足够用；比例字体上会有偏差。
pub fn sub_quad(line: &Quad, line_chars: usize, hit: Hit) -> Quad {
    if line_chars == 0 {
        return *line;
    }
    let f0 = hit.start as f64 / line_chars as f64;
    let f1 = hit.end as f64 / line_chars as f64;
    let lerp = |a: [f64; 2], b: [f64; 2], f: f64| [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f];
    // 行四边形顺序：左上、右上、右下、左下
    let (tl, tr, br, bl) = (line[0], line[1], line[2], line[3]);
    [lerp(tl, tr, f0), lerp(tl, tr, f1), lerp(bl, br, f1), lerp(bl, br, f0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(x0: f64, x1: f64) -> Quad {
        [[x0, 0.0], [x1, 0.0], [x1, 10.0], [x0, 10.0]]
    }

    #[test]
    fn normalization_maps_fullwidth_and_case() {
        assert_eq!(normalize_char('Ａ'), 'a');
        assert_eq!(normalize_char('１'), '1');
        assert_eq!(normalize_char('\u{3000}'), ' ');
        assert_eq!(normalize_char('机'), '机');
    }

    #[test]
    fn contains_finds_substring_ignoring_width_and_case() {
        let q = TextQuery::new("ABC", MatchMode::Contains, true).unwrap();
        assert_eq!(q.find("xxＡＢＣyy"), Some(Hit { start: 2, end: 5 }));
        assert_eq!(q.find("abd"), None);
    }

    #[test]
    fn exact_requires_whole_line() {
        let q = TextQuery::new("机密", MatchMode::Exact, true).unwrap();
        assert!(q.find("机密").is_some());
        assert!(q.find("机密文件").is_none());
    }

    #[test]
    fn regex_reports_character_offsets() {
        let q = TextQuery::new(r"\d{4}", MatchMode::Regex, true).unwrap();
        assert_eq!(q.find("账号 1234 结束"), Some(Hit { start: 3, end: 7 }));
    }

    #[test]
    fn invalid_regex_is_an_error() {
        assert!(matches!(TextQuery::new("(", MatchMode::Regex, true), Err(TextError::Regex(_))));
    }

    #[test]
    fn fuzzy_allows_bounded_edits() {
        let q = TextQuery::new("机密", MatchMode::Fuzzy { max_edits: 1 }, true).unwrap();
        assert!(q.find("这是机宓文件").is_some());
        assert!(q.find("这是机宓文件").is_some());
        let strict = TextQuery::new("机密", MatchMode::Fuzzy { max_edits: 0 }, true).unwrap();
        assert!(strict.find("机宓").is_none());
    }

    #[test]
    fn empty_pattern_rejected() {
        assert!(matches!(TextQuery::new("", MatchMode::Contains, true), Err(TextError::EmptyPattern)));
    }

    #[test]
    fn sub_quad_interpolates_along_line() {
        // 10 个字符占满 0..100，命中第 2 到 5 个字符
        let q = quad(0.0, 100.0);
        let s = sub_quad(&q, 10, Hit { start: 2, end: 5 });
        assert_eq!(s[0], [20.0, 0.0]);
        assert_eq!(s[1], [50.0, 0.0]);
        assert_eq!(s[2], [50.0, 10.0]);
        assert_eq!(s[3], [20.0, 10.0]);
    }
}
