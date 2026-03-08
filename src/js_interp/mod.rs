use std::collections::HashMap;
use std::error::Error;
use std::io::Write;
use std::process::{Command, Stdio};

// These scripts are embedded in the binary at compile time.
// yt.solver.lib.js provides meriyah + astring (JS AST parser/generator).
// yt.solver.core.js est le solveur de challenges YouTube de yt-dlp.
const SOLVER_LIB: &str = include_str!("vendor/yt.solver.lib.js");
const SOLVER_CORE: &str = include_str!("vendor/yt.solver.core.js");

/// Solver result for a set of n and sig challenges.
pub struct SolverResult {
    /// Mapping n_value_original -> n_value_decrypted
    pub n_values: HashMap<String, String>,
    /// Mapping sig_value_original -> sig_value_decrypted
    pub sig_values: HashMap<String, String>,
}

/// Resolves n and/or sig challenges using Node.js as the JS runtime
/// with the yt-dlp/ejs solver (yt.solver.core.js + yt.solver.lib.js).
///
/// `player_js`: content of the YouTube TV player JavaScript (player 9f4cc5e4, tv variant).
/// `n_challenges`: list of n values to decrypt.
/// `sig_challenges`: list of signatures to decrypt.
pub fn solve_challenges(
    player_js: &str,
    n_challenges: &[&str],
    sig_challenges: &[&str],
    verbose: bool,
) -> Result<SolverResult, Box<dyn Error>> {
    if n_challenges.is_empty() && sig_challenges.is_empty() {
        return Ok(SolverResult {
            n_values: HashMap::new(),
            sig_values: HashMap::new(),
        });
    }

    // Build the JSON request array
    let mut requests = Vec::new();
    if !n_challenges.is_empty() {
        let challenges_json: Vec<String> = n_challenges
            .iter()
            .map(|c| serde_json::to_string(c).unwrap())
            .collect();
        requests.push(format!(
            r#"{{"type":"n","challenges":[{}]}}"#,
            challenges_json.join(",")
        ));
    }
    if !sig_challenges.is_empty() {
        let challenges_json: Vec<String> = sig_challenges
            .iter()
            .map(|c| serde_json::to_string(c).unwrap())
            .collect();
        requests.push(format!(
            r#"{{"type":"sig","challenges":[{}]}}"#,
            challenges_json.join(",")
        ));
    }

    let player_json = serde_json::to_string(player_js)?;
    let input_json = format!(
        r#"{{"type":"player","player":{},"requests":[{}],"output_preprocessed":false}}"#,
        player_json,
        requests.join(",")
    );

    // Script stdin : lib + Object.assign + core + appel jsc
    let stdin_script = format!(
        "{}\nObject.assign(globalThis, lib);\n{}\nconsole.log(JSON.stringify(jsc({})));",
        SOLVER_LIB, SOLVER_CORE, input_json
    );

    if verbose {
        eprintln!("[debug] Launching Node.js to solve {} n challenge(s) and {} sig challenge(s)",
            n_challenges.len(), sig_challenges.len());
    }

    let mut child = Command::new("node")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to launch node: {}. Make sure Node.js is installed.", e))?;

    {
        let stdin = child.stdin.as_mut().ok_or("stdin unavailable")?;
        stdin.write_all(stdin_script.as_bytes())?;
    }

    let output = child.wait_with_output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("node failed (code {:?}): {}", output.status.code(), stderr).into());
    }

    let stdout = String::from_utf8(output.stdout)?;
    let json: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|e| format!("Invalid JSON from node: {} — output: {}", e, &stdout[..stdout.len().min(200)]))?;

    if json["type"] == "error" {
        return Err(format!("Solver error: {}", json["error"]).into());
    }

    let responses = json["responses"]
        .as_array()
        .ok_or("Missing 'responses' field")?;

    let mut result = SolverResult {
        n_values: HashMap::new(),
        sig_values: HashMap::new(),
    };

    let mut resp_idx = 0;
    if !n_challenges.is_empty() {
        if let Some(resp) = responses.get(resp_idx) {
            if resp["type"] == "result" {
                if let Some(data) = resp["data"].as_object() {
                    for (k, v) in data {
                        if let Some(val) = v.as_str() {
                            result.n_values.insert(k.clone(), val.to_string());
                        }
                    }
                }
            } else {
                eprintln!("[warn] n challenge failed: {}", resp["error"]);
            }
        }
        resp_idx += 1;
    }
    if !sig_challenges.is_empty() {
        if let Some(resp) = responses.get(resp_idx) {
            if resp["type"] == "result" {
                if let Some(data) = resp["data"].as_object() {
                    for (k, v) in data {
                        if let Some(val) = v.as_str() {
                            result.sig_values.insert(k.clone(), val.to_string());
                        }
                    }
                }
            } else {
                eprintln!("[warn] sig challenge failed: {}", resp["error"]);
            }
        }
    }

    Ok(result)
}

/// Decrypts the n value from a YouTube URL via the Node.js solver.
/// Returns the decrypted n value, or the original value on failure.
pub fn decrypt_n_sig(n_token: &str, player_js: &str) -> String {
    match solve_challenges(player_js, &[n_token], &[], false) {
        Ok(result) => result
            .n_values
            .get(n_token)
            .cloned()
            .unwrap_or_else(|| {
                eprintln!("[warn] n value not found in solver response");
                n_token.to_string()
            }),
        Err(e) => {
            eprintln!("[warn] decrypt_n_sig failed: {}", e);
            n_token.to_string()
        }
    }
}

/// Decrypts a YouTube signature via the Node.js solver.
pub fn decrypt_signature(ciphered_sig: &str, player_js: &str) -> Result<String, Box<dyn Error>> {
    let result = solve_challenges(player_js, &[], &[ciphered_sig], false)?;
    Ok(result
        .sig_values
        .get(ciphered_sig)
        .cloned()
        .unwrap_or_else(|| {
            eprintln!("[warn] Signature not found in solver response");
            ciphered_sig.to_string()
        }))
}
