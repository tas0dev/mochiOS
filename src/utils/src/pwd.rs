use swiftlib::fs;

fn main() {
    let mut buf = [0u8; 256];
    match fs::getcwd(&mut buf) {
        Some(cwd) => println!("{}", cwd),
        None => {
            eprintln!("pwd: failed to get current directory");
            std::process::exit(1);
        }
    }
}
