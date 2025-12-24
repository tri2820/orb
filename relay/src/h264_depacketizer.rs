use anyhow::{Result, bail};

pub struct H264Depacketizer {
    fu_buffer: Vec<u8>,
}

pub struct H264Frame {
    pub data: Vec<u8>,
}

impl H264Depacketizer {
    pub fn new() -> Self {
        Self {
            fu_buffer: Vec::new(),
        }
    }

    pub fn push(&mut self, payload: &[u8]) -> Result<Option<H264Frame>> {
        if payload.is_empty() {
            return Ok(None);
        }

        let nalu_header = payload[0];
        let nalu_type = nalu_header & 0x1F;

        match nalu_type {
            1..=23 => {
                // Single NAL unit
                Ok(Some(H264Frame { data: payload.to_vec() }))
            }
            24 => {
                // STAP-A (Aggregation packet)
                let mut offset = 1;
                let mut data = Vec::new();
                while offset + 2 <= payload.len() {
                    let size = u16::from_be_bytes([payload[offset], payload[offset + 1]]) as usize;
                    offset += 2;
                    if offset + size <= payload.len() {
                        // For STAP-A, we need to keep the NALs separable.
                        // We'll use Annex B inside the bundled data if needed,
                        // but let's just return the whole segment for now.
                        data.extend_from_slice(&[0, 0, 0, 1]);
                        data.extend_from_slice(&payload[offset..offset + size]);
                        offset += size;
                    } else {
                        break;
                    }
                }
                Ok(Some(H264Frame { data }))
            }
            28 => {
                // FU-A (Fragmentation unit)
                if payload.len() < 2 {
                    bail!("FU-A packet too short");
                }
                let fu_header = payload[1];
                let start_bit = (fu_header & 0x80) != 0;
                let end_bit = (fu_header & 0x40) != 0;
                let nalu_type = fu_header & 0x1F;
                let nri = (nalu_header & 0x60) >> 5;
                let f = (nalu_header & 0x80) >> 7;

                if start_bit {
                    self.fu_buffer.clear();
                    let reconstructed_header = (f << 7) | (nri << 5) | nalu_type;
                    self.fu_buffer.push(reconstructed_header);
                }

                self.fu_buffer.extend_from_slice(&payload[2..]);

                if end_bit {
                    let data = self.fu_buffer.clone();
                    self.fu_buffer.clear();
                    return Ok(Some(H264Frame { data }));
                }

                Ok(None)
            }
            _ => {
                Ok(None)
            }
        }
    }
}
