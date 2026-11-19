use std::{collections::HashMap, net::IpAddr};

pub const MAX_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_RECORDS: usize = 200_000;

#[derive(Clone, Debug, Default)]
pub struct Hosts {
    exact: HashMap<String, Vec<IpAddr>>,
    suffix: HashMap<String, Vec<IpAddr>>,
    children: HashMap<String, Vec<IpAddr>>,
    pub records: usize,
    pub wildcards: usize,
}

pub fn domain(value: &str) -> Result<String, String> {
    let value = value.trim_end_matches('.').to_ascii_lowercase();
    if value.is_empty() || value.len() > 253 || !value.is_ascii() {
        return Err("Invalid domain name; use punycode for IDNs".into());
    }
    for label in value.split('.') {
        if label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        {
            return Err(format!("Invalid domain label: {value}"));
        }
    }
    Ok(value)
}

impl Hosts {
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() > MAX_BYTES {
            return Err("File exceeds 8 MiB".into());
        }
        let mut result = Self::default();
        for (line_no, line) in text.trim_start_matches('\u{feff}').lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() { continue; }
            let mut fields = line.split_whitespace();
            let ip: IpAddr = fields.next().unwrap_or("").parse()
                .map_err(|_| format!("Line {}: invalid IP address", line_no + 1))?;
            let mut fields = fields.peekable();
            if fields.peek().is_none() {
                return Err(format!("Line {}: missing domain name", line_no + 1));
            }
            for raw in fields {
                let (kind, name) = if let Some(s) = raw.strip_prefix("*.") {
                    (2, s)
                } else if let Some(s) = raw.strip_prefix('.') {
                    (1, s)
                } else { (0, raw) };
                let name = domain(name).map_err(|e| format!("Line {}: {e}", line_no + 1))?;
                let map = match kind { 1 => &mut result.suffix, 2 => &mut result.children, _ => &mut result.exact };
                let addresses = map.entry(name).or_default();
                if !addresses.contains(&ip) {
                    addresses.push(ip);
                    result.records += 1;
                    if kind != 0 { result.wildcards += 1; }
                    if result.records > MAX_RECORDS { return Err("Maximum 200,000 records exceeded".into()); }
                }
            }
        }
        if result.records == 0 { return Err("No valid hosts records found".into()); }
        Ok(result)
    }

    /*
     * Exact rules win; otherwise walk suffixes from most to least specific.
     * A match with the wrong address family still produces DNS NODATA.
     */
    pub fn lookup(&self, name: &str) -> Option<&[IpAddr]> {
        if let Some(v) = self.exact.get(name) { return Some(v); }
        let mut suffix = name;
        loop {
            if suffix != name {
                if let Some(v) = self.children.get(suffix) { return Some(v); }
            }
            if let Some(v) = self.suffix.get(suffix) { return Some(v); }
            suffix = suffix.split_once('.')?.1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wildcard_precedence_and_boundaries() {
        let h = Hosts::parse("1.1.1.1 .example.com\n2.2.2.2 *.deep.example.com\n3.3.3.3 x.deep.example.com").unwrap();
        let ip = |s: &str| h.lookup(s).unwrap()[0].to_string();
        assert_eq!(ip("example.com"), "1.1.1.1");
        assert_eq!(ip("a.example.com"), "1.1.1.1");
        assert_eq!(ip("deep.example.com"), "1.1.1.1");
        assert_eq!(ip("a.deep.example.com"), "2.2.2.2");
        assert_eq!(ip("x.deep.example.com"), "3.3.3.3");
        assert!(h.lookup("notexample.com").is_none());
    }
    #[test]
    fn aliases_ipv6_comments_bom_and_duplicates() {
        let h = Hosts::parse("\u{feff}127.0.0.1 EXAMPLE.COM. alias.test # hi\n::1 example.com\n::1 example.com").unwrap();
        assert_eq!(h.records, 3);
        assert_eq!(h.lookup("example.com").unwrap().len(), 2);
    }
    #[test]
    fn rejects_invalid_and_empty_input() {
        for s in ["", "# comment", "127.0.0.1", "oops a.com", "0.0.0.0 a.*.com", "0.0.0.0 a..com", "0.0.0.0 https://a.com"] {
            assert!(Hosts::parse(s).is_err(), "{s}");
        }
        assert!(Hosts::parse(&"#".repeat(MAX_BYTES + 1)).is_err());
    }
}
