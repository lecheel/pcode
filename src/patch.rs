use std::path::{Path, PathBuf};

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

            // 1. Try exact match first
            if let Some(idx) = new_content.find(&search_str) {
                new_content.replace_range(idx..idx + search_str.len(), &replace_str);
                count += 1;
            } else {
                // 2. Fallback: Normalized line matching (ignoring whitespace)
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

                    // Calculate indentation difference to fix REPLACE block
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

                    // If the file is more indented than the search block, we need to prefix the replace block
                    let indent_prefix = if actual_indent > search_indent {
                        Some(&actual_first_line[..actual_indent - search_indent])
                    } else {
                        None
                    };

                    let mut result_lines: Vec<String> = file_lines[..start_line]
                        .iter()
                        .map(|s| s.to_string())
                        .collect();

                    // Apply indentation fix to the replace block
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
                    // Provide closest match context for easier debugging
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
