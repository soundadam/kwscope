//! `kws stream` 的一行：Python `json.dumps` 的样子（`", "` / `": "` 分隔、浮点按
//! `repr`、非 ASCII 转义），字段顺序同 `kws_infer.stage_one.Trigger`。

use super::engine::Trigger;
use std::fmt::Write;

pub fn line(wav: &str, trigger: &Trigger, channel: Option<usize>) -> String {
    let mut s = String::from("{");
    let mut sep = "";
    let mut key = |s: &mut String, k: &str| {
        let _ = write!(s, "{sep}{}: ", py_str(k));
        sep = ", ";
    };
    key(&mut s, "wav");
    s += &py_str(wav);
    key(&mut s, "keyword");
    s += &py_str(&trigger.keyword);
    key(&mut s, "arm_frame");
    s += &trigger.arm_frame.to_string();
    key(&mut s, "decision_frame");
    s += &trigger.decision_frame.to_string();
    key(&mut s, "score");
    s += &py_float(trigger.score);
    key(&mut s, "start_offset_frames");
    s += &py_float(trigger.start_offset_frames);
    key(&mut s, "start_sample");
    s += &trigger.start_sample.to_string();
    key(&mut s, "end_sample");
    s += &trigger.end_sample.to_string();
    key(&mut s, "accepted");
    s += if trigger.accepted { "true" } else { "false" };
    if let Some(c) = channel {
        key(&mut s, "channel");
        s += &c.to_string();
    }
    if let Some(logit) = trigger.stage_two_logit {
        key(&mut s, "stage_two_logit");
        s += &py_float(logit);
    }
    s.push('}');
    s
}

/// Python 的 `repr(float)`：最短往返的数字，指数在 [-4, 16) 里写定点，否则
/// `1e-05`、`1.5e+16` 这样。
pub fn py_float(v: f64) -> String {
    if v.is_nan() {
        return "NaN".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    if v == 0.0 {
        return if v.is_sign_negative() { "-0.0" } else { "0.0" }.into();
    }
    // Rust 的 {:e} 也是最短往返：「d.ddde±x」
    let sci = format!("{v:e}");
    let (mantissa, exp) = sci.split_once('e').expect("{:e} has an exponent");
    let exp: i32 = exp.parse().expect("integer exponent");
    let (sign, mantissa) = match mantissa.strip_prefix('-') {
        Some(m) => ("-", m),
        None => ("", mantissa),
    };
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    if (-4..16).contains(&exp) {
        let n = digits.len() as i32;
        let body = if exp < 0 {
            format!("0.{}{digits}", "0".repeat((-exp - 1) as usize))
        } else if exp + 1 >= n {
            format!("{digits}{}.0", "0".repeat((exp + 1 - n) as usize))
        } else {
            let (int, frac) = digits.split_at((exp + 1) as usize);
            format!("{int}.{frac}")
        };
        format!("{sign}{body}")
    } else {
        let (first, rest) = digits.split_at(1);
        let mantissa = if rest.is_empty() {
            first.to_owned()
        } else {
            format!("{first}.{rest}")
        };
        let esign = if exp < 0 { '-' } else { '+' };
        format!("{sign}{mantissa}e{esign}{:02}", exp.abs())
    }
}

/// `json.dumps` 的字符串：`ensure_ascii`，非 ASCII 写成 UTF-16 的 `\uXXXX`。
pub fn py_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_print_like_python_repr() {
        for (v, want) in [
            (0.8119581189918389, "0.8119581189918389"),
            (68.85154855251312, "68.85154855251312"),
            (1.0, "1.0"),
            (1200.0, "1200.0"),
            (1e-05, "1e-05"),
            (0.0001, "0.0001"),
            (1.5e-7, "1.5e-07"),
            (1e16, "1e+16"),
            (123456789012345.6, "123456789012345.6"),
            (-2.5, "-2.5"),
            (0.0, "0.0"),
        ] {
            assert_eq!(py_float(v), want, "{v}");
        }
    }

    #[test]
    fn strings_escape_like_json_dumps() {
        assert_eq!(py_str("a\"b\\c"), r#""a\"b\\c""#);
        assert_eq!(py_str("录音.wav"), r#""\u5f55\u97f3.wav""#);
        assert_eq!(py_str("\u{7f}"), r#""\u007f""#);
    }
}
