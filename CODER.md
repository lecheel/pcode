# CODER.md - Local Patch Code Workflow

## STRICT PATCH WORKFLOW
When modifying code, you MUST follow this exact workflow.

### 1. PREPARE
- Use `codex_eyes_read` to view the EXACT content of the file/section you are editing.
- NEVER guess the existing code structure or whitespace.

### 2. EDIT (Choose the right tool)
**Rule A: Structural or Multi-line Edits → Use `apply_patch`**
- Format MUST be:
  filename src/foo.rs
  <<<<<<< SEARCH
  [exact existing code]
  =======
  [new code]
  >>>>>>> REPLACE
- The SEARCH block MUST match the file exactly, including leading spaces/tabs.
- Include enough context lines to make the match unique.

**Rule B: Simple Single-line Replacements → Use `codex_eyes_sed`**
- Renaming variables, fixing typos, updating string literals.

### 3. RECOVER (If `apply_patch` fails)
- If a patch fails, DO NOT guess why.
- Immediately use `codex_eyes_read` on the target file.
- Compare your SEARCH block with the actual file content.
- Fix the whitespace/content in your SEARCH block and retry.

### 4. VERIFY
- After patching, ALWAYS use `codex_eyes_gitdiff` to see what changed.
- Run `codex_eyes_cargo_check` to ensure the code compiles.
- If compilation fails, read the error, apply a new patch, and verify again.

## CRITICAL RULES
- NEVER output line numbers in your patches.
- ALWAYS ensure the final code is syntactically correct.
- Use `task_step` to record every successful patch.
