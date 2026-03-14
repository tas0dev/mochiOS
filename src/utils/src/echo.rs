fn main() {
    let mut args = std::env::args();
    args.next(); // skip argv[0]
    let parts: Vec<String> = args.collect();
    println!("{}", parts.join(" "));
}
