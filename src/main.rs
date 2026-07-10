// src/main.rs
use anyhow::Result;
use std::path::Path;
mod agent;
mod config;
mod debug;
mod diff;
mod llm;
mod patch;
mod repl;
mod session;
mod spinner;
mod task;
mod tools;
use config::AppConfig;
fn print_help() {
    eprintln!("pcode — vim-modal patch REPL\n");
    eprintln!("Usage:");
    eprintln!("  pl                       Start REPL with default config");
    eprintln!("  pl --todo <todo.md>      Start REPL and auto-submit todo task");
    eprintln!("  pl --fastpatch [file]    Apply patches from file locally using fuzzy match");
    eprintln!("  pl --pb                  Apply patches from clipboard locally using fuzzy match");
    eprintln!("  pl --fzf                 Select patch file (todo.md/temp.md/impl.md) via fzf");
    eprintln!("  pl --patch               Print and copy the aider patch format to clipboard");
    eprintln!("  pl <file>                open file for view");
    eprintln!("  pl -q                    Quick switch via mswitch binary");
    eprintln!("  pl -s                    Run 'cli sync' and exit");
    eprintln!("  pl --help                Show this help message");
}

#[cfg(target_os = "macos")]
fn read_clipboard() -> Result<String, String> {
    use std::process::Command;
    let output = Command::new("pbpaste")
        .output()
        .map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(target_os = "linux")]
fn read_clipboard() -> Result<String, String> {
    use std::process::Command;
    let output = Command::new("xclip")
        .args(["-selection", "clipboard", "-o"])
        .output()
        .or_else(|_| {
            Command::new("xsel")
                .args(["--clipboard", "--output"])
                .output()
        })
        .map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn read_clipboard() -> Result<String, String> {
    Err("Clipboard reading is only supported on macOS and Linux".to_string())
}

#[cfg(target_os = "macos")]
fn write_clipboard(content: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::Command;
    let mut child = Command::new("pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(content.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    child.wait().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn write_clipboard(content: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::Command;
    let mut child = Command::new("xclip")
        .args(["-selection", "clipboard"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .or_else(|_| {
            Command::new("xsel")
                .args(["--clipboard", "--input"])
                .stdin(std::process::Stdio::piped())
                .spawn()
        })
        .map_err(|e| e.to_string())?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(content.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    child.wait().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn write_clipboard(_content: &str) -> Result<(), String> {
    Err("Clipboard writing is only supported on macOS and Linux".to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut config_path = None;
    let mut initial_prompt = None;
    let mut fastpatch_target = None;
    let mut use_clipboard_patch = false;
    let mut args = std::env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-q" | "--quickswitch" => {
                let extra_args: Vec<String> = args.collect();
                let mut cmd = std::process::Command::new("mswitch");
                for a in extra_args {
                    cmd.arg(a);
                }
                match cmd.status() {
                    Ok(status) => {
                        if !status.success() {
                            eprintln!("mswitch exited with code: {:?}", status.code());
                        }
                    }
                    Err(e) => eprintln!("Failed to run mswitch: {}", e),
                }
                std::process::exit(0);
            }
            "-s" | "--sync" => {
                let extra_args: Vec<String> = args.collect();
                let mut cmd = std::process::Command::new("cli");
                cmd.arg("sync");
                for a in extra_args {
                    cmd.arg(a);
                }
                match cmd.status() {
                    Ok(status) => {
                        if !status.success() {
                            eprintln!("cli sync exited with code: {:?}", status.code());
                        }
                    }
                    Err(e) => eprintln!("Failed to run cli sync: {}", e),
                }
                std::process::exit(0);
            }
            "--help" | "-h" | "help" => {
                print_help();
                std::process::exit(0);
            }
            "-c" | "--config" => {
                if let Some(p) = args.next() {
                    config_path = Some(Path::new(&p).to_path_buf());
                } else {
                    eprintln!("Error: Missing argument for -c");
                    std::process::exit(1);
                }
            }
            "--fastpatch" => {
                fastpatch_target = Some(args.next().unwrap_or_else(|| "todo.md".to_string()));
            }
            "--pb" => {
                use_clipboard_patch = true;
            }
            "--patch" => {
                let template = r#"Please apply changes using this aider style format all changed in single code block
```
// src/filename1.rs
<<<<<<< SEARCH
[exact original lines (include enough context to be unique, avoid too thin blocks)]
=======
[modified lines]
>>>>>>> REPLACE
 // src/filename2.rs
<<<<<<< SEARCH
[exact original lines (include enough context to be unique, avoid too thin blocks)]
=======
[modified lines]
>>>>>>> REPLACE
```"#;
                println!("{}", template);
                match write_clipboard(template) {
                    Ok(_) => println!("📋 Copied to clipboard!"),
                    Err(e) => eprintln!("\n❌ Clipboard Error: {}", e),
                }
                std::process::exit(0);
            }
            "--fzf" => {
                use std::io::Write;
                use std::process::{Command, Stdio};
                let choices = "todo.md\ntemp.md\nimpl.md";
                let mut cmd = Command::new("fzf");
                cmd.stdin(Stdio::piped()).stdout(Stdio::piped());
                let mut child = match cmd.spawn() {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("❌ Failed to run fzf: {}. Is it installed?", e);
                        std::process::exit(1);
                    }
                };
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(choices.as_bytes());
                }
                let output = match child.wait_with_output() {
                    Ok(o) => o,
                    Err(e) => {
                        eprintln!("❌ fzf failed: {}", e);
                        std::process::exit(1);
                    }
                };
                if !output.status.success() {
                    eprintln!("Canceled.");
                    std::process::exit(0);
                }
                let selected = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if selected.is_empty() {
                    eprintln!("Canceled.");
                    std::process::exit(0);
                }
                fastpatch_target = Some(selected);
            }
            "--todo" => {
                if let Some(p) = args.next() {
                    initial_prompt = Some(format!("Do the todo: {}", p));
                } else {
                    eprintln!("Error: Missing argument for --todo");
                    std::process::exit(1);
                }
            }
            _ => {
                let p = Path::new(&arg);
                if p.extension().and_then(|e| e.to_str()) == Some("toml") {
                    config_path = Some(p.to_path_buf());
                } else {
                    initial_prompt = Some(format!(":open {}", arg));
                }
            }
        }
    }
    let mut config = config::load_config(config_path.as_deref())?;
    if config.tools.project_root.is_empty() {
        config.tools.project_root = std::env::current_dir()?.to_string_lossy().to_string();
    }

    if let Some(target) = fastpatch_target {
        config::ensure_dirs(&config);
        match patch::run_fastpatch(&target, &config) {
            Ok(report) => {
                println!("{}", report);
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("❌ FastPatch Error: {}", e);
                std::process::exit(1);
            }
        }
    }
    config::ensure_dirs(&config);
    debug::set_debug(config.debug.enabled);
    let client = llm::LLMClient::new(
        &config.server.base_url,
        &config.server.model,
        config.server.timeout,
        config.server.api_key.clone(),
        config.server.num_ctx,
        config.server.api_type.clone(),
    );
    let bin_path = config.tools.codex_eyes_binary.clone();
    let agent = agent::PatchAgent::new(client, bin_path, config.clone());
    let mut app = repl::Repl::new(agent, config);

    if use_clipboard_patch {
        let content = match read_clipboard() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("❌ Clipboard Error: {}", e);
                std::process::exit(1);
            }
        };
        let hunks = patch::parse_patches(&content);
        if !hunks.is_empty() {
            app.start_merge(hunks);
        } else {
            eprintln!("❌ No patches found in clipboard.");
        }
    }

    match app.run(initial_prompt).await {
        Err(e) if e.to_string() == "__QUIT__" => {}
        Err(e) => eprintln!("Error: {}", e),
        Ok(_) => {}
    }
    Ok(())
}
