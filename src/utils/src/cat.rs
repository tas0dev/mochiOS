use swiftlib::io;

#[derive(Debug)]
enum Meow {
    Meow,
    Mew,
    Purr,
}

struct RndGen {
    state: u64,
}

impl RndGen {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next(&mut self) -> u32 {
        self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1);
        (self.state >> 32) as u32
    }
}

fn main() {
    let mut exit_code = 0i32;
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|arg| arg == "--meow") {
        println!("{:?}", print_meow());
        return;
    }

    for path in args {
        let fd = io::open(&path, 0);
        if fd < 0 {
            eprintln!("cat: {}: No such file or directory", path);
            exit_code = 1;
            continue;
        }

        let mut buf = [0u8; 512];
        loop {
            let n = io::read(fd as u64, &mut buf);
            if n <= 0 {
                break;
            }
            if let Ok(s) = core::str::from_utf8(&buf[..n as usize]) {
                print!("{}", s);
            }
        }
        io::close(fd as u64);
    }

    println!();
    std::process::exit(exit_code);
}

fn print_meow() -> Meow {
    let cat_aa = r#"
    A____A
    | .w. |
    |O   O|
    |     |
    |     |
    |     |
    |     |
    |     |
    |     |
    |     |
    |     |
    |     |
    |     |
    |     |
    |     |
    |     |
    |     |
    |     |
    |     |
    |     |
    U-----U
    "#;

    println!("{}", cat_aa);

    let mut rand = RndGen::new(77697987);
    match rand.next() % 3 {
        0 => Meow::Meow,
        1 => Meow::Mew,
        _ => Meow::Purr,
    }
}