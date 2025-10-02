use sov_risc0_adapter::host::Risc0Host;
use sov_rollup_interface::zk::ZkvmHost;

fn main() {
    // Minimal reproducer: run the simple even-check guest with a valid hint,
    // first without proving (should work), then with proving (crashes on macOS 15).
    let elf = zk_poc_risc0_methods::EVEN_ELF;
    println!("EVEN_ELF len: {}", elf.len());

    let mut host = Risc0Host::new(elf);
    host.add_hint(100u64);

    println!("About to run_without_proving()...");
    match host.run_without_proving() {
        Ok(_) => println!("run_without_proving() ok"),
        Err(e) => {
            eprintln!("run_without_proving() error: {e:?}");
            return;
        }
    }

    println!("About to run() with proving...");
    match host.run() {
        Ok(_receipt) => println!("run() ok (unexpected if crash exists)"),
        Err(e) => eprintln!("run() returned error: {e:?}"),
    }
}
