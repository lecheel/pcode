use anyhow::Result;
use std::path::Path;
mod agent;
mod config;
mod debug;
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
    eprintln!("  pl                Start REPL with default config");
    eprintln!("  pl <config.toml>  Start REPL with custom config");
    eprintln!("  pl <todo.md>      Start REPL and auto-submit todo task");
    eprintln!("  pl -q             Quick switch via mswitch binary");
    eprintln!("  pl -s             Run 'cli sync' and exit");
    eprintln!("  pl --help         Show this help message");
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut config_path = None;
    let mut initial_prompt = None;
    if let Some(arg) = std::env::args().nth(1) {
        match arg.as_str() {
            "-q" | "--quickswitch" => {
                let extra_args: Vec<String> = std::env::args().skip(2).collect();
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
                let extra_args: Vec<String> = std::env::args().skip(2).collect();
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
            _ => {
                let p = Path::new(&arg);
                if p.extension().and_then(|e| e.to_str()) == Some("md") {
                    initial_prompt = Some(format!("Do the todo: {}", arg));
                } else {
                    config_path = Some(p.to_path_buf());
                }
            }
        }
    }

    let mut config = config::load_config(config_path.as_deref())?;

    // Fix: If project_root is not specified in config, default to current working directory
    if config.tools.project_root.is_empty() {
        config.tools.project_root = std::env::current_dir()?.to_string_lossy().to_string();
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

    match app.run(initial_prompt).await {
        Err(e) if e.to_string() == "__QUIT__" => {}
        Err(e) => eprintln!("Error: {}", e),
        Ok(_) => {}
    }
    Ok(())
}
