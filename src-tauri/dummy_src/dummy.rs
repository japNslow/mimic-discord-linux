// A sleeping local process. No network, shell, file access or downloaded code.
#[cfg(not(target_os = "linux"))]
compile_error!("Mimic Linux only supports Linux");
unsafe extern "C" {
    fn prctl(option: i32, arg2: usize, arg3: usize, arg4: usize, arg5: usize) -> i32;
    fn getppid() -> i32;
}
fn main() {
    let expected_parent: i32 = std::env::args().nth(1).and_then(|v| v.parse().ok()).unwrap_or(0);
    // Repeat the parent-death setup after exec and close the fork/exec race.
    unsafe {
        if expected_parent <= 1 || prctl(1, 9, 0, 0, 0) != 0 || getppid() != expected_parent {
            std::process::exit(1);
        }
        if prctl(38, 1, 0, 0, 0) != 0 { std::process::exit(1); } // NO_NEW_PRIVS
    }
    // Leave comm as the executable basename, not the human-readable game title.
    loop { std::thread::sleep(std::time::Duration::from_secs(3600)); }
}
