#[cfg(all(target_os = "linux", target_env = "musl"))]
mod mochi_main;
#[cfg(all(target_os = "linux", target_env = "gnu"))]
mod host_main;

#[cfg(all(target_os = "linux", target_env = "musl"))]
fn main() {
    mochi_main::main();
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn main() {
    if let Err(e) = host_main::main() {
        eprintln!("[Binder host] {}", e);
    }
}

#[cfg(not(any(
    all(target_os = "linux", target_env = "musl"),
    all(target_os = "linux", target_env = "gnu")
)))]
fn main() {
    eprintln!("Binder is only supported on linux-gnu(host) and linux-musl(mochiOS).");
}
