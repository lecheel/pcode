#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Equal,
    Delete,
    Insert,
}
#[derive(Debug, Clone)]
pub struct DiffRow {
    pub kind: RowKind,
    pub left: Option<String>,
    pub right: Option<String>,
    pub left_num: Option<usize>,
    pub right_num: Option<usize>,
}
#[derive(Debug, Clone)]
pub struct MatchResult {
    pub score: f32,
    pub file_start: usize,
    pub file_end: usize,
    pub rows: Vec<DiffRow>,
    pub candidates: Vec<(usize, usize, f32)>,
}
pub fn is_valuable_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.len() == 1 {
        let c = trimmed.chars().next().unwrap();
        if c == '}'
            || c == '{'
            || c == ']'
            || c == '['
            || c == ')'
            || c == '('
            || c == ','
            || c == ';'
            || c == '.'
        {
            return false;
        }
    }
    true
}
pub fn diff_patch(
    search: &[String],
    replace: &[String],
    ignore_comments: bool,
) -> Vec<(RowKind, Option<String>, Option<String>)> {
    lcs_diff(search, replace, ignore_comments)
}
fn is_comment_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("//")
        || trimmed.starts_with("#")
        || trimmed.starts_with("--")
        || trimmed.starts_with("/*")
        || trimmed.starts_with("*")
        || trimmed.starts_with("<!--")
}

fn lcs_diff(
    left: &[String],
    right: &[String],
    ignore_comments: bool,
) -> Vec<(RowKind, Option<String>, Option<String>)> {
    let m = left.len();
    let n = right.len();
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for i in 1..=m {
        for j in 1..=n {
            // Use trim_end to ignore trailing \r (CRLF vs LF mismatches)
            let equal = left[i - 1].trim_end() == right[j - 1].trim_end()
                || (ignore_comments
                    && is_comment_line(&left[i - 1])
                    && is_comment_line(&right[j - 1]));
            if equal {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }
    let mut result = Vec::new();
    let mut i = m;
    let mut j = n;
    while i > 0 || j > 0 {
        // Use trim_end to ignore trailing \r (CRLF vs LF mismatches)
        let equal = i > 0
            && j > 0
            && (left[i - 1].trim_end() == right[j - 1].trim_end()
                || (ignore_comments
                    && is_comment_line(&left[i - 1])
                    && is_comment_line(&right[j - 1])));
        if equal {
            result.push((
                RowKind::Equal,
                Some(left[i - 1].clone()),
                Some(right[j - 1].clone()),
            ));
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || dp[i][j - 1] >= dp[i - 1][j]) {
            result.push((RowKind::Insert, None, Some(right[j - 1].clone())));
            j -= 1;
        } else {
            result.push((RowKind::Delete, Some(left[i - 1].clone()), None));
            i -= 1;
        }
    }
    result.reverse();
    result
}
pub fn find_best_match(search: &[String], file: &[String], ignore_comments: bool) -> MatchResult {
    if search.is_empty() || file.is_empty() {
        return MatchResult {
            score: 0.0,
            file_start: 0,
            file_end: 0,
            rows: vec![],
            candidates: vec![],
        };
    }
    let search_len = search.len();
    let valuable_search_count = search
        .iter()
        .filter(|l| is_valuable_line(l) && (!ignore_comments || !is_comment_line(l)))
        .count();
    if search_len > file.len() {
        let raw = lcs_diff(search, file, ignore_comments);
        let score = if valuable_search_count > 0 {
            let mut matched_valuable = 0;
            for (kind, left, _) in &raw {
                if *kind == RowKind::Equal {
                    if let Some(ref l) = left {
                        if is_valuable_line(l) && (!ignore_comments || !is_comment_line(l)) {
                            matched_valuable += 1;
                        }
                    }
                }
            }
            (matched_valuable as f32 / valuable_search_count as f32) * 100.0
        } else {
            let matched = raw.iter().filter(|(k, _, _)| *k == RowKind::Equal).count();
            (matched as f32 / search_len as f32) * 100.0
        };
        let rows = build_rows(&raw, 1, 1);
        return MatchResult {
            score,
            file_start: 0,
            file_end: file.len(),
            rows,
            candidates: vec![(0, file.len(), score)],
        };
    }

    // Restrict window size to search_len - 5 up to search_len + 6
    let min_window = search_len.saturating_sub(5).max(1);
    let max_window = (search_len + 6).min(file.len());

    let mut best_score = -1.0_f32;
    let mut best_matched_count = 0;
    let mut best_start = 0;
    let mut best_end = 0;
    let mut best_raw = Vec::new();
    let mut all_candidates: Vec<(usize, usize, f32)> = Vec::new();
    let required_lines: Vec<&String> = search
        .iter()
        .filter(|l| !l.trim().is_empty() && (!ignore_comments || !is_comment_line(l)))
        .take(2)
        .collect();

    // Use a +3 boundary anchor (check first 3 and last 3 non-empty lines)
    const BOUNDARY_ANCHOR: usize = 3;
    let s_head = first_n_nonempty(search, BOUNDARY_ANCHOR, true);
    let s_tail = last_n_nonempty(search, BOUNDARY_ANCHOR, true);
    let k_head = s_head.len();
    let k_tail = s_tail.len();

    let s_first = s_head.first().cloned();
    let s_last = s_tail.last().cloned();

    let mut candidate_windows: Vec<(usize, usize)> = Vec::new();

    if let (Some(first), Some(last)) = (s_first, s_last) {
        let first_matches: Vec<usize> = file
            .iter()
            .enumerate()
            .filter_map(|(i, l)| {
                if l.trim() == first.trim() {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();
        let last_matches: Vec<usize> = file
            .iter()
            .enumerate()
            .filter_map(|(i, l)| {
                if l.trim() == last.trim() {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();

        for &start in &first_matches {
            for &end in &last_matches {
                let window_size = end - start + 1;
                if start <= end && window_size >= min_window && window_size <= max_window {
                    candidate_windows.push((start, end + 1));
                }
            }
        }
    } else {
        // Fallback for trivial searches (e.g. all comments or empty)
        for window_size in min_window..=max_window {
            for start in 0..=file.len().saturating_sub(window_size) {
                candidate_windows.push((start, start + window_size));
            }
        }
    }

    for (start, end) in candidate_windows {
        let window = &file[start..end];

        let mut all_present = true;
        for &req in &required_lines {
            if !window.iter().any(|l| l.trim() == req.trim()) {
                all_present = false;
                break;
            }
        }
        if !all_present {
            continue;
        }

        let w_head = first_n_nonempty(window, k_head, true);
        let w_tail = last_n_nonempty(window, k_tail, true);
        let boundary_match =
            !s_head.is_empty() && !s_tail.is_empty() && w_head == s_head && w_tail == s_tail;

        if !boundary_match {
            continue;
        }

        let raw = lcs_diff(search, window, ignore_comments);
        let matched_count = raw.iter().filter(|(k, _, _)| *k == RowKind::Equal).count();
        let score = (matched_count as f32 / search_len as f32) * 100.0;

        // Track the file index of the last matched line to avoid deleting
        // trailing unmatched lines (e.g. the next function's end bracket)
        let mut last_equal_file_end = start;
        let mut file_idx = start;
        for (kind, _, _) in &raw {
            match kind {
                RowKind::Equal => {
                    last_equal_file_end = file_idx + 1;
                    file_idx += 1;
                }
                RowKind::Insert => {
                    file_idx += 1;
                }
                _ => {}
            }
        }
        let actual_end = if last_equal_file_end > start {
            last_equal_file_end
        } else {
            end
        };

        let matched_count = raw.iter().filter(|(k, _, _)| *k == RowKind::Equal).count();

        // Tie-breaker: if scores are equal, prefer the window that matches
        // more lines (closer to the full search_len) to avoid missing
        // ignored lines like "}"
        let is_better = if score > best_score {
            true
        } else if (score - best_score).abs() < 0.1 {
            matched_count > best_matched_count
        } else {
            false
        };

        if is_better {
            best_score = score;
            best_matched_count = matched_count;
            best_start = start;
            best_end = actual_end;
            best_raw = raw;
            all_candidates.push((start, actual_end, score));
        }
    }
    let rows = build_rows(&best_raw, 1, best_start + 1);
    let score = (best_score * 100.0).clamp(0.0, 100.0);
    all_candidates.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    let mut candidates: Vec<(usize, usize, f32)> = Vec::new();
    for (s, e, sc) in all_candidates {
        let overlaps = candidates.iter().any(|(rs, re, _)| s < *re && e > *rs);
        if !overlaps {
            candidates.push((s, e, sc));
        }
    }
    candidates.truncate(20);
    MatchResult {
        score,
        file_start: best_start,
        file_end: best_end,
        rows,
        candidates,
    }
}
pub fn compute_match_for_window(
    search: &[String],
    file: &[String],
    file_start: usize,
    file_end: usize,
    ignore_comments: bool,
) -> MatchResult {
    if search.is_empty() || file.is_empty() || file_start >= file.len() {
        return MatchResult {
            score: 0.0,
            file_start: 0,
            file_end: 0,
            rows: vec![],
            candidates: vec![],
        };
    }
    let end = file_end.min(file.len());
    let window = &file[file_start..end];
    let raw = lcs_diff(search, window, ignore_comments);
    let valuable_search_count = search
        .iter()
        .filter(|l| is_valuable_line(l) && (!ignore_comments || !is_comment_line(l)))
        .count();
    let first_non_empty = search
        .iter()
        .find(|l| !l.trim().is_empty() && (!ignore_comments || !is_comment_line(l)));
    let last_non_empty = search
        .iter()
        .rev()
        .find(|l| !l.trim().is_empty() && (!ignore_comments || !is_comment_line(l)));
    let win_first = window
        .iter()
        .find(|l| !l.trim().is_empty() && (!ignore_comments || !is_comment_line(l)));
    let win_last = window
        .iter()
        .rev()
        .find(|l| !l.trim().is_empty() && (!ignore_comments || !is_comment_line(l)));

    let boundary_match = match (first_non_empty, win_first, last_non_empty, win_last) {
        (Some(s_first), Some(w_first), Some(s_last), Some(w_last)) => {
            s_first.trim() == w_first.trim() && s_last.trim() == w_last.trim()
        }
        _ => false,
    };
    let score = if !boundary_match {
        0.0
    } else {
        let matched = raw.iter().filter(|(k, _, _)| *k == RowKind::Equal).count();
        (matched as f32 / search.len().max(1) as f32) * 100.0
    };

    // Track the file index of the last matched line to avoid deleting
    // trailing unmatched lines (e.g. the next function's end bracket)
    let mut last_equal_file_end = file_start;
    let mut file_idx = file_start;
    for (kind, _, _) in &raw {
        match kind {
            RowKind::Equal => {
                last_equal_file_end = file_idx + 1;
                file_idx += 1;
            }
            RowKind::Insert => {
                file_idx += 1;
            }
            _ => {}
        }
    }
    let actual_end = if last_equal_file_end > file_start {
        last_equal_file_end
    } else {
        end
    };

    let rows = build_rows(&raw, 1, file_start + 1);
    MatchResult {
        score,
        file_start,
        file_end: actual_end,
        rows,
        candidates: vec![],
    }
}
pub fn build_rows(
    raw: &[(RowKind, Option<String>, Option<String>)],
    left_start: usize,
    right_start: usize,
) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    let mut ln = left_start;
    let mut rn = right_start;
    for (kind, left, right) in raw {
        let left_num = if left.is_some() {
            let n = Some(ln);
            ln += 1;
            n
        } else {
            None
        };
        let right_num = if right.is_some() {
            let n = Some(rn);
            rn += 1;
            n
        } else {
            None
        };
        rows.push(DiffRow {
            kind: *kind,
            left: left.clone(),
            right: right.clone(),
            left_num,
            right_num,
        });
    }
    rows
}

fn first_n_nonempty(lines: &[String], n: usize, ignore_comments: bool) -> Vec<String> {
    lines
        .iter()
        .filter(|l| {
            let trimmed = l.trim();
            !trimmed.is_empty() && (!ignore_comments || !is_comment_line(l))
        })
        .map(|l| l.trim().to_string())
        .take(n)
        .collect()
}
fn last_n_nonempty(lines: &[String], n: usize, ignore_comments: bool) -> Vec<String> {
    let mut v: Vec<String> = lines
        .iter()
        .rev()
        .filter(|l| {
            let trimmed = l.trim();
            !trimmed.is_empty() && (!ignore_comments || !is_comment_line(l))
        })
        .map(|l| l.trim().to_string())
        .take(n)
        .collect();
    v.reverse();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(String::from).collect()
    }

    #[test]
    fn test_swallow_next_func_bracket() {
        let search = lines(
            r#"fn process() {
    let x = 1;
    let y = 2;
    let z = x + y;
    println!("{}", z);
}"#,
        );
        let file = lines(
            r#"fn process() {
    let x = 1;
    let y = 2;
    let z = x + y;
    println!("{}", z);
}

fn cleanup() {
    // ...
}"#,
        );
        let result = find_best_match(&search, &file, false);
        println!(
            "test_swallow_next_func_bracket: start={}, end={}",
            result.file_start, result.file_end
        );
        assert_eq!(
            result.file_end, 6,
            "Should end at the first '}}', not swallow the next function"
        );
    }

    #[test]
    fn test_swallow_extra_brackets() {
        let search = lines(
            r#"    {
        let a = 1;
    }"#,
        );
        let file = lines(
            r#"    {
        let a = 1;
    }
}

fn another() {
    {
        let b = 2;
    }"#,
        );
        let result = find_best_match(&search, &file, false);
        println!(
            "test_swallow_extra_brackets: start={}, end={}",
            result.file_start, result.file_end
        );
        assert_eq!(
            result.file_end, 3,
            "Should end at the first matching '}}', not swallow extra brackets"
        );
    }

    #[test]
    fn test_swallow_with_missing_line() {
        let search = lines(
            r#"fn func() {
    let a = 1;
    let c = 3;
}"#,
        );
        let file = lines(
            r#"fn func() {
    let a = 1;
    let b = 2;
    let c = 3;
}

fn extra() {
}"#,
        );
        let result = find_best_match(&search, &file, false);
        println!(
            "test_swallow_with_missing_line: start={}, end={}",
            result.file_start, result.file_end
        );
        assert_eq!(
            result.file_end, 5,
            "Should end at line 5, not swallow the next function"
        );
    }
}