fn main() -> Result<(), Box<dyn std::error::Error>> {
    nexus_eval::run(std::env::args_os())
}
