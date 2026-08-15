use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchResult {
    pub kind: String,
    pub title: String,
    pub subtitle: String,
    pub icon: Option<String>,
    pub score: i64,
    pub data: String,
}

pub fn calc_result(q: &str) -> Option<String> {
    let q = q.trim();
    if q.len() < 3 {
        return None;
    }
    if !q.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    if !q.chars().any(|c| "+-*/^%".contains(c)) {
        return None;
    }
    if !q
        .chars()
        .all(|c| c.is_ascii_digit() || "+-*/^%()., ".contains(c))
    {
        return None;
    }
    let normalized = floatify(&q.replace(',', "."));
    match evalexpr::eval(&normalized) {
        Ok(evalexpr::Value::Float(f)) => Some(format_float(f)),
        Ok(evalexpr::Value::Int(i)) => Some(i.to_string()),
        _ => None,
    }
}

fn floatify(expr: &str) -> String {
    let chars: Vec<char> = expr.chars().collect();
    let mut out = String::with_capacity(expr.len() + 8);
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            out.extend(&chars[start..i]);
            let prev_dot = start > 0 && chars[start - 1] == '.';
            let next_dot = chars.get(i) == Some(&'.');
            if !prev_dot && !next_dot {
                out.push_str(".0");
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn format_float(f: f64) -> String {
    if f.fract() == 0.0 && f.abs() < 1e15 {
        return format!("{}", f as i64);
    }
    let s = format!("{:.10}", f);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

pub fn one_line(text: &str) -> String {
    let first = text.lines().next().unwrap_or("");
    let mut s: String = first.chars().take(100).collect();
    if first.chars().count() > 100 || text.lines().count() > 1 {
        s.push('…');
    }
    s
}

pub fn rel_time(ts_ms: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let secs = now.saturating_sub(ts_ms) / 1000;
    if secs < 60 {
        "agora mesmo".into()
    } else if secs < 3600 {
        format!("há {} min", secs / 60)
    } else if secs < 86_400 {
        format!("há {} h", secs / 3600)
    } else {
        format!("há {} d", secs / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calc_basic() {
        assert_eq!(calc_result("2+2").as_deref(), Some("4"));
        assert_eq!(calc_result("2+2*3").as_deref(), Some("8"));
        assert_eq!(calc_result("10/4").as_deref(), Some("2.5"));
        assert_eq!(calc_result("2^10").as_deref(), Some("1024"));
        assert_eq!(calc_result("10,5*2").as_deref(), Some("21"));
        assert_eq!(calc_result("(2+3)*4").as_deref(), Some("20"));
    }

    #[test]
    fn calc_rejects_non_math() {
        assert_eq!(calc_result("firefox"), None);
        assert_eq!(calc_result("42"), None);
        assert_eq!(calc_result("ab+cd"), None);
    }

    #[test]
    fn one_line_truncates() {
        assert_eq!(one_line("ola"), "ola");
        assert_eq!(one_line("a\nb"), "a…");
    }
}
