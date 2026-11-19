use crate::hosts::{domain, Hosts};
use std::net::IpAddr;

#[derive(Debug)]
pub struct Question {
    pub name: String,
    pub kind: u16,
    pub end: usize,
}

fn word(data: &[u8], pos: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(pos..pos + 2)?.try_into().ok()?))
}

pub fn question(data: &[u8]) -> Option<Question> {
    if data.len() < 12 || data[2] & 0xf8 != 0 || word(data, 4)? != 1 { return None; }
    let mut pos = 12;
    let mut labels = Vec::new();
    loop {
        let len = *data.get(pos)? as usize;
        pos += 1;
        if len == 0 { break; }
        /* Reject compressed questions rather than follow unchecked pointers. */
        if len > 63 { return None; }
        labels.push(std::str::from_utf8(data.get(pos..pos + len)?).ok()?);
        pos += len;
        if pos > 267 { return None; }
    }
    let name = domain(&labels.join(".")).ok()?;
    let kind = word(data, pos)?;
    if word(data, pos + 2)? != 1 { return None; }
    Some(Question { name, kind, end: pos + 4 })
}

fn header(query: &[u8], q: &Question, rcode: u8) -> Vec<u8> {
    let mut out = query[..q.end].to_vec();
    /* Preserve RD and CD; never assert DNSSEC AD for local answers. */
    out[2] = 0x80 | (query[2] & 1);
    out[3] = 0x80 | (query[3] & 0x10) | rcode;
    out[6..12].fill(0);
    out
}

pub fn failure(query: &[u8]) -> Option<Vec<u8>> {
    Some(header(query, &question(query)?, 2))
}

pub fn answer(hosts: &Hosts, query: &[u8]) -> Option<Vec<u8>> {
    let q = question(query)?;
    let addresses = hosts.lookup(&q.name)?;
    let mut out = header(query, &q, 0);
    let mut count = 0u16;
    for address in addresses {
        let data = match (q.kind, address) {
            (1, IpAddr::V4(ip)) => ip.octets().to_vec(),
            (28, IpAddr::V6(ip)) => ip.octets().to_vec(),
            _ => continue,
        };
        /* Stay within the UDP limit of clients without EDNS. */
        if out.len() + 12 + data.len() > 512 { out[2] |= 2; break; }
        out.extend_from_slice(&[0xc0, 0x0c]);
        out.extend_from_slice(&q.kind.to_be_bytes());
        /* IN class, 60-second TTL. */
        out.extend_from_slice(&[0, 1, 0, 0, 0, 60]);
        out.extend_from_slice(&(data.len() as u16).to_be_bytes());
        out.extend_from_slice(&data);
        count += 1;
    }
    out[6..8].copy_from_slice(&count.to_be_bytes());
    Some(out)
}

pub fn same_question(query: &[u8], response: &[u8]) -> bool {
    if response.len() < 12 || query.get(..2) != response.get(..2) || response[2] & 0x80 == 0 { return false; }
    let mut copy = response.to_vec();
    copy[2] &= 0x07;
    match (question(query), question(&copy)) {
        (Some(a), Some(b)) => a.name == b.name && a.kind == b.kind,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub fn query(name: &str, kind: u16) -> Vec<u8> {
        let mut q = vec![0x12, 0x34, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        for part in name.split('.') { q.push(part.len() as u8); q.extend_from_slice(part.as_bytes()); }
        q.push(0); q.extend_from_slice(&kind.to_be_bytes()); q.extend_from_slice(&[0, 1]); q
    }
    #[test]
    fn ipv4_ipv6_nodata_and_passthrough() {
        let h = Hosts::parse("127.0.0.1 .example.com\n127.0.0.1 example.com\n::1 example.com").unwrap();
        let q = query("example.com", 1);
        let a = answer(&h, &q).unwrap();
        assert_eq!(&a[a.len()-4..], &[127, 0, 0, 1]);
        assert!(same_question(&q, &a));
        assert_eq!(answer(&h, &query("example.com", 28)).unwrap()[7], 1);
        assert_eq!(answer(&h, &query("a.example.com", 28)).unwrap()[7], 0);
        assert_eq!(answer(&h, &query("example.com", 65)).unwrap()[7], 0);
        assert!(answer(&h, &query("unmatched.test", 1)).is_none());
    }
    #[test]
    fn exact_rule_does_not_fall_back_to_a_wildcard_address_family() {
        let h = Hosts::parse("127.0.0.1 .example.com\n::1 example.com").unwrap();
        let q = query("example.com", 1);
        let a = answer(&h, &q).unwrap();
        assert_eq!(a[7], 0);
        assert_eq!(a.len(), q.len());
        assert_eq!(answer(&h, &query("example.com", 28)).unwrap()[7], 1);
        assert_eq!(answer(&h, &query("a.example.com", 1)).unwrap()[7], 1);
    }
    #[test]
    fn malformed_dns_never_panics() {
        for len in 0..512 { assert!(question(&vec![0xff; len]).is_none()); }
        let mut q = query("test.com", 1); q[12] = 0xc0;
        assert!(question(&q).is_none());
        let q = query("test.com", 1);
        for len in 0..q.len() { assert!(question(&q[..len]).is_none()); }
        assert_eq!(failure(&q).unwrap()[3] & 15, 2);
    }
}
