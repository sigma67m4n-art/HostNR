/* Route only the synthetic DNS IPv4 /32 into the VPN. */
pub fn dns_payload(packet: &[u8]) -> Option<&[u8]> {
    if packet.len() < 28 || packet[0] >> 4 != 4 || packet[9] != 17 { return None; }
    let header = ((packet[0] & 15) as usize) * 4;
    let total = u16::from_be_bytes([packet[2], packet[3]]) as usize;
    if header < 20 || total > packet.len() || total < header + 8 { return None; }
    let fragment = u16::from_be_bytes([packet[6], packet[7]]);
    if fragment & 0x3fff != 0 || packet.get(16..20)? != [10, 77, 0, 2] { return None; }
    let udp = packet.get(header..total)?;
    let len = u16::from_be_bytes([udp[4], udp[5]]) as usize;
    if udp[2..4] != [0, 53] || len < 8 || len != udp.len() { return None; }
    Some(&udp[8..])
}

pub fn response(packet: &[u8], dns: &[u8]) -> Option<Vec<u8>> {
    dns_payload(packet)?;
    if dns.len() > 1472 { return None; }
    let h = ((packet[0] & 15) as usize) * 4;
    let total = 28 + dns.len();
    let mut out = vec![0u8; total];
    out[0] = 0x45;
    out[2..4].copy_from_slice(&(total as u16).to_be_bytes());
    out[6] = 0x40;
    out[8] = 64;
    out[9] = 17;
    out[12..16].copy_from_slice(&packet[16..20]);
    out[16..20].copy_from_slice(&packet[12..16]);
    out[20..22].copy_from_slice(&packet[h+2..h+4]);
    out[22..24].copy_from_slice(&packet[h..h+2]);
    out[24..26].copy_from_slice(&((8 + dns.len()) as u16).to_be_bytes());
    out[28..].copy_from_slice(dns);
    let sum: u32 = out[..20].chunks_exact(2).map(|b| u16::from_be_bytes([b[0], b[1]]) as u32).sum();
    let sum = (sum & 0xffff) + (sum >> 16);
    let checksum = !((sum & 0xffff) + (sum >> 16)) as u16;
    out[10..12].copy_from_slice(&checksum.to_be_bytes());
    /* A zero UDP checksum is valid for IPv4. */
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packet_validation_and_checksum() {
        let mut p = vec![0u8; 32];
        p[0] = 0x45; p[3] = 32; p[9] = 17;
        p[12..16].copy_from_slice(&[10,77,0,1]);
        p[16..20].copy_from_slice(&[10,77,0,2]);
        p[20..28].copy_from_slice(&[0x12,0x34,0,53,0,12,0,0]);
        assert_eq!(dns_payload(&p).unwrap(), &[0;4]);
        let r = response(&p, &[1,2,3]).unwrap();
        assert_eq!(&r[12..16], &[10,77,0,2]);
        assert_eq!(&r[22..24], &[0x12,0x34]);
        let sum: u32 = r[..20].chunks_exact(2).map(|b| u16::from_be_bytes([b[0],b[1]]) as u32).sum();
        assert_eq!((sum & 65535) + (sum >> 16), 65535);
        for n in 0..p.len() { assert!(dns_payload(&p[..n]).is_none()); }
        p[6] = 0x20; assert!(dns_payload(&p).is_none());
    }
}
