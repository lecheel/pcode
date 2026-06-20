pcode — vim-modal patch REPL
```
  i                → enter Insert mode (type message to LLM)
  :                → enter Command mode (:help, :quit, …)
  F12              → Cancel running task
  Esc              → back to Normal mode
  j/k G gg C-d C-u → scroll response buffer
  yy               → Yank line to clipboard
  dd (5dd)         → Delete line (5 lines)
  u                → Undo line deletion
  Alt-w            → Write buffer
  Alt-x            → Close buffer
  Alt-- / Alt-=    → Previous / Next buffer

[01:21:37] > what skills you have
  💭 The user is asking about what skills/tools I have available. Let me provide a comprehensive overview of my capabilities based on the tools described in the system prompt.

I have a wide range of **surgical code editing skills** organized into four phases:

---

## 🔍 DISCOVER — Understanding Code
| Skill | Tool | What it does |
|-------|------|-------------|
| **Code Skeleton** | `daemon_skeleton` | Fetches compressed project overview with AST hashes |
| **Hash Lookup** | `daemon_get_hash` | Fetches full code blocks by hash (saves context) |
| **File Info** | `daemon_file_info` | Lists all AST body hashes in a file |
| **List Files** | `codex_eyes_ls` | Directory listing (flat or recursive) |
| **Grep Search** | `codex_eyes_grep` | Recursive text search across all files |
| **AST Search** | `ast_grep_search` | Pattern-based code search (understands syntax) |
| **AST Explain** | `ast_explain` | Shows AST structure at a line or text snippet |
| **File Find** | `codex_eyes_find` / `codex_eyes_find_fuzzy` | Find files by name (exact or fuzzy) |
| **File Read** | `codex_eyes_read` | Read file contents with line numbers |

---

## ✏️ EDIT — Making Changes
| Skill | Tool | Best for |
|-------|------|----------|
| **Simple Replace** | `codex_eyes_sed` | Renaming variables, fixing typos, updating strings |
| **Structural Patch** | `apply_patch` | Multi-line edits, refactoring, changing function signatures |
| **Create/Overwrite** | `codex_eyes_createFile` | New files, complete file rewrites, creating directories |
| **Undo** | `codex_eyes_undo` | Revert a file to last git commit |

---

## ✅ VERIFY — Checking Work
| Skill | Tool | What it does |
|-------|------|-------------|
| **Compile Check** | `codex_eyes_cargo_check` | Run `cargo check` to verify compilation |
| **Git Diff** | `codex_eyes_gitdiff` | See what changed |
| **Run Tests** | `codex_eyes_run` | Run `cargo test`, `cargo clippy`, `cargo fmt`, etc. |

---

## 📋 TASK MANAGEMENT
| Skill | Tool | What it does |
|-------|------|-------------|
| **Create Task** | `task_create` | Start a new implementation task with a plan |
| **Log Step** | `task_step` | Record each modification with success/failure |
| **Complete Task** | `task_complete` | Finalize and write change log |
| **List/Abort** | `task_list` / `task_abort` | Manage tasks |

---

**In short**: I can surgically edit Rust (and other language) code with AST-level precision, never breaking the structure by accident. I work best when you tell me what you want to build, fix, or refactor! 🛠️
```