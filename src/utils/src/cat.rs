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

/// バイナリファイルかどうかを判定する
fn is_binary(data: &[u8]) -> bool {
    if data.is_empty() {
        return false;
    }
    let mut non_text = 0usize;
    for &b in data {
        match b {
            0x00 => return true, // ヌルバイトは即バイナリ
            0x01..=0x08 | 0x0b | 0x0c | 0x0e..=0x1f | 0x7f => non_text += 1,
            _ => {}
        }
    }
    // 非テキスト文字が 10% 超えたらバイナリ
    non_text * 10 > data.len()
}

fn main() {
    let mut exit_code = 0i32;
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|arg| arg == "--meow") {
        println!("{:?}", print_meow());
        return;
    }

    for path in &args {
        let fd = io::open(path, 0);
        if fd < 0 {
            eprintln!("cat: {}: No such file or directory", path);
            exit_code = 1;
            continue;
        }

        // ファイル全体を読み込んでバイナリ判定してから出力
        let mut data = Vec::new();
        let mut buf = [0u8; 512];
        loop {
            let n = io::read(fd as u64, &mut buf);
            if n <= 0 {
                break;
            }
            data.extend_from_slice(&buf[..n as usize]);
        }
        io::close(fd as u64);

        if is_binary(&data) {
            eprintln!("cat: {}: this file is not text file", path);
            exit_code = 1;
            continue;
        }

        match core::str::from_utf8(&data) {
            Ok(s) => print!("{}", s),
            Err(_) => {
                eprintln!("cat: {}: this file is not text file", path);
                exit_code = 1;
            }
        }
    }

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