use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "yt-dlp-rs")]
#[command(about = "A Rust port of yt-dlp focusing on YouTube")]
pub struct Cli {
    /// URL to download or extract
    pub url: String,

    /// Format selection (e.g., bestvideo+bestaudio/best, 137+140)
    #[arg(short = 'f', long = "format", default_value = "bestvideo+bestaudio/best")]
    pub format: String,

    /// List formats
    #[arg(short = 'F', long = "list-formats")]
    pub list_formats: bool,

    /// Output filename template
    #[arg(short = 'o', long = "output")]
    pub output: Option<String>,

    /// Path to cookies.txt file
    #[arg(long = "cookies")]
    pub cookies: Option<String>,

    /// Print various debugging information
    #[arg(short = 'v', long = "verbose")]
    pub verbose: bool,
}