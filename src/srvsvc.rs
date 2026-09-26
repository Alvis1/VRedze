//! Minimal MS-SRVS `NetrShareEnum` (level 1) over DCE/RPC with classic NDR
//! (transfer syntax 2.0). smb-rs only offers NDR64, which Samba rejects.

use anyhow::{Context, bail, ensure};

const PTYPE_REQUEST: u8 = 0;
const PTYPE_RESPONSE: u8 = 2;
const PTYPE_FAULT: u8 = 3;
const PTYPE_BIND: u8 = 11;
const PTYPE_BIND_ACK: u8 = 12;
const FRAG: u16 = 4280;
const OPNUM_NETR_SHARE_ENUM: u16 = 15;

/// MS-SRVS SRVSVC interface 4b324fc8-1670-01d3-1278-5a47bf6ee188 v3.0.
const SRVSVC: ([u8; 16], u32) = (
    guid(
        0x4b324fc8,
        0x1670,
        0x01d3,
        [0x12, 0x78, 0x5a, 0x47, 0xbf, 0x6e, 0xe1, 0x88],
    ),
    3,
);
/// NDR 2.0 transfer syntax 8a885d04-1ceb-11c9-9fe8-08002b104860.
const NDR20: ([u8; 16], u32) = (
    guid(
        0x8a885d04,
        0x1ceb,
        0x11c9,
        [0x9f, 0xe8, 0x08, 0x00, 0x2b, 0x10, 0x48, 0x60],
    ),
    2,
);

const fn guid(a: u32, b: u16, c: u16, d: [u8; 8]) -> [u8; 16] {
    let a = a.to_le_bytes();
    let b = b.to_le_bytes();
    let c = c.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d[0], d[1], d[2], d[3], d[4], d[5], d[6],
        d[7],
    ]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Share {
    pub name: String,
    /// STYPE_*: low bits 0 disk, 1 printer, 2 device, 3 IPC; 0x80000000 special.
    pub kind: u32,
    pub remark: String,
}

impl Share {
    pub fn is_browsable_disk(&self) -> bool {
        self.kind & 0x0fff_ffff == 0 && self.kind & 0x8000_0000 == 0 && !self.name.ends_with('$')
    }
}

fn pdu(ptype: u8, call_id: u32, body: &[u8]) -> Vec<u8> {
    let mut out = vec![5, 0, ptype, 0x03, 0x10, 0, 0, 0];
    out.extend_from_slice(&((16 + body.len()) as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&call_id.to_le_bytes());
    out.extend_from_slice(body);
    out
}

pub fn bind_request() -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&FRAG.to_le_bytes());
    body.extend_from_slice(&FRAG.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes()); // assoc group
    body.extend_from_slice(&[1, 0, 0, 0]); // one context element
    body.extend_from_slice(&0u16.to_le_bytes()); // context id
    body.extend_from_slice(&[1, 0]); // one transfer syntax
    body.extend_from_slice(&SRVSVC.0);
    body.extend_from_slice(&SRVSVC.1.to_le_bytes());
    body.extend_from_slice(&NDR20.0);
    body.extend_from_slice(&NDR20.1.to_le_bytes());
    pdu(PTYPE_BIND, 1, &body)
}

fn header(data: &[u8]) -> anyhow::Result<(u8, &[u8])> {
    ensure!(data.len() >= 16 && data[0] == 5, "Not a DCE/RPC reply");
    ensure!(
        data[4] & 0xf0 == 0x10,
        "Big-endian RPC replies are unsupported"
    );
    ensure!(
        data[3] & 0x03 == 0x03,
        "Fragmented RPC replies are unsupported"
    );
    let frag = u16::from_le_bytes([data[8], data[9]]) as usize;
    ensure!(frag <= data.len() && frag >= 16, "Truncated RPC reply");
    Ok((data[2], &data[16..frag]))
}

pub fn check_bind_ack(data: &[u8]) -> anyhow::Result<()> {
    let (ptype, body) = header(data)?;
    ensure!(
        ptype == PTYPE_BIND_ACK,
        "Share listing refused by server (RPC type {ptype})"
    );
    let mut r = Reader::new(body);
    r.skip(8)?; // frags, assoc group
    let addr_len = r.u16()? as usize;
    r.skip(addr_len)?;
    r.align(4);
    let results = r.u8()?;
    r.skip(3)?;
    ensure!(results >= 1, "No bind result");
    let result = r.u16()?;
    ensure!(
        result == 0,
        "Server rejected the SRVSVC binding (result {result})"
    );
    Ok(())
}

fn push_string(stub: &mut Vec<u8>, text: &str) {
    let units: Vec<u16> = text.encode_utf16().chain([0]).collect();
    let n = units.len() as u32;
    stub.extend_from_slice(&n.to_le_bytes());
    stub.extend_from_slice(&0u32.to_le_bytes());
    stub.extend_from_slice(&n.to_le_bytes());
    for u in units {
        stub.extend_from_slice(&u.to_le_bytes());
    }
    while !stub.len().is_multiple_of(4) {
        stub.push(0);
    }
}

pub fn share_enum_request(server: &str) -> Vec<u8> {
    let mut stub = Vec::new();
    stub.extend_from_slice(&0x0002_0000u32.to_le_bytes()); // ServerName: unique pointer
    push_string(&mut stub, &format!("\\\\{server}"));
    stub.extend_from_slice(&1u32.to_le_bytes()); // InfoStruct.Level
    stub.extend_from_slice(&1u32.to_le_bytes()); // union switch
    stub.extend_from_slice(&0x0002_0004u32.to_le_bytes()); // -> SHARE_INFO_1_CONTAINER
    stub.extend_from_slice(&0u32.to_le_bytes()); // EntriesRead
    stub.extend_from_slice(&0u32.to_le_bytes()); // Buffer: null
    stub.extend_from_slice(&u32::MAX.to_le_bytes()); // PreferedMaximumLength
    stub.extend_from_slice(&0u32.to_le_bytes()); // ResumeHandle: null
    let mut body = Vec::new();
    body.extend_from_slice(&(stub.len() as u32).to_le_bytes()); // alloc hint
    body.extend_from_slice(&0u16.to_le_bytes()); // context id
    body.extend_from_slice(&OPNUM_NETR_SHARE_ENUM.to_le_bytes());
    body.extend_from_slice(&stub);
    pdu(PTYPE_REQUEST, 2, &body)
}

pub fn parse_share_enum_response(data: &[u8]) -> anyhow::Result<Vec<Share>> {
    let (ptype, body) = header(data)?;
    if ptype == PTYPE_FAULT {
        let status = body
            .get(8..12)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()));
        bail!("Server refused share listing (RPC fault {status:#x?})");
    }
    ensure!(ptype == PTYPE_RESPONSE, "Unexpected RPC reply type {ptype}");
    let mut r = Reader::new(body.get(8..).context("Short RPC response")?);
    let level = r.u32()?;
    ensure!(level == 1, "Unexpected share info level {level}");
    r.skip(4)?; // union switch
    if r.u32()? == 0 {
        return Ok(Vec::new()); // no container
    }
    let count = r.u32()? as usize;
    if r.u32()? == 0 {
        return Ok(Vec::new()); // no array
    }
    let max = r.u32()? as usize;
    ensure!(max >= count && count < 10_000, "Implausible share count");
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let name_ptr = r.u32()?;
        let kind = r.u32()?;
        let remark_ptr = r.u32()?;
        entries.push((name_ptr, kind, remark_ptr));
    }
    let mut shares = Vec::with_capacity(count);
    for (name_ptr, kind, remark_ptr) in entries {
        let name = if name_ptr != 0 {
            r.string()?
        } else {
            String::new()
        };
        let remark = if remark_ptr != 0 {
            r.string()?
        } else {
            String::new()
        };
        shares.push(Share { name, kind, remark });
    }
    Ok(shares)
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn take(&mut self, n: usize) -> anyhow::Result<&'a [u8]> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.data.len());
        let end = end.context("Truncated share list")?;
        let bytes = &self.data[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }

    fn skip(&mut self, n: usize) -> anyhow::Result<()> {
        self.take(n).map(|_| ())
    }

    fn align(&mut self, n: usize) {
        self.pos = self.pos.div_ceil(n) * n;
    }

    fn u8(&mut self) -> anyhow::Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> anyhow::Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> anyhow::Result<u32> {
        self.align(4);
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    /// Conformant varying UTF-16 string.
    fn string(&mut self) -> anyhow::Result<String> {
        let _max = self.u32()?;
        let _offset = self.u32()?;
        let actual = self.u32()? as usize;
        ensure!(actual < 65_536, "Implausible string length");
        let bytes = self.take(actual * 2)?;
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&u| u != 0)
            .collect();
        Ok(String::from_utf16_lossy(&units))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_layout_matches_wire_format() {
        // 8a885d04-1ceb-11c9-9fe8-08002b104860 as sent on the wire.
        assert_eq!(
            NDR20.0,
            [
                0x04, 0x5d, 0x88, 0x8a, 0xeb, 0x1c, 0xc9, 0x11, 0x9f, 0xe8, 0x08, 0x00, 0x2b, 0x10,
                0x48, 0x60
            ]
        );
        let bind = bind_request();
        assert_eq!(bind.len(), 72);
        assert_eq!(u16::from_le_bytes([bind[8], bind[9]]) as usize, bind.len());
    }

    fn string(out: &mut Vec<u8>, s: &str) {
        push_string(out, s);
    }

    #[test]
    fn parses_share_list() {
        let mut stub = Vec::new();
        for v in [
            1u32, 1, 0x20000, 2, 0x20004, 2, 0x20008, 0, 0x2000c, 0x2000c, 3, 0x20010,
        ] {
            stub.extend_from_slice(&v.to_le_bytes());
        }
        string(&mut stub, "vr");
        string(&mut stub, "Videos");
        string(&mut stub, "IPC$");
        string(&mut stub, "IPC Service");
        for v in [2u32, 0, 0] {
            stub.extend_from_slice(&v.to_le_bytes()); // total, resume ptr, WERROR
        }
        let mut body = vec![0u8; 8];
        body.extend_from_slice(&stub);
        let shares = parse_share_enum_response(&pdu(PTYPE_RESPONSE, 2, &body)).unwrap();
        assert_eq!(shares.len(), 2);
        assert_eq!(
            (shares[0].name.as_str(), shares[0].remark.as_str()),
            ("vr", "Videos")
        );
        assert!(shares[0].is_browsable_disk());
        assert!(!shares[1].is_browsable_disk());
    }
}
