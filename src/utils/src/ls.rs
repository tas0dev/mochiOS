use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();
    let path = if args.len() > 1 { &args[1] } else { "." };

    match std::fs::read_dir(path) {
        Ok(entries) => {
            let mut names: Vec<String> = entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            for name in &names {
                println!("{}", name);
            }
        }
        Err(e) => {
            eprintln!("ls: {}: {}", path, e);
            std::process::exit(1);
        }
    }
}
