use swiftlib::{fs, io};

fn main() {
    let mut args = std::env::args();
    args.next(); // skip argv[0]
    let path = args.next().unwrap_or_else(|| ".".to_string());

    // Open the directory
    let fd = io::open(&path, 0);
    if fd < 0 {
        println!("ls: {}: cannot open directory", path);
        std::process::exit(1);
    }

    let mut buf = [0u8; 4096];
    let n = fs::readdir(fd as u64, &mut buf);

    io::close(fd as u64);

    if n == 0 {
        return;
    }

    let content = core::str::from_utf8(&buf[..n as usize]).unwrap_or("");
    let mut names: Vec<&str> = content.split('\n').filter(|s| !s.is_empty()).collect();
    names.sort();
    for name in names {
        println!("{}", name);
    }
}
