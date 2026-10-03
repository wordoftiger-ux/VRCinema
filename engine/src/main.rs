use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;

use vrcinema::config::{apply_env, find_root, load_config, read_env_file};

fn main() -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
        .unwrap_or_else(|| cwd.clone());
    let root = find_root(&exe_dir, &cwd);

    // vars() panics on non-Unicode entries; skip those instead.
    let mut env: HashMap<String, String> = std::env::vars_os()
        .filter_map(|(k, v)| Some((k.to_str()?.to_string(), v.to_str()?.to_string())))
        .collect();
    let env_file = root.join(".env");
    if env_file.exists() {
        match read_env_file(&env_file) {
            Ok(values) => apply_env(&mut env, values),
            Err(e) => {
                eprintln!("Nie można odczytać pliku {}: {e}", env_file.display());
                return ExitCode::FAILURE;
            }
        }
    }

    match load_config(&env, &root, cfg!(windows)) {
        Ok(_) => {
            println!("ok");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
