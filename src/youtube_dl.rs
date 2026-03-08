use reqwest::Client;
use std::error::Error;
use fancy_regex::Regex;
use serde_json::Value;
use rand::Rng;

use crate::format::Format;

pub struct YoutubeDl {
    client: Client,
    cookies_path: Option<String>,
}

impl YoutubeDl {
    /// Creates a new YoutubeDl instance with a randomized Chrome user agent.
    pub fn new() -> Self {
        let mut rng = rand::thread_rng();
        let chrome_major: u32 = rng.gen_range(137..=143);
        let user_agent = format!("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{}.0.0.0 Safari/537.36", chrome_major);

        Self {
            client: Client::builder()
                .user_agent(user_agent)
                .build()
                .unwrap(),
            cookies_path: None,
        }
    }

    pub fn set_cookies(&mut self, path: &str) {
        self.cookies_path = Some(path.to_string());
    }

    /// Parses Netscape format cookies from a file path.
    fn load_netscape_cookies(path: &str) -> std::io::Result<String> {
        let content = std::fs::read_to_string(path)?;
        let mut cookie_parts = Vec::new();
        for line in content.lines() {
            let line = line.trim();
            // Netscape cookie file comments
            if line.is_empty() || (line.starts_with('#') && !line.starts_with("#HttpOnly_")) {
                continue;
            }
            let line = line.strip_prefix("#HttpOnly_").unwrap_or(line);
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() >= 7 {
                let domain = fields[0];
                if domain.contains("youtube") || domain.contains("google") {
                    let name = fields[5].trim();
                    let value = fields[6].trim();
                    cookie_parts.push(format!("{}={}", name, value));
                }
            }
        }
        Ok(cookie_parts.join("; "))
    }
    /// Extracts video and audio codecs from a mime-type string.
    
    fn parse_mimetype(mime: &str) -> (String, String, String) {
        // Ex: video/mp4; codecs="avc1.42001E, mp4a.40.2"
        let parts: Vec<&str> = mime.split(';').collect();
        let mime_type = parts[0];
        let ext = mime_type.split('/').last().unwrap_or("unknown").to_string();

        let mut vcodec = "none".to_string();
        let mut acodec = "none".to_string();

        if mime_type.starts_with("audio/") {
            vcodec = "audio only".to_string();
        }
        if mime_type.starts_with("video/") {
            acodec = "video only".to_string();
        }

        if parts.len() > 1 && parts[1].trim().starts_with("codecs=") {
            let codecs_str = parts[1].trim()["codecs=".len()..].trim_matches('\"');
            let codecs: Vec<&str> = codecs_str.split(',').map(|s| s.trim()).collect();
            if mime_type.starts_with("audio/") {
                acodec = codecs[0].to_string();
            } else if mime_type.starts_with("video/") {
                vcodec = codecs[0].to_string();
                if codecs.len() > 1 {
                    acodec = codecs[1].to_string();
                }
            }
        }
        (ext, vcodec, acodec)
    }

    /// Fetches YouTube page and extracts the available streaming formats.
    pub async fn extract_info(&self, url: &str) -> Result<Vec<Format>, Box<dyn Error>> {
        let mut req = self.client.get(url)
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
            .header("Accept-Language", "en-us,en;q=0.5")
            .header("Sec-Fetch-Mode", "navigate");

        if let Some(cookie_path) = &self.cookies_path {
            if let Ok(cookie_str) = Self::load_netscape_cookies(cookie_path) {
                if !cookie_str.is_empty() {
                    req = req.header("Cookie", cookie_str);
                    req = req.header("Accept-Language", "en-US,en;q=0.9");
                }
            }
        }

        let res = req.send().await?;
        let html = res.text().await?;

        // Extract ytInitialPlayerResponse using fancy-regex
        let re = Regex::new(r#"ytInitialPlayerResponse\s*=\s*(\{.+?\});(?:var|</script>)"#)?;
        let captures = re.captures(&html)?;

        let json_str = match captures {
            Some(caps) => caps.get(1).unwrap().as_str(),
            None => return Err("ytInitialPlayerResponse not found in HTML".into()),
        };

        let json: Value = serde_json::from_str(json_str)?;

        // Check for YouTube playability status (errors, login boundaries, bot checks)
        if let Some(playability) = json.get("playabilityStatus") {
            if let Some(status) = playability.get("status").and_then(|s| s.as_str()) {
                if status == "ERROR" || status == "LOGIN_REQUIRED" || status == "UNPLAYABLE" {
                    let reason = playability.get("reason").and_then(|r| r.as_str()).unwrap_or("Unknown error");
                    // Extract video ID safely
                    let video_id = if url.contains("v=") {
                        url.split("v=").nth(1).unwrap_or("").split('&').next().unwrap_or("unknown")
                    } else {
                        url.split('/').last().unwrap_or("").split('?').next().unwrap_or("unknown")
                    };
                    
                    if status == "LOGIN_REQUIRED" || reason.contains("bot") {
                        let error_msg = format!("ERROR: [youtube] {}: {}. Use --cookies for the authentication.", video_id, reason);
                        return Err(error_msg.into());
                    } else {
                        return Err(format!("ERROR: [youtube] {}: {}", video_id, reason).into());
                    }
                }
            }
        }

        let mut parsed_formats = Vec::new();

        // Local extraction function to iterate over formats and adaptiveFormats
        let mut extract_stream_formats = |formats_array: &Vec<Value>, is_adaptive: bool| {
            for f in formats_array {
                let format_id = f["itag"].as_i64().unwrap_or(0).to_string();
                let mime_type = f["mimeType"].as_str().unwrap_or("");
                let (ext, vcodec, acodec) = Self::parse_mimetype(mime_type);

                let width = f["width"].as_u64();
                let height = f["height"].as_u64();
                let resolution = if let (Some(w), Some(h)) = (width, height) {
                    format!("{}x{}", w, h)
                } else {
                    "".to_string()
                };

                let fps = f["fps"].as_u64();
                let ch = f["audioChannels"].as_u64().map(|v| v as u8);

                let bitrate = f["bitrate"].as_f64();
                let tbr = bitrate.map(|b| b / 1000.0);

                let mut vbr = f["averageBitrate"].as_f64().map(|b| b / 1000.0);
                let mut abr = None;

                if vcodec == "none" || vcodec == "images" || vcodec == "audio only" {
                    vbr = None;
                }
                if acodec != "none" && acodec != "video only" {
                    abr = f["audioSampleRate"].as_str().and_then(|_| vbr);
                    if vcodec == "none" || vcodec == "audio only" {
                         abr = tbr;
                    }
                }
                if vcodec != "none" && vcodec != "audio only" && acodec != "none" && acodec != "video only" {
                    // For format 18, it's combined, so it only has tbr, neither vbr nor abr are exact
                    vbr = None;
                    abr = None;
                }

                let asr = f["audioSampleRate"].as_str().and_then(|s| s.parse().ok());
                let protocol = "https".to_string(); // Simplification for now
                let url = f["url"].as_str().unwrap_or("").to_string();

                // If the signature is ciphered, the URL is not directly usable.
                // We will need to implement JS decryption later on.
                let note = if f["signatureCipher"].is_string() {
                    "encrypted_signature".to_string()
                } else if is_adaptive {
                    format!("{}_dash", ext)
                } else {
                    "".to_string()
                };

                parsed_formats.push(Format {
                    format_id,
                    ext,
                    width,
                    height,
                    resolution,
                    fps,
                    ch,
                    vcodec,
                    vbr,
                    acodec,
                    abr,
                    asr,
                    tbr,
                    protocol,
                    filesize: f["contentLength"].as_str().and_then(|s| s.parse().ok()),
                    url,
                    note,
                });
            }
        };

        if let Some(streaming_data) = json.get("streamingData") {
            if let Some(formats) = streaming_data.get("formats").and_then(|v| v.as_array()) {
                extract_stream_formats(formats, false);
            }
            if let Some(adaptive) = streaming_data.get("adaptiveFormats").and_then(|v| v.as_array()) {
                extract_stream_formats(adaptive, true);
            }
        }

        parsed_formats.sort_by(|a, b| {
            let has_video = |f: &Format| f.vcodec != "none" && !f.vcodec.starts_with("audio") && f.vcodec != "images";
            let has_audio = |f: &Format| f.acodec != "none" && !f.acodec.starts_with("video");
            let is_images = |f: &Format| f.vcodec == "none" && f.acodec == "none" || f.vcodec == "images" || f.note.contains("storyboard");

            let a_v = has_video(a);
            let a_a = has_audio(a);
            let a_i = is_images(a);

            let b_v = has_video(b);
            let b_a = has_audio(b);
            let b_i = is_images(b);

            let type_score = |v: bool, a: bool, i: bool| {
                if i {
                    0 // images / storyboards at the top
                } else if !v && a {
                    1 // audio only
                } else {
                    2 // video only AND combined
                }
            };

            let score_a = type_score(a_v, a_a, a_i);
            let score_b = type_score(b_v, b_a, b_i);

            let vcodec_score = |c: &str| {
                if c.contains("av01") { 4 }
                else if c.contains("vp9") { 3 }
                else if c.contains("vp8") { 2 }
                else if c.contains("avc") || c.contains("h264") { 1 }
                else { 0 }
            };

            let acodec_score = |c: &str| {
                if c.contains("opus") { 3 }
                else if c.contains("vorbis") || c.contains("ogg") { 2 }
                else if c.contains("mp4a") || c.contains("aac") { 1 }
                else { 0 }
            };

            score_a.cmp(&score_b)
                // Resolution (height then width)
                .then(a.height.unwrap_or(0).cmp(&b.height.unwrap_or(0)))
                .then(a.width.unwrap_or(0).cmp(&b.width.unwrap_or(0)))
                // Framerate
                .then(a.fps.unwrap_or(0).cmp(&b.fps.unwrap_or(0)))
                // vcodec
                .then(vcodec_score(&a.vcodec).cmp(&vcodec_score(&b.vcodec)))
                // acodec (to differentiate audio only, or audio from combined)
                .then(acodec_score(&a.acodec).cmp(&acodec_score(&b.acodec)))
                // Total bitrate for video or combined streams and filesize
                .then(a.tbr.unwrap_or(0.0).partial_cmp(&b.tbr.unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal))
                .then(a.filesize.unwrap_or(0).cmp(&b.filesize.unwrap_or(0)))
                // Format ID
                .then(a.format_id.cmp(&b.format_id))
        });

        Ok(parsed_formats)
    }
}
