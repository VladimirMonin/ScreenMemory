fn main() {
    if std::env::args_os().len() != 1 {
        std::process::exit(2);
    }
    let result = sm_image_worker::serve(std::io::stdin().lock(), std::io::stdout().lock());
    if result.is_err() {
        std::process::exit(3);
    }
}
