use colored::Colorize;

pub fn format_size(bytes: Option<u64>) -> String {
    match bytes {
        Some(b) => {
            let k = 1024.0;
            let m = 1048576.0;
            let g = 1073741824.0;
            let bf = b as f64;
            if bf >= g {
                format!("{:.2}GiB", bf / g)
            } else if bf >= m {
                format!("{:.2}MiB", bf / m)
            } else if bf >= k {
                format!("{:.2}KiB", bf / k)
            } else {
                format!("{}B", b)
            }
        }
        None => "~       ".to_string(),
    }
}

pub fn dim_if_only(text: &str) -> String {
    if text.contains("only") {
        text.bright_black().to_string()
    } else {
        text.to_string()
    }
}
