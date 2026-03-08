use clap::Parser;
use yt_dlp_rs::YoutubeDl;
use yt_dlp_rs::cli::Cli;
use yt_dlp_rs::utils::{format_size, dim_if_only};
use colored::Colorize;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse command line arguments
    let cli = Cli::parse();

    let mut ytdl = YoutubeDl::new();

    if let Some(cookie_path) = cli.cookies {
        ytdl.set_cookies(&cookie_path);
    }

    if cli.list_formats {
        println!("[youtube] Extracting URL: {}", cli.url);
        println!("[youtube] {}: Downloading webpage", cli.url.split('/').last().unwrap_or(&cli.url));
        println!("[info] Available formats for {}:", cli.url.split('/').last().unwrap_or(&cli.url));
        let formats = match ytdl.extract_info(&cli.url).await {
            Ok(f) => f,
            Err(e) => {
                let err_msg = e.to_string();
                if err_msg.starts_with("ERROR:") {
                    let rest = err_msg["ERROR:".len()..].trim_start();
                    eprintln!("{}: {}", "ERROR".red(), rest);
                } else {
                    eprintln!("{}: {}", "ERROR".red(), err_msg);
                }
                std::process::exit(1);
            }
        };

        let delim = "│".blue();

        struct Row {
            id: String, ext: String, res: String, fps: String, ch: String,
            fs: String, tbr: String, proto: String,
            vcodec: String, vbr: String, acodec: String, abr: String, asr: String,
            more: String,
        }

        let mut rows = Vec::new();

        // Initialize max widths for the format table columns
        let mut max_id = 2;
        let mut max_ext = 3;
        let mut max_res = 10;
        let mut max_fps = 3;
        let mut max_ch = 2;
        let mut max_fs = 8;
        let mut max_tbr = 3;
        let mut max_proto = 5;
        let mut max_vcodec = 6;
        let mut max_vbr = 3;
        let mut max_acodec = 6;
        let mut max_abr = 3;
        let mut max_asr = 3;

        for f in formats {
            let fps_str = f.fps.map(|v| v.to_string()).unwrap_or_default();
            let ch_str = f.ch.map(|v| v.to_string()).unwrap_or_default();
            let tbr_str = f.tbr.map(|v| format!("{:.0}k", v)).unwrap_or_default();
            let asr_str = f.asr.map(|v| format!("{}k", v / 1000)).unwrap_or_default();

            let vbr_str = f.vbr.map(|v| format!("{:.0}k", v)).unwrap_or_default();
            let abr_str = f.abr.map(|v| format!("{:.0}k", v)).unwrap_or_default();

            let vcodec_fmt = if f.vcodec.starts_with("audio") { "audio only".to_string() } else if f.vcodec == "none" { "images".to_string() } else { f.vcodec.clone() };   
            let acodec_fmt = if f.acodec.starts_with("video") { "video only".to_string() } else if f.acodec == "none" { "".to_string() } else { f.acodec.clone() };
            let res = if f.resolution.is_empty() {
                if f.vcodec == "none" && f.acodec == "none" { "".to_string() } else { "audio only".to_string() }
            } else { f.resolution.clone() };

            let fs_str = format_size(f.filesize);

            max_id = max_id.max(f.format_id.len());
            max_ext = max_ext.max(f.ext.len());
            max_res = max_res.max(res.len());
            max_fps = max_fps.max(fps_str.len());
            max_ch = max_ch.max(ch_str.len());
            max_fs = max_fs.max(fs_str.len());
            max_tbr = max_tbr.max(tbr_str.len());
            max_proto = max_proto.max(f.protocol.len());
            max_vcodec = max_vcodec.max(vcodec_fmt.len());
            max_vbr = max_vbr.max(vbr_str.len());
            max_acodec = max_acodec.max(acodec_fmt.len());
            max_abr = max_abr.max(abr_str.len());
            max_asr = max_asr.max(asr_str.len());

            rows.push(Row {
                id: f.format_id, ext: f.ext, res, fps: fps_str, ch: ch_str,
                fs: fs_str, tbr: tbr_str, proto: f.protocol,
                vcodec: vcodec_fmt, vbr: vbr_str, acodec: acodec_fmt, abr: abr_str, asr: asr_str,
                more: f.note
            });
        }

        let header = format!(
            "{} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {}",
            format!("{:width$}", "ID", width=max_id).yellow(),
            format!("{:width$}", "EXT", width=max_ext).yellow(),
            format!("{:width$}", "RESOLUTION", width=max_res).yellow(),
            format!("{:>width$}", "FPS", width=max_fps).yellow(),
            format!("{:>width$}", "CH", width=max_ch).yellow(),
            delim,
            format!("{:>width$}", "FILESIZE", width=max_fs).yellow(),
            format!("{:>width$}", "TBR", width=max_tbr).yellow(),
            format!("{:width$}", "PROTO", width=max_proto).yellow(),
            delim,
            format!("{:width$}", "VCODEC", width=max_vcodec).yellow(),
            format!("{:>width$}", "VBR", width=max_vbr).yellow(),
            format!("{:width$}", "ACODEC", width=max_acodec).yellow(),
            format!("{:>width$}", "ABR", width=max_abr).yellow(),
            format!("{:>width$}", "ASR", width=max_asr).yellow(),
            "MORE INFO".yellow()
        );
        println!("{}", header);

        // Adjust length of dash line dynamically
        // sum of widths + 15 spaces
        let total_width = max_id + max_ext + max_res + max_fps + max_ch + max_fs + max_tbr + max_proto + max_vcodec + max_vbr + max_acodec + max_abr + max_asr + 9 /* MORE INFO */ + 2 /* delims */ + 15;
        println!("{}", "─".repeat(total_width).blue());

        for r in rows {
            let fs_display = if r.fs.starts_with('~') {
                r.fs.bright_black().to_string()
            } else {
                r.fs
            };

            println!(
                "{} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {}",
                format!("{:width$}", r.id, width=max_id).green(),
                format!("{:width$}", r.ext, width=max_ext),
                dim_if_only(&format!("{:width$}", r.res, width=max_res)),
                format!("{:>width$}", r.fps, width=max_fps),
                format!("{:>width$}", r.ch, width=max_ch),
                delim.clone(),
                format!("{:>width$}", fs_display, width=max_fs),
                format!("{:>width$}", r.tbr, width=max_tbr),
                format!("{:width$}", r.proto, width=max_proto),
                delim.clone(),
                dim_if_only(&format!("{:width$}", r.vcodec, width=max_vcodec)),
                format!("{:>width$}", r.vbr, width=max_vbr),
                dim_if_only(&format!("{:width$}", r.acodec, width=max_acodec)),
                format!("{:>width$}", r.abr, width=max_abr),
                format!("{:>width$}", r.asr, width=max_asr),
                r.more
            );
        }
    } else {
        println!("Start downloading (simulation)...");
        println!("Format selection logic applied: {}", cli.format);
        // TODO: Parse the format selector string, then proceed to downloading
    }

    Ok(())
}