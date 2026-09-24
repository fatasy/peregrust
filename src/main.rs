fn main() {
    match peregrust::run() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("Peregrust: {error:#}");
            std::process::exit(1);
        }
    }
}
