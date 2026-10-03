use std::process;

fn main() {
    match vexa_boot::run_from_environment(vexa_boot::forwarded_args()) {
        Ok(code) => process::exit(code),
        Err(error) => {
            eprintln!("vexa-boot: {error}");
            process::exit(1);
        }
    }
}
