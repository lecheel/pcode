// src/patch.rs
use regex::Regex;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};

fn compute_hash<T: Hash>(t: &T) -> String {
    let mut hasher = DefaultHasher::new();
    t.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[derive(Debug, Clone)]
pub struct PatchHunk {
    pub filename: String,
    pub search: Vec<String>,
    pub replace: Vec<String>,
}

pub fn parse_patches(content: &str) -> Vec<PatchHunk> {
    let trimmed = content.trim();
    if trimmed.contains("// === SKELETON MODE") || trimmed.contains("//--+ file:///") {
        return parse_skeleton_patches(content);
    }
    if trimmed.contains("<patch>") || trimmed.contains("<<<<<<< SEARCH") {
        return parse_aider_patches(content);
    }
    if trimmed.contains("diff --git") || trimmed.contains("--- ") || trimmed.contains("+++ ") {
        let hunks = parse_git_or_unified_patches(content);
        if !hunks.is_empty() {
            return hunks;
        }
    }
    parse_raw_paste(content)
}

fn parse_skeleton_patches(content: &str) -> Vec<PatchHunk> {
    let mut hunks = Vec::new();
    let mut current_filename = String::new();
    let mut current_lines: Vec<String> = Vec::new();

    for line in content.lines() {
        if line.starts_with("//--+ file:///") {
            if !current_filename.is_empty() {
                hunks.push(PatchHunk {
                    filename: current_filename.clone(),
                    search: current_lines.clone(),
                    replace: Vec::new(),
                });
                current_lines.clear();
            }
            if let Some(rest) = line.strip_prefix("//--+ file:///") {
                let filename = if let Some(bracket_pos) = rest.find('[') {
                    rest[..bracket_pos].trim()
                } else {
                    rest.trim()
                };
                current_filename = filename.to_string();
            }
        } else if line.trim() == "// === SKELETON MODE (COMPRESSED) ===" {
            continue;
        } else if !current_filename.is_empty() {
            current_lines.push(line.to_string());
        }
    }
    if !current_filename.is_empty() {
        hunks.push(PatchHunk {
            filename: current_filename,
            search: current_lines,
            replace: Vec::new(),
        });
    }
    hunks
}

fn parse_aider_patches(content: &str) -> Vec<PatchHunk> {
    let mut hunks = Vec::new();
    let mut current_hunk: Option<PatchHunk> = None;
    let mut state = 0;
    let mut current_filename = String::new();
    for line in content.lines() {
        if line.starts_with("<<<<<<< SEARCH") {
            current_hunk = Some(PatchHunk {
                filename: current_filename.clone(),
                search: Vec::new(),
                replace: Vec::new(),
            });
            state = 1;
        } else if line.starts_with("=======") {
            state = 2;
        } else if line.starts_with(">>>>>>> REPLACE") {
            if let Some(h) = current_hunk.take() {
                hunks.push(h);
            }
            state = 0;
        } else {
            if state == 0 {
                let trimmed = line.trim();
                let mut found_fn = None;
                if let Some(start_idx) = trimmed.find('`') {
                    if let Some(end_idx) = trimmed[start_idx + 1..].find('`') {
                        let potential = trimmed[start_idx + 1..start_idx + 1 + end_idx].trim();
                        if !potential.is_empty()
                            && (potential.contains('/')
                                || potential.contains('.')
                                || potential.ends_with(".rs"))
                        {
                            found_fn = Some(potential.to_string());
                        }
                    }
                }
                if found_fn.is_none() {
                    if let Some(rest) = trimmed.strip_prefix("// ") {
                        if !rest.is_empty()
                            && (rest.contains('/') || rest.contains('.') || rest.ends_with(".rs"))
                        {
                            found_fn = Some(rest.trim().trim_matches('`').to_string());
                        }
                    } else if let Some(rest) = trimmed.strip_prefix("# ") {
                        if !rest.is_empty()
                            && (rest.contains('/') || rest.contains('.') || rest.ends_with(".rs"))
                        {
                            found_fn = Some(rest.trim().trim_matches('`').to_string());
                        }
                    } else if let Some(rest) = trimmed.strip_prefix("filename ") {
                        found_fn = Some(rest.trim().trim_matches('`').to_string());
                    } else if let Some(rest) = trimmed.strip_prefix("filename:") {
                        found_fn = Some(rest.trim().trim_matches('`').to_string());
                    } else if let Some(rest) = trimmed.strip_prefix("file:") {
                        found_fn = Some(rest.trim().trim_matches('`').to_string());
                    } else if let Some(rest) = trimmed.strip_prefix("+++ b/") {
                        found_fn = Some(rest.trim().trim_matches('`').to_string());
                    } else if let Some(rest) = trimmed.strip_prefix("+++ ") {
                        found_fn = Some(rest.trim().trim_matches('`').to_string());
                    } else if !trimmed.is_empty()
                        && (trimmed.contains('/')
                            || trimmed.ends_with(".rs")
                            || trimmed.ends_with(".toml")
                            || trimmed.ends_with(".md"))
                    {
                        found_fn = Some(trimmed.trim_matches('`').to_string());
                    }
                }
                if let Some(fname) = found_fn {
                    current_filename = fname;
                }
            } else if state == 1 {
                if let Some(h) = current_hunk.as_mut() {
                    h.search.push(line.to_string());
                }
            } else if state == 2 {
                if let Some(h) = current_hunk.as_mut() {
                    h.replace.push(line.to_string());
                }
            }
        }
    }
    for h in &mut hunks {
        while h.search.last().map(|l| l.is_empty()).unwrap_or(false) {
            h.search.pop();
        }
        while h.replace.last().map(|l| l.is_empty()).unwrap_or(false) {
            h.replace.pop();
        }
    }
    hunks
}

fn parse_git_or_unified_patches(content: &str) -> Vec<PatchHunk> {
    let mut hunks = Vec::new();
    let mut current_hunk: Option<PatchHunk> = None;
    let mut current_filename = String::new();
    let mut state = 0;
    for line in content.lines() {
        if line.starts_with("diff --git") {
            if let Some(h) = current_hunk.take() {
                hunks.push(h);
            }
            state = 0;
        } else if line.starts_with("+++ b/") {
            current_filename = line.strip_prefix("+++ b/").unwrap().trim().to_string();
        } else if line.starts_with("+++ ") {
            current_filename = line.strip_prefix("+++ ").unwrap().trim().to_string();
        } else if line.starts_with("@@") {
            if let Some(h) = current_hunk.take() {
                hunks.push(h);
            }
            current_hunk = Some(PatchHunk {
                filename: current_filename.clone(),
                search: Vec::new(),
                replace: Vec::new(),
            });
            state = 1;
        } else if line.starts_with("-") && !line.starts_with("---") {
            if state == 1 {
                if let Some(h) = current_hunk.as_mut() {
                    h.search.push(line[1..].to_string());
                }
            }
        } else if line.starts_with("+") && !line.starts_with("+++") {
            if state == 1 {
                if let Some(h) = current_hunk.as_mut() {
                    h.replace.push(line[1..].to_string());
                }
            }
        } else if line.starts_with(" ") {
            if state == 1 {
                if let Some(h) = current_hunk.as_mut() {
                    h.search.push(line[1..].to_string());
                    h.replace.push(line[1..].to_string());
                }
            }
        }
    }
    if let Some(h) = current_hunk.take() {
        hunks.push(h);
    }
    hunks
}

fn parse_raw_paste(content: &str) -> Vec<PatchHunk> {
    let lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let mut filename = String::new();
    let mut search_start = 0;
    while search_start < lines.len() {
        let first_line = lines[search_start].trim();
        if first_line.is_empty() {
            search_start += 1;
            continue;
        }
        if let Some(rest) = first_line.strip_prefix("// ") {
            if !rest.is_empty() && (rest.contains('/') || rest.ends_with(".rs")) {
                filename = rest.trim().to_string();
                search_start += 1;
                break;
            }
        } else if let Some(rest) = first_line.strip_prefix("# ") {
            if !rest.is_empty() && (rest.contains('/') || rest.ends_with(".rs")) {
                filename = rest.trim().to_string();
                search_start += 1;
                break;
            }
        } else if first_line.starts_with("filename ") {
            filename = first_line
                .strip_prefix("filename ")
                .unwrap()
                .trim()
                .to_string();
            search_start += 1;
            break;
        } else if first_line.starts_with("+++ b/") {
            filename = first_line
                .strip_prefix("+++ b/")
                .unwrap()
                .trim()
                .to_string();
            search_start += 1;
            break;
        } else if first_line.starts_with("+++ ") {
            filename = first_line.strip_prefix("+++ ").unwrap().trim().to_string();
            search_start += 1;
            break;
        }
        break;
    }
    let search_lines: Vec<String> = lines[search_start..]
        .iter()
        .filter(|l| {
            !l.starts_with("<<<<<<<") && !l.starts_with("=======") && !l.starts_with(">>>>>>>")
        })
        .cloned()
        .collect();
    if search_lines.is_empty() && filename.is_empty() {
        return Vec::new();
    }
    vec![PatchHunk {
        filename,
        search: search_lines,
        replace: Vec::new(),
    }]
}

pub fn run_fastpatch(todo_path: &str, config: &crate::config::AppConfig) -> Result<String, String> {
    let content = std::fs::read_to_string(todo_path)
        .map_err(|e| format!("Failed to read {}: {}", todo_path, e))?;
    run_clipboard_patch(&content, config)
}

pub fn run_clipboard_patch(content: &str, config: &crate::config::AppConfig) -> Result<String, String> {
    let re = Regex::new(r#"path="([^"]+)"\s+patch="([^"]+)""#).unwrap();
    let mut patched_content = content.to_string();
    for caps in re.captures_iter(&content) {
        let path = caps.get(1).unwrap().as_str();
        let patch_text = caps.get(2).unwrap().as_str().replace("\\n", "\n");
        let replacement = format!("{}\n{}", path, patch_text);
        patched_content = patched_content.replace(caps.get(0).unwrap().as_str(), &replacement);
    }

    let hunks = parse_patches(&patched_content);
    if hunks.is_empty() {
        return Ok("No patches found.".to_string());
    }

    let mut results = Vec::new();
    let project_root = PathBuf::from(&config.tools.project_root);
    let impl_dir = project_root.join(".impl");
    let _ = std::fs::create_dir_all(&impl_dir);
    let log_path = impl_dir.join("patchinfo.log");

    let mut applied_hashes = HashSet::new();
    if log_path.exists() {
        if let Ok(log_content) = std::fs::read_to_string(&log_path) {
            for line in log_content.lines() {
                let hash = line.split('|').next().unwrap_or("").trim();
                if !hash.is_empty() {
                    applied_hashes.insert(hash.to_string());
                }
            }
        }
    }
    let mut newly_applied = Vec::new();

    for hunk in hunks {
        if hunk.search.is_empty() {
            continue;
        }

        let patch_str = format!(
            "<<<<<<< SEARCH\n{}\n=======\n{}\n>>>>>>> REPLACE",
            hunk.search.join("\n"),
            hunk.replace.join("\n")
        );
        let patch_hash = compute_hash(&patch_str);

        if applied_hashes.contains(&patch_hash) {
            results.push(format!(
                "⏭️ {} skipped (already applied in patchinfo.log)",
                hunk.filename
            ));
            continue;
        }

        let path = &hunk.filename;
        let resolved = match validate_path(path, &project_root, &config.tools.allow_paths) {
            Ok(p) => p,
            Err(e) => {
                results.push(format!("❌ {}: {}", path, e));
                continue;
            }
        };

        if !resolved.exists() {
            results.push(format!("❌ {}: File does not exist", path));
            continue;
        }

        let file_content = std::fs::read_to_string(&resolved)
            .map_err(|e| format!("Failed to read {}: {}", path, e))?;
        let file_lines: Vec<String> = file_content.lines().map(String::from).collect();

        let match_result = crate::diff::find_best_match(&hunk.search, &file_lines);

        let search_match = crate::diff::find_best_match(&hunk.search, &file_lines);
        let replace_match = crate::diff::find_best_match(&hunk.replace, &file_lines);

        // Prevent duplicate patches: if the replace block is already present, skip
        if replace_match.score >= 95.0 {
            results.push(format!("⏭️ {} skipped (already patched)", path));
            continue;
        }

        if search_match.score > 90.0 {
            let mut new_lines: Vec<String> = file_lines[..search_match.file_start].to_vec();
            new_lines.extend(hunk.replace.clone());
            new_lines.extend(file_lines[search_match.file_end..].to_vec());

            let mut new_content = new_lines.join("\n");
            if file_content.ends_with('\n') {
                new_content.push('\n');
            }

            std::fs::write(&resolved, &new_content)
                .map_err(|e| format!("Failed to write {}: {}", path, e))?;

            let summary = {
                let mut s = format!("✅ {} patched (Score: {:.1}%)\n", path, search_match.score);
                let r_lines = &hunk.replace;
                if r_lines.len() <= 6 {
                    for l in r_lines {
                        s.push_str(&format!("  {}\n", l));
                    }
                } else {
                    for l in r_lines.iter().take(3) {
                        s.push_str(&format!("  {}\n", l));
                    }
                    s.push_str("  ...\n");
                    for l in r_lines.iter().skip(r_lines.len() - 3) {
                        s.push_str(&format!("  {}\n", l));
                    }
                }
                s.trim_end().to_string()
            };

            results.push(summary);

            // Mark as applied in memory
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            newly_applied.push(format!("{} | {} | {}", patch_hash, path, timestamp));
        } else {
            results.push(format!(
                "❌ {} skipped (Score: {:.1}%, required > 90.0%)",
                path, search_match.score
            ));
        }
    }

    // Append newly applied patches to patchinfo.log
    if !newly_applied.is_empty() {
        let mut log_content = String::new();
        for entry in &newly_applied {
            log_content.push_str(entry);
            log_content.push('\n');
        }
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .and_then(|mut f| f.write_all(log_content.as_bytes()))
            .map_err(|e| format!("Failed to write patchinfo.log: {}", e))?;
    }

    Ok(results.join("\n\n"))
}

fn path_contains_blocked_dir(path: &Path) -> bool {
    path.components().any(|c| {
        if let std::path::Component::Normal(os_str) = c {
            if let Some(s) = os_str.to_str() {
                return s.starts_with('.')
                    || s == "target"
                    || s == "node_modules"
                    || s == "__pycache__"
                    || s == "vendor"
                    || s == ".cargo";
            }
        }
        false
    })
}
fn validate_path(
    path: &str,
    project_root: &Path,
    allow_paths: &[String],
) -> Result<PathBuf, String> {
    let canonical_root = project_root
        .canonicalize()
        .map_err(|e| format!("Invalid project root '{}': {}", project_root.display(), e))?;
    let raw = Path::new(path);
    let target = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        canonical_root.join(path)
    };
    if path_contains_blocked_dir(&target) {
        return Err(format!(
            "🚫 Access denied: '{}' is inside a blocked directory",
            path
        ));
    }
    let canonical_target = if target.exists() {
        target
            .canonicalize()
            .map_err(|e| format!("Cannot resolve path '{}': {}", path, e))?
    } else if let Some(parent) = target.parent() {
        if parent.as_os_str().is_empty() {
            canonical_root.join(target.file_name().unwrap_or_default())
        } else if parent.exists() {
            let canonical_parent = parent
                .canonicalize()
                .map_err(|e| format!("Parent directory does not exist for '{}': {}", path, e))?;
            canonical_parent.join(target.file_name().unwrap_or_default())
        } else {
            return Err(format!("Parent directory does not exist for '{}'", path));
        }
    } else {
        return Err(format!("Invalid path: '{}'", path));
    };
    if canonical_target.starts_with(&canonical_root) {
        if path_contains_blocked_dir(&canonical_target) {
            return Err(format!(
                "🚫 Access denied: '{}' is inside a blocked directory",
                path
            ));
        }
        return Ok(canonical_target);
    }
    for allowed in allow_paths {
        let canonical_allowed = match PathBuf::from(allowed).canonicalize() {
            Ok(p) => p,
            Err(_) => continue,
        };
        if canonical_target.starts_with(&canonical_allowed) {
            if path_contains_blocked_dir(&canonical_target) {
                return Err(format!(
                    "🚫 Access denied: '{}' is inside a blocked directory",
                    path
                ));
            }
            return Ok(canonical_target);
        }
    }
    Err(format!(
        "🚫 Access denied: '{}' is outside the project root ({}) and not in allow_paths",
        path,
        canonical_root.display()
    ))
}
fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let mut dp = vec![vec![0; b_chars.len() + 1]; a_chars.len() + 1];
    for i in 0..=a_chars.len() {
        dp[i][0] = i;
    }
    for j in 0..=b_chars.len() {
        dp[0][j] = j;
    }
    for i in 1..=a_chars.len() {
        for j in 1..=b_chars.len() {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };
            dp[i][j] = (dp[i - 1][j] + 1)
                .min(dp[i][j - 1] + 1)
                .min(dp[i - 1][j - 1] + cost);
        }
    }
    dp[a_chars.len()][b_chars.len()]
}
pub fn apply_patch(
    path: &str,
    patch_text: &str,
    project_root: &Path,
    allow_paths: &[String],
) -> Result<String, String> {
    let resolved = validate_path(path, project_root, allow_paths)?;
    if !resolved.exists() {
        return Err(format!("File does not exist: {}", path));
    }
    if resolved.is_dir() {
        return Err(format!("Path is a directory, not a file: {}", path));
    }
    let content = std::fs::read_to_string(&resolved)
        .map_err(|e| format!("Failed to read {}: {}", path, e))?;
    let mut new_content = content.clone();
    let mut count = 0;
    let mut lines = patch_text.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim() == "<<<<<<< SEARCH" {
            let mut search = Vec::new();
            let mut found_separator = false;
            while let Some(l) = lines.next() {
                if l.trim() == "=======" {
                    found_separator = true;
                    break;
                }
                search.push(l);
            }
            if !found_separator {
                return Err(format!(
                    "Malformed patch: Missing '=======' separator in {}",
                    path
                ));
            }
            let mut replace = Vec::new();
            let mut found_terminator = false;
            while let Some(l) = lines.next() {
                if l.trim() == ">>>>>>> REPLACE" {
                    found_terminator = true;
                    break;
                }
                replace.push(l);
            }
            if !found_terminator {
                return Err(format!(
                    "Malformed patch: Missing '>>>>>>> REPLACE' terminator in {}",
                    path
                ));
            }
            let search_str = search.join("\n");
            let replace_str = replace.join("\n");
            if search_str.is_empty() {
                return Err("SEARCH block cannot be empty".to_string());
            }
            if let Some(idx) = new_content.find(&search_str) {
                new_content.replace_range(idx..idx + search_str.len(), &replace_str);
                count += 1;
            } else {
                let file_lines: Vec<&str> = new_content.lines().collect();
                let search_lines: Vec<&str> = search_str.lines().collect();
                if search_lines.is_empty() || search_lines.len() > file_lines.len() {
                    return Err(format!(
                        "SEARCH block not found in {}. The search block is larger than the file or empty.",
                        path
                    ));
                }
                let mut match_indices = Vec::new();
                for i in 0..=(file_lines.len() - search_lines.len()) {
                    let mut is_match = true;
                    for j in 0..search_lines.len() {
                        if file_lines[i + j].trim() != search_lines[j].trim() {
                            is_match = false;
                            break;
                        }
                    }
                    if is_match {
                        match_indices.push(i);
                    }
                }
                if match_indices.len() == 1 {
                    let start_line = match_indices[0];
                    let end_line = start_line + search_lines.len();
                    let actual_first_line = file_lines[start_line];
                    let search_first_line = search_lines[0];
                    let actual_indent = actual_first_line
                        .chars()
                        .take_while(|c| *c == ' ' || *c == '\t')
                        .count();
                    let search_indent = search_first_line
                        .chars()
                        .take_while(|c| *c == ' ' || *c == '\t')
                        .count();
                    let indent_prefix = if actual_indent > search_indent {
                        Some(&actual_first_line[..actual_indent - search_indent])
                    } else {
                        None
                    };
                    let mut result_lines: Vec<String> = file_lines[..start_line]
                        .iter()
                        .map(|s| s.to_string())
                        .collect();
                    for r_line in replace_str.lines() {
                        if let Some(prefix) = indent_prefix {
                            if !r_line.trim().is_empty() {
                                result_lines.push(format!("{}{}", prefix, r_line));
                            } else {
                                result_lines.push(r_line.to_string());
                            }
                        } else {
                            result_lines.push(r_line.to_string());
                        }
                    }
                    result_lines.extend(file_lines[end_line..].iter().map(|s| s.to_string()));
                    new_content = result_lines.join("\n");
                    if content.ends_with('\n') && !new_content.ends_with('\n') {
                        new_content.push('\n');
                    }
                    count += 1;
                } else if match_indices.is_empty() {
                    let first_search_line = search_lines.get(0).map(|s| s.trim()).unwrap_or("");
                    let mut closest_line = 0;
                    let mut min_distance = usize::MAX;
                    for (i, l) in file_lines.iter().enumerate() {
                        let dist = levenshtein_distance(l.trim(), first_search_line);
                        if dist < min_distance {
                            min_distance = dist;
                            closest_line = i;
                        }
                    }
                    return Err(format!(
                        "SEARCH block not found in {}.\nExpected:\n{}\nMake sure the text matches the code exactly.\nClosest match at line {}: {}",
                        path,
                        search_str,
                        closest_line + 1,
                        file_lines.get(closest_line).unwrap_or(&"")));
                } else {
                    return Err(format!(
                        "Ambiguous match: SEARCH block found {} times in {}. Add more surrounding context lines to make it unique.",
                        match_indices.len(), path
                    ));
                }
            }
        }
    }
    if count == 0 {
        return Err("No valid SEARCH/REPLACE blocks found in patch text".to_string());
    }
    std::fs::write(&resolved, &new_content)
        .map_err(|e| format!("Failed to write {}: {}", path, e))?;
    Ok(format!("Patched {} ({} block(s) applied)", path, count))
}