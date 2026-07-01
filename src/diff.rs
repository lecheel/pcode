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
) -> Vec<(RowKind, Option<String>, Option<String>)> {
    lcs_diff(search, replace)
}
fn lcs_diff(left: &[String], right: &[String]) -> Vec<(RowKind, Option<String>, Option<String>)> {
    let m = left.len();
    let n = right.len();
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for i in 1..=m {
        for j in 1..=n {
            if left[i - 1] == right[j - 1] {
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
        if i > 0 && j > 0 && left[i - 1] == right[j - 1] {
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
pub fn find_best_match(search: &[String], file: &[String]) -> MatchResult {
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
    let valuable_search_count = search.iter().filter(|l| is_valuable_line(l)).count();

    if search_len > file.len() {
        let raw = lcs_diff(search, file);
        let score = if valuable_search_count > 0 {
            let mut matched_valuable = 0;
            for (kind, left, _) in &raw {
                if *kind == RowKind::Equal {
                    if let Some(ref l) = left {
                        if is_valuable_line(l) {
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
    let min_window = search_len.saturating_sub(2).max(1);
    let max_window = (search_len + 3).min(file.len());
    let mut best_score = -1.0_f32;
    let mut best_start = 0;
    let mut best_end = 0;
    let mut best_raw: Vec<(RowKind, Option<String>, Option<String>)> = Vec::new();

    let mut all_candidates: Vec<(usize, usize, f32)> = Vec::new();
    let required_lines: Vec<&String> = search
        .iter()
        .filter(|l| !l.trim().is_empty())
        .take(2)
        .collect();

    const BOUNDARY_ANCHOR: usize = 3;
    let s_head = first_n_nonempty(search, BOUNDARY_ANCHOR);
    let s_tail = last_n_nonempty(search, BOUNDARY_ANCHOR);
    let k_head = s_head.len();
    let k_tail = s_tail.len();

    for window_size in min_window..=max_window {
        for start in 0..=file.len().saturating_sub(window_size) {
            let window = &file[start..start + window_size];
            let mut all_present = true;
            for req in &required_lines {
                if !window.iter().any(|l| l == *req) {
                    all_present = false;
                    break;
                }
            }
            if !all_present {
                continue;
            }

            let w_head = first_n_nonempty(window, k_head);
            let w_tail = last_n_nonempty(window, k_tail);
            let boundary_match =
                !s_head.is_empty() && !s_tail.is_empty() && w_head == s_head && w_tail == s_tail;

            if !boundary_match {
                continue;
            }
            let raw = lcs_diff(search, window);

            let score = if valuable_search_count > 0 {
                let mut matched_valuable = 0;
                for (kind, left, _) in &raw {
                    if *kind == RowKind::Equal {
                        if let Some(ref l) = left {
                            if is_valuable_line(l) {
                                matched_valuable += 1;
                            }
                        }
                    }
                }
                matched_valuable as f32 / valuable_search_count as f32
            } else {
                let matched = raw.iter().filter(|(k, _, _)| *k == RowKind::Equal).count();
                matched as f32 / search_len as f32
            };

            let extra = window_size.saturating_sub(search_len);
            let penalty = extra as f32 * 0.03;
            let adjusted = score - penalty;
            if adjusted > best_score {
                best_score = adjusted;
                best_start = start;
                best_end = start + window_size;
                best_raw = raw;
            }
            let pct = (adjusted * 100.0).clamp(0.0, 100.0);
            if pct >= 15.0 {
                all_candidates.push((start, start + window_size, pct));
            }
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
    let raw = lcs_diff(search, window);
    let valuable_search_count = search.iter().filter(|l| is_valuable_line(l)).count();
    let first_non_empty = search.iter().find(|l| !l.trim().is_empty());
    let last_non_empty = search.iter().rev().find(|l| !l.trim().is_empty());
    let win_first = window.iter().find(|l| !l.trim().is_empty());
    let win_last = window.iter().rev().find(|l| !l.trim().is_empty());
    let boundary_match = match (first_non_empty, win_first, last_non_empty, win_last) {
        (Some(s_first), Some(w_first), Some(s_last), Some(w_last)) => {
            s_first.trim() == w_first.trim() && s_last.trim() == w_last.trim()
        }
        _ => false,
    };
    let score = if !boundary_match {
        0.0
    } else if valuable_search_count > 0 {
        let mut matched_valuable = 0;
        for (kind, left, _) in &raw {
            if *kind == RowKind::Equal {
                if let Some(ref l) = left {
                    if is_valuable_line(l) {
                        matched_valuable += 1;
                    }
                }
            }
        }
        (matched_valuable as f32 / valuable_search_count as f32) * 100.0
    } else {
        let matched = raw.iter().filter(|(k, _, _)| *k == RowKind::Equal).count();
        (matched as f32 / search.len().max(1) as f32) * 100.0
    };

    let rows = build_rows(&raw, 1, file_start + 1);
    MatchResult {
        score,
        file_start,
        file_end: end,
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

fn first_n_nonempty(lines: &[String], n: usize) -> Vec<String> {
    lines
        .iter()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .take(n)
        .collect()
}

fn last_n_nonempty(lines: &[String], n: usize) -> Vec<String> {
    let mut v: Vec<String> = lines
        .iter()
        .rev()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .take(n)
        .collect();
    v.reverse();
    v
}
