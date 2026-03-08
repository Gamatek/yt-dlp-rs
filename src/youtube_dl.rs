use reqwest::Client;
use std::error::Error;
use fancy_regex::Regex;
use serde_json::Value;
use rand::Rng;
use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;
use colored::Colorize;

use crate::format::Format;

fn format_bytes(n: u64) -> String {
    const GIB: u64 = 1024 * 1024 * 1024;
    const MIB: u64 = 1024 * 1024;
    const KIB: u64 = 1024;
    if n >= GIB {
        format!("{:.2}GiB", n as f64 / GIB as f64)
    } else if n >= MIB {
        format!("{:.2}MiB", n as f64 / MIB as f64)
    } else if n >= KIB {
        format!("{:.2}KiB", n as f64 / KIB as f64)
    } else {
        format!("{} B", n)
    }
}

pub struct YoutubeDl {
    client: Client,
    pub verbose: bool,
    cookies_path: Option<String>,
    player_js: Option<String>,
    pub title: Option<String>,
}

impl YoutubeDl {
    /// Creates a new YoutubeDl instance with a randomized Chrome user agent.
    pub fn new(verbose: bool) -> Self {
        let mut rng = rand::thread_rng();
        let chrome_major: u32 = rng.gen_range(137..=143);
        let user_agent = format!("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{}.0.0.0 Safari/537.36", chrome_major);

        Self {
            client: Client::builder()
                .user_agent(user_agent)
                .build()
                .unwrap(),
            verbose,
            cookies_path: None,
            player_js: None,
            title: None,
        }
    }

    pub fn set_cookies(&mut self, path: &str) {
        self.cookies_path = Some(path.to_string());
    }

    /// Parses Netscape format cookies from a file path.
    fn load_netscape_cookies(path: &str, verbose: bool) -> std::io::Result<String> {
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
        let result = cookie_parts.join("; ");
        if verbose {
            if verbose { eprintln!("[debug] Cookies parsed: {}", cookie_parts.len()); }
        }
        Ok(result)
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
    pub async fn extract_info(&mut self, url: &str) -> Result<Vec<Format>, Box<dyn Error>> {
        let video_id = Self::extract_video_id(url).unwrap_or_else(|| "unknown".to_string());
        eprintln!("[youtube] {}: Downloading webpage", video_id);
        let mut req = self.client.get(url)
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
            .header("Accept-Language", "en-us,en;q=0.5")
            .header("Accept-Encoding", "identity")
            .header("Sec-Fetch-Mode", "navigate");

        if let Some(cookie_path) = &self.cookies_path {
            if let Ok(cookie_str) = Self::load_netscape_cookies(cookie_path, self.verbose) {
                if !cookie_str.is_empty() {
                    req = req.header("Cookie", cookie_str);
                    req = req.header("Accept-Language", "en-US,en;q=0.9");
                }
            }
        }

        let res = req.send().await?;
        let html = res.text().await?;

        // Fetch TV player JS (pinned to 9f4cc5e4, variante tv) pour le solveur n-sig/sig.
        // yt-dlp uses the same player because the AST resolution algorithm is compatible.
        if self.player_js.is_none() {
            let tv_player_url = "https://www.youtube.com/s/player/9f4cc5e4/tv-player-ias.vflset/tv-player-ias.js";
            if self.verbose { eprintln!("[debug] Fetching TV player JS: {}", tv_player_url); }
            match self.client.get(tv_player_url).send().await {
                Ok(js_res) => match js_res.text().await {
                    Ok(js_text) => {
                        if self.verbose { eprintln!("[debug] TV player JS fetched ({} bytes)", js_text.len()); }
                        self.player_js = Some(js_text);
                    }
                    Err(e) => eprintln!("[warn] Failed to read TV player JS body: {}", e),
                },
                Err(e) => eprintln!("[warn] Failed to fetch TV player JS: {}", e),
            }
        }

        // Extract ytInitialPlayerResponse using fancy-regex
        let re = Regex::new(r#"ytInitialPlayerResponse\s*=\s*(\{.+?\});(?:var|</script>)"#)?;
        let captures = re.captures(&html)?;

        let json_str = match captures {
            Some(caps) => caps.get(1).unwrap().as_str(),
            None => return Err("ytInitialPlayerResponse not found in HTML".into()),
        };

        let json: Value = serde_json::from_str(json_str)?;

        // Extract video title
        self.title = json.get("videoDetails")
            .and_then(|vd| vd.get("title"))
            .and_then(|t| t.as_str())
            .map(|s| s.to_string());

        // Check for YouTube playability status (errors, login boundaries, bot checks)
        if let Some(playability) = json.get("playabilityStatus") {
            if let Some(status) = playability.get("status").and_then(|s| s.as_str()) {
                if status == "ERROR" || status == "LOGIN_REQUIRED" || status == "UNPLAYABLE" {
                    let reason = playability.get("reason").and_then(|r| r.as_str()).unwrap_or("Unknown error");
                    if status == "LOGIN_REQUIRED" || reason.contains("bot") {
                        let error_msg = format!("ERROR: [youtube] {}: {}. Use --cookies for the authentication.", video_id, reason);
                        return Err(error_msg.into());
                    } else {
                        eprintln!("WARNING: [youtube] {}: {}. Will try Innertube APIs...", video_id, reason);
                    }
                }
            }
        }

        let mut parsed_formats = Vec::new();

        // Local extraction function to iterate over formats and adaptiveFormats
        let mut extract_stream_formats = |formats_array: &Vec<Value>, is_adaptive: bool| {
            for f in formats_array {
                let format_id = f["itag"].as_i64().unwrap_or(0).to_string();

                // Temporary debug for format 251
                if format_id == "251" {
                    if self.verbose { eprintln!("[debug] format 251 full JSON: {}", serde_json::to_string_pretty(f).unwrap_or_default()); }
                }

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
                let sig_cipher = f["signatureCipher"].as_str().map(|s| s.to_string());

                let note = if sig_cipher.is_some() {
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
                    sig_cipher,
                    note,
                });
            }
        };

        let mut has_formats = false;
        if let Some(streaming_data) = json.get("streamingData") {
            if let Some(formats) = streaming_data.get("formats").and_then(|v| v.as_array()) {
                if !formats.is_empty() { has_formats = true; }
                extract_stream_formats(formats, false);
            }
            if let Some(adaptive) = streaming_data.get("adaptiveFormats").and_then(|v| v.as_array()) {
                if !adaptive.is_empty() { has_formats = true; }
                extract_stream_formats(adaptive, true);
            }
        }

        if !has_formats {
            eprintln!("[youtube] {}: Downloading tv downgraded player API JSON", video_id);
            if self.verbose { eprintln!("[debug] No formats found in initial webpage, trying TV_DOWNGRADED API..."); }
            if let Ok(tv_json) = self.fetch_innertube_tv_downgraded(&video_id).await {
                if self.verbose { eprintln!("[debug] TV_DOWNGRADED playabilityStatus: {:?}", tv_json.get("playabilityStatus")); }
                if self.verbose { eprintln!("[debug] TV_DOWNGRADED has streamingData: {}", tv_json.get("streamingData").is_some()); }
                if let Some(streaming_data) = tv_json.get("streamingData") {
                    if let Some(formats) = streaming_data.get("formats").and_then(|v| v.as_array()) {
                        extract_stream_formats(formats, false);
                    }
                    if let Some(adaptive) = streaming_data.get("adaptiveFormats").and_then(|v| v.as_array()) {
                        extract_stream_formats(adaptive, true);
                    }
                }
            }
        }

        drop(extract_stream_formats);

        // --- Step 1: parse signatureCiphers and collect 's' values ---
        // Structure : format_id -> (base_url, sp_param, s_value)
        let mut sig_cipher_map: std::collections::HashMap<String, (String, String, String)> = std::collections::HashMap::new();
        for fmt in &mut parsed_formats {
            if let Some(sc) = fmt.sig_cipher.take() {
                let mut s = String::new();
                let mut sp = "sig".to_string();
                let mut base_url = String::new();
                for (key, val) in url::form_urlencoded::parse(sc.as_bytes()) {
                    match key.as_ref() {
                        "s"   => s        = val.to_string(),
                        "sp"  => sp       = val.to_string(),
                        "url" => base_url = val.to_string(),
                        _     => {}
                    }
                }
                if !s.is_empty() && !base_url.is_empty() {
                    if self.verbose { eprintln!("[debug] format {} has signatureCipher (s len={})", fmt.format_id, s.len()); }
                    sig_cipher_map.insert(fmt.format_id.clone(), (base_url, sp, s));
                }
            }
        }

        // --- Step 2: fetch missing URLs via Innertube ANDROID_VR ---
        let missing_count = parsed_formats.iter().filter(|f| f.url.is_empty() && !sig_cipher_map.contains_key(&f.format_id)).count();
        if missing_count > 0 {
            eprintln!("[youtube] {}: Downloading ANDROID VR player API JSON", video_id);
            if self.verbose { eprintln!("[debug] {} format(s) without URL — Innertube ANDROID_VR API request", missing_count); }
            match self.fetch_innertube_android_vr(&video_id).await {
                Ok((innertube_urls, innertube_sig_ciphers)) => {
                    for fmt in &mut parsed_formats {
                        if fmt.url.is_empty() && !sig_cipher_map.contains_key(&fmt.format_id) {
                            if let Some(u) = innertube_urls.get(&fmt.format_id) {
                                fmt.url = u.clone();
                                if self.verbose { eprintln!("[debug] format {} URL directe via Innertube ANDROID_VR", fmt.format_id); }
                            } else if let Some(sc) = innertube_sig_ciphers.get(&fmt.format_id) {
                                sig_cipher_map.insert(fmt.format_id.clone(), sc.clone());
                                if self.verbose { eprintln!("[debug] format {} signatureCipher via Innertube ANDROID_VR", fmt.format_id); }
                            }
                        }
                    }
                }
                Err(e) => eprintln!("[warn] Innertube ANDROID_VR API failed: {}", e),
            }
        }

        // --- Step 2.5: if some are still missing, try TV_DOWNGRADED ---
        let missing_count_after_vr = parsed_formats.iter().filter(|f| f.url.is_empty() && !sig_cipher_map.contains_key(&f.format_id)).count();
        if missing_count_after_vr > 0 {
            eprintln!("[youtube] {}: Downloading TV_DOWNGRADED player API JSON because {} formats are missing", video_id, missing_count_after_vr);
            if let Ok(tv_json) = self.fetch_innertube_tv_downgraded(&video_id).await {
                let mut tv_urls = std::collections::HashMap::new();
                let mut tv_sig = std::collections::HashMap::new();
                if let Some(sd) = tv_json.get("streamingData") {
                    for arr_key in &["formats", "adaptiveFormats"] {
                        if let Some(arr) = sd.get(arr_key).and_then(|v| v.as_array()) {
                            for f in arr {
                                let itag = match f["itag"].as_i64() {
                                    Some(i) => i.to_string(),
                                    None => continue,
                                };
                                if let Some(url) = f["url"].as_str() {
                                    tv_urls.insert(itag, url.to_string());
                                } else if let Some(sc) = f["signatureCipher"].as_str().or_else(|| f["cipher"].as_str()) {
                                    let mut s = String::new();
                                    let mut sp = "sig".to_string();
                                    let mut base_url = String::new();
                                    for (key, val) in url::form_urlencoded::parse(sc.as_bytes()) {
                                        match key.as_ref() {
                                            "s"   => s        = val.to_string(),
                                            "sp"  => sp       = val.to_string(),
                                            "url" => base_url = val.to_string(),
                                            _     => {}
                                        }
                                    }
                                    if !s.is_empty() && !base_url.is_empty() {
                                        tv_sig.insert(itag, (base_url, sp, s));
                                    }
                                }
                            }
                        }
                    }
                }
                for fmt in &mut parsed_formats {
                    if fmt.url.is_empty() && !sig_cipher_map.contains_key(&fmt.format_id) {
                        if let Some(u) = tv_urls.get(&fmt.format_id) {
                            fmt.url = u.clone();
                            if self.verbose { eprintln!("[debug] format {} URL directe via TV_DOWNGRADED", fmt.format_id); }
                        } else if let Some(sc) = tv_sig.get(&fmt.format_id) {
                            sig_cipher_map.insert(fmt.format_id.clone(), sc.clone());
                            if self.verbose { eprintln!("[debug] format {} signatureCipher via TV_DOWNGRADED", fmt.format_id); }
                        }
                    }
                }
            }
        }

        // --- Step 3: collect all n and sig values for batch resolution ---
        // Deduplicate n values by their content
        let mut all_n_values: std::collections::HashSet<String> = std::collections::HashSet::new();
        for fmt in &parsed_formats {
            if let Some(n) = Self::extract_n_param(&fmt.url) {
                all_n_values.insert(n);
            }
        }
        // For signatureCiphers, base_url may also contain an n param
        for (_, (base_url, _, _)) in &sig_cipher_map {
            if let Some(n) = Self::extract_n_param(base_url) {
                all_n_values.insert(n);
            }
        }
        let sig_values_list: Vec<String> = sig_cipher_map.values().map(|(_, _, s)| s.clone()).collect();

        // --- Step 4: batch resolution via Node.js solver ---
        let player_js = self.player_js.as_deref().unwrap_or_default();
        let n_refs: Vec<&str> = all_n_values.iter().map(|s| s.as_str()).collect();
        let sig_refs: Vec<&str> = sig_values_list.iter().map(|s| s.as_str()).collect();

        let (n_map, sig_map) = if !n_refs.is_empty() || !sig_refs.is_empty() {
            match crate::js_interp::solve_challenges(player_js, &n_refs, &sig_refs, self.verbose) {
                Ok(result) => {
                    if self.verbose { eprintln!("[debug] Solver: {} n-value(s) and {} signature(s) decrypted",
                        result.n_values.len(), result.sig_values.len()); }
                    if !sig_refs.is_empty() && result.sig_values.is_empty() {
                        eprintln!("WARNING: [youtube] {}: Signature solving failed: Some formats may be missing", video_id);
                    }
                    if !n_refs.is_empty() && result.n_values.is_empty() {
                        eprintln!("WARNING: [youtube] {}: n challenge solving failed: Some formats may be missing", video_id);
                    }
                    (result.n_values, result.sig_values)
                }
                Err(e) => {
                    if !sig_refs.is_empty() {
                        eprintln!("WARNING: [youtube] {}: Signature solving failed: {}", video_id, e);
                    }
                    if !n_refs.is_empty() {
                        eprintln!("WARNING: [youtube] {}: n challenge solving failed: {}", video_id, e);
                    }
                    (std::collections::HashMap::new(), std::collections::HashMap::new())
                }
            }
        } else {
            (std::collections::HashMap::new(), std::collections::HashMap::new())
        };

        // --- Step 5: apply the decrypted values ---
        // Apply decrypted signatures to formats with signatureCipher
        for fmt in &mut parsed_formats {
            if fmt.url.is_empty() {
                if let Some((base_url, sp, s)) = sig_cipher_map.get(&fmt.format_id) {
                    let decrypted_s = sig_map.get(s).cloned().unwrap_or_else(|| {
                        eprintln!("WARNING: [youtube] {}: Signature solving failed for format {}", video_id, fmt.format_id);
                        s.clone()
                    });
                    let mut resolved_url = format!("{}&{}={}", base_url, sp,
                        url::form_urlencoded::byte_serialize(decrypted_s.as_bytes()).collect::<String>());
                    resolved_url = Self::apply_n_map_to_url(&resolved_url, &n_map, self.verbose);
                    fmt.url = resolved_url;
                }
            } else {
                fmt.url = Self::apply_n_map_to_url(&fmt.url, &n_map, self.verbose);
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

    /// Extracts the video ID from a YouTube URL.
    fn extract_video_id(url: &str) -> Option<String> {
        if let Some(rest) = url.split("youtu.be/").nth(1) {
            return Some(rest.split(|c| c == '?' || c == '&' || c == '/').next()?.to_string());
        }
        if let Some(rest) = url.split("v=").nth(1) {
            return Some(rest.split(|c| c == '&' || c == '#').next()?.to_string());
        }
        None
    }

    async fn fetch_innertube_tv_downgraded(&self, video_id: &str) -> std::result::Result<Value, Box<dyn Error>> {
        let body = serde_json::json!({
            "context": {
                "client": {
                    "clientName": "TVHTML5",
                    "clientVersion": "5.20260114",
                    "userAgent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Cobalt/Version",
                    "hl": "en",
                    "gl": "US"
                }
            },
            "videoId": video_id,
            "playbackContext": {
                "contentPlaybackContext": {
                    "signatureTimestamp": 20514
                }
            }
        });

        let mut req = self.client
            .post("https://www.youtube.com/youtubei/v1/player?key=AIzaSyAO_FJ2SlqU8Q4STEHLGCilw_Y9_11qcW8")
            .header("Content-Type", "application/json")
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Cobalt/Version")
            .header("Origin", "https://www.youtube.com");

        if let Some(cookie_path) = &self.cookies_path {
            if let Ok(cookie_str) = Self::load_netscape_cookies(cookie_path, self.verbose) {
                if !cookie_str.is_empty() {
                    req = req.header("Cookie", &cookie_str);

                    // Try to find a SAPISID (or variants)
                    let get_sid = |prefix: &str| -> Option<&str> {
                        cookie_str.split("; ").find(|s| s.starts_with(prefix)).map(|s| &s[prefix.len()..])
                    };
                    
                    let mut auth_header = None;
                    if let Some(sid) = get_sid("SAPISID=") {
                        use sha1::{Sha1, Digest};
                        let timestamp = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs()
                            .to_string();
                        let origin = "https://www.youtube.com";
                        let hash_input = format!("{} {} {}", timestamp, sid, origin);
                        let mut hasher = Sha1::new();
                        hasher.update(hash_input.as_bytes());
                        let result = hasher.finalize();
                        let hex_str = result.iter().fold(String::new(), |acc, b| acc + &format!("{:02x}", b));
                        auth_header = Some(format!("SAPISIDHASH {}_{}", timestamp, hex_str));
                    }
                    if let Some(auth) = auth_header {
                        req = req.header("Authorization", auth);
                    }
                }
            }
        }

        let res = req.json(&body).send().await?;
        let json: Value = res.json().await?;
        Ok(json)
    }

    /// Fetches streaming URLs and signatureCiphers via the Innertube ANDROID_VR API.
    /// The ANDROID_VR client doesn't require a GVS PO token, unlike the plain ANDROID client.
    async fn fetch_innertube_android_vr(&self, video_id: &str) -> Result<(
        std::collections::HashMap<String, String>,                    // url directe : format_id -> url
        std::collections::HashMap<String, (String, String, String)>  // sig_cipher  : format_id -> (base_url, sp, s)
    ), Box<dyn Error>> {
        let body = serde_json::json!({
            "context": {
                "client": {
                    "clientName": "ANDROID_VR",
                    "clientVersion": "1.71.26",
                    "deviceMake": "Oculus",
                    "deviceModel": "Quest 3",
                    "androidSdkVersion": 32,
                    "osName": "Android",
                    "osVersion": "12L",
                    "hl": "en",
                    "gl": "US"
                }
            },
            "videoId": video_id
        });

        let mut builder = self.client
            .post("https://www.youtube.com/youtubei/v1/player?key=AIzaSyAO_FJ2SlqU8Q4STEHLGCilw_Y9_11qcW8")
            .header("Content-Type", "application/json")
            .header("User-Agent", "com.google.android.apps.youtube.vr.oculus/1.71.26 (Linux; U; Android 12L; eureka-user Build/SQ3A.220605.009.A1) gzip");

        if let Some(cookie_path) = &self.cookies_path {
            if let Ok(cookie_str) = Self::load_netscape_cookies(cookie_path, false) {
                if !cookie_str.is_empty() {
                    builder = builder.header("Cookie", cookie_str);
                }
            }
        }

        let res = builder
            .json(&body)
            .send()
            .await?;

        let json: Value = res.json().await?;

        let mut urls: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        let mut sig_ciphers: std::collections::HashMap<String, (String, String, String)> = std::collections::HashMap::new();

        if let Some(sd) = json.get("streamingData") {
            for arr_key in &["formats", "adaptiveFormats"] {
                if let Some(arr) = sd.get(arr_key).and_then(|v| v.as_array()) {
                    for f in arr {
                        let itag = match f["itag"].as_i64() {
                            Some(i) => i.to_string(),
                            None => continue,
                        };
                        if let Some(url) = f["url"].as_str() {
                            urls.insert(itag, url.to_string());
                        } else if let Some(sc) = f["signatureCipher"].as_str().or_else(|| f["cipher"].as_str()) {
                            let mut s = String::new();
                            let mut sp = "sig".to_string();
                            let mut base_url = String::new();
                            for (key, val) in url::form_urlencoded::parse(sc.as_bytes()) {
                                match key.as_ref() {
                                    "s"   => s        = val.to_string(),
                                    "sp"  => sp       = val.to_string(),
                                    "url" => base_url = val.to_string(),
                                    _     => {}
                                }
                            }
                            if !s.is_empty() && !base_url.is_empty() {
                                sig_ciphers.insert(itag, (base_url, sp, s));
                            }
                        }
                    }
                }
            }
        }

        if self.verbose { eprintln!("[debug] Innertube ANDROID_VR: {} direct URL(s) and {} signatureCipher(s) for video {}",
            urls.len(), sig_ciphers.len(), video_id); }
        if urls.len() + sig_ciphers.len() == 0 {
            let raw = serde_json::to_string_pretty(&json).unwrap_or_default();
            if self.verbose { eprintln!("[debug] Raw ANDROID_VR response (1500 chars): {}", &raw[..raw.len().min(1500)]); }
        }
        Ok((urls, sig_ciphers))
    }

    /// Extracts the `n` parameter value from a YouTube URL, if present.
    fn extract_n_param(url: &str) -> Option<String> {
        for prefix in ["&n=", "?n="] {
            if let Some(pos) = url.find(prefix) {
                let start = pos + prefix.len();
                let end = url[start..].find('&').map(|i| start + i).unwrap_or(url.len());
                return Some(url[start..end].to_string());
            }
        }
        None
    }

    /// Replaces the `n` parameter in a URL using the decryption map.
    fn apply_n_map_to_url(url: &str, n_map: &std::collections::HashMap<String, String>, verbose: bool) -> String {
        for prefix in ["&n=", "?n="] {
            if let Some(pos) = url.find(prefix) {
                let start = pos + prefix.len();
                let end = url[start..].find('&').map(|i| start + i).unwrap_or(url.len());
                let n_param = &url[start..end];
                if let Some(decrypted) = n_map.get(n_param) {
                    if verbose { eprintln!("[debug] n-sig : {}... -> {}...",
                        &n_param[..n_param.len().min(12)],
                        &decrypted[..decrypted.len().min(12)]); }
                    return format!("{}{}{}", &url[..start], decrypted, &url[end..]);
                }
                break;
            }
        }
        url.to_string()
    }

    /// Downloads a format's URL to the given output path, using HTTP Range requests
    /// (chunks of ~10 MB, randomised between 95-100 %) — same strategy as yt-dlp Python
    /// to avoid YouTube throttling long-running connections.
    pub async fn download_format(&self, format: &Format, output_path: &str) -> Result<(), Box<dyn Error>> {
        println!("[download] Destination: {}", output_path);

        // yt-dlp uses 10 MiB chunks (randomised between 95 % and 100 % of that size)
        const CHUNK_SIZE: u64 = 10 << 20; // 10 MiB

        // --- Determine total content length ---
        // Try a cheap HEAD request first, fall back to a GET if the server refuses HEAD.
        let content_len: u64 = if let Some(sz) = format.filesize {
            sz
        } else {
            let head_res = self.client
                .head(&format.url)
                .header("Accept-Encoding", "identity")
                .send().await;
            match head_res {
                Ok(r) if r.status().is_success() => r.content_length().unwrap_or(0),
                _ => 0,
            }
        };

        if self.verbose {
            eprintln!("[debug] download_format: content_len={} chunk_size={} CHUNK", content_len, CHUNK_SIZE);
        }

        let mut file = tokio::fs::File::create(output_path).await?;
        let mut downloaded = 0u64;
        let mut rng = rand::thread_rng();
        let dl_start = std::time::Instant::now();

        let pb = if content_len > 0 {
            indicatif::ProgressBar::new(content_len)
        } else {
            indicatif::ProgressBar::new_spinner()
        };

        pb.set_style(
            indicatif::ProgressStyle::default_bar()
                .template(if content_len > 0 {
                    "[download] {bar:40.cyan/bright_black} \x1b[94m{percent:>3}%\x1b[0m of {total_bytes} at {bytes_per_sec:>9.green} ETA {eta:>5.yellow}"
                } else {
                    "[download] {bytes} downloaded at {bytes_per_sec:>9.green}"
                })
                .unwrap()
                .progress_chars("━━ ") // Fill, Head, Empty 
        );

        if content_len == 0 {
            // Size unknown — single request with Accept-Encoding: identity
            if self.verbose { pb.println("[debug] Size unknown, falling back to single request"); }
            let res = self.client
                .get(&format.url)
                .header("Accept-Encoding", "identity")
                .send().await?;
            let status = res.status();
            if !status.is_success() {
                return Err(format!("HTTP {} for format {}", status.as_u16(), format.format_id).into());
            }
            let total = res.content_length().unwrap_or(0);
            if total > 0 {
                pb.set_length(total);
                pb.set_style(
                    indicatif::ProgressStyle::default_bar()
                        .template("[download] {bar:40.cyan/bright_black} \x1b[94m{percent:>3}%\x1b[0m of {total_bytes} at {bytes_per_sec:>9.green} ETA {eta:>5.yellow}")
                        .unwrap()
                        .progress_chars("━━ ")
                );
            }
            let mut stream = res.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                file.write_all(&chunk).await?;
                downloaded += chunk.len() as u64;
                pb.set_position(downloaded);
            }
        } else {
            // Chunked Range-request download — same as yt-dlp Python http_chunk_size strategy
            let mut range_start = 0u64;
            while range_start < content_len {
                // Randomise chunk size between 95 % and 100 % of CHUNK_SIZE (yt-dlp behaviour)
                let effective_chunk = rng.gen_range((CHUNK_SIZE as f64 * 0.95) as u64..=CHUNK_SIZE);
                let range_end = (range_start + effective_chunk - 1).min(content_len - 1);

                if self.verbose {
                    pb.println(format!("[debug] Range: bytes={}-{}", range_start, range_end));
                }

                let res = self.client
                    .get(&format.url)
                    .header("Accept-Encoding", "identity")
                    .header("Range", format!("bytes={}-{}", range_start, range_end))
                    .send().await?;

                let status = res.status();
                // 206 Partial Content is the expected success for a range request.
                // 200 OK means the server ignored the Range header — still usable but no chunking.
                if !status.is_success() {
                    return Err(format!("HTTP {} on range {}-{} for format {}",
                        status.as_u16(), range_start, range_end, format.format_id).into());
                }

                let mut stream = res.bytes_stream();
                while let Some(chunk) = stream.next().await {
                    let chunk = chunk?;
                    file.write_all(&chunk).await?;
                    downloaded += chunk.len() as u64;
                    pb.set_position(downloaded);
                }

                range_start = range_end + 1;
            }
        }

        pb.finish_and_clear();
        let elapsed = dl_start.elapsed();
        let total_secs = elapsed.as_secs_f64().max(0.001);
        let speed = downloaded as f64 / total_secs;
        let secs = elapsed.as_secs();
        let time_str = format!("{:02}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60);
        eprintln!(
            "[download] {} of {:>10} in {} at {}",
            "100%".bright_blue(),
            format_bytes(downloaded),
            time_str,
            format!("{}/s", format_bytes(speed as u64)).bright_green()
        );
        Ok(())
    }
}
