use hostnr_core::{hosts::MAX_BYTES, store::Profile};
use std::{io::Read, time::Duration};

pub enum Download { Unchanged, Text { text: String, etag: Option<String>, modified: Option<String> } }

pub fn fetch(source: &str, cached: Option<&Profile>) -> Result<Download, String> {
    let mut url = url::Url::parse(source.trim()).map_err(|_| "Invalid URL")?;
    let agent = ureq::AgentBuilder::new().redirects(0).timeout(Duration::from_secs(20)).build();
    for hop in 0..=5 {
        if url.scheme() != "https" || url.host_str().is_none() || !url.username().is_empty() || url.password().is_some() {
            return Err("Use an HTTPS URL without a username or password".into());
        }
        let mut request = agent.get(url.as_str()).set("User-Agent", "HostNR/0.1").set("Accept", "text/plain");
        if let Some(p) = cached {
            if let Some(tag) = &p.etag { request = request.set("If-None-Match", tag); }
            if let Some(date) = &p.modified { request = request.set("If-Modified-Since", date); }
        }
        let response = request.call().map_err(|e| format!("Download failed: {e}. Your previous cache is still available."))?;
        match response.status() {
            304 if cached.is_some() => return Ok(Download::Unchanged),
            301 | 302 | 303 | 307 | 308 if hop < 5 => {
                url = url.join(response.header("Location").ok_or("Redirect has no destination")?).map_err(|e| e.to_string())?;
            }
            200 => {
                if response.header("Content-Length").and_then(|s| s.parse::<usize>().ok()).is_some_and(|n| n > MAX_BYTES) {
                    return Err("Source exceeds 8 MiB".into());
                }
                let etag = response.header("ETag").map(str::to_owned);
                let modified = response.header("Last-Modified").map(str::to_owned);
                let mut bytes = Vec::new();
                response.into_reader().take((MAX_BYTES + 1) as u64).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
                if bytes.len() > MAX_BYTES { return Err("Source exceeds 8 MiB".into()); }
                let text = String::from_utf8(bytes).map_err(|_| "Source is not UTF-8 text")?;
                hostnr_core::hosts::Hosts::parse(&text)?;
                return Ok(Download::Text { text, etag, modified });
            }
            _ => return Err(format!("Unsupported HTTP response: {}", response.status())),
        }
    }
    Err("Too many redirects".into())
}
