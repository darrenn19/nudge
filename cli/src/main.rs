//! `nudge` command-line tool. Phase 1 adds `send`, `receive` and `bench`.

fn main() {
    println!(
        "nudge {} (protocol v{})",
        nudge_core::VERSION,
        nudge_core::PROTOCOL_VERSION
    );
}
