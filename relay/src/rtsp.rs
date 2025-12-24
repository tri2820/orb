use anyhow::{anyhow, Result};
use base64::prelude::*;
use std::collections::HashMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::mpsc;
use tokio::time::{timeout, Duration};

/// RTSP client for connecting to RTSP streams
pub struct RtspClient {
    host: String,
    port: u16,
    path: String,
    auth: Option<(String, String)>,
    session_id: Option<String>,
    cseq: u32,
    stream: Option<TcpStream>,
}

#[derive(Debug)]
pub struct SdpMedia {
    pub media_type: String,
    pub attributes: HashMap<String, String>,
}

#[derive(Debug)]
struct RtspResponse {
    status_code: u16,
    status_text: String,
    headers: HashMap<String, String>,
    body: Option<String>,
}

impl RtspClient {
    pub fn new(host: String, port: u16, path: String, auth: Option<(String, String)>) -> Self {
        Self {
            host,
            port,
            path,
            auth,
            session_id: None,
            cseq: 1,
            stream: None,
        }
    }

    /// Connect to RTSP server and start streaming
    pub async fn connect_and_stream(&mut self, data_tx: mpsc::Sender<Vec<u8>>) -> Result<()> {
        // Step 1: OPTIONS
        self.send_options().await?;

        // Step 2: DESCRIBE
        let sdp = self.send_describe().await?;

        // Parse SDP to get media info
        let media = self.parse_sdp(&sdp)?;
        println!("Found {} media streams", media.len());
        // println!("SDP:\n{}", sdp);

        // Step 3: SETUP for the first video stream
        let video_media = media
            .iter()
            .find(|m| m.media_type == "video")
            .ok_or_else(|| anyhow!("No video stream found in SDP"))?;

        // Extract sprop-parameter-sets if available
        let mut parameter_sets = Vec::new();
        if let Some(fmtp) = video_media.attributes.get("fmtp") {
            for part in fmtp.split(';') {
                let part = part.trim();
                if let Some((key, value)) = part.split_once('=') {
                    if key == "sprop-parameter-sets" {
                        for b64_nal in value.split(',') {
                            if let Ok(nal) = BASE64_STANDARD.decode(b64_nal.trim()) {
                                println!("Extracted parameter set NAL from SDP");
                                parameter_sets.push(nal);
                            }
                        }
                    }
                }
            }
        }

        let (rtp_port, rtcp_port) = self.send_setup(video_media).await?;
        println!("RTP port: {}, RTCP port: {}", rtp_port, rtcp_port);

        // Step 4: PLAY
        self.send_play().await?;

        // If we have parameter sets, wrap them in a dummy Data Message and send first?
        // Actually we can just send them as the first packet.
        if !parameter_sets.is_empty() {
            // We need to wrap them in RTP or just have the depacketizer handle them.
            // Simplest: send a "fake" RTP packet with just these NALs.
            // OR even simpler: inject them into the first REAL RTP packet's payload.
            // BUT our depacketizer expects RTP headers.

            // Let's create a tiny fake RTP packet for these NALs.
            // Payload Type 96, Marker 0, Seq 0, TS 0, SSRC 0
            let mut fake_rtp = vec![0u8; 12];
            fake_rtp[0] = 0x80;
            fake_rtp[1] = 96; // H264
            // ... the rest are 0

            // For H264 payload, we can use STAP-A (Type 24) to send SPS/PPS together.
            let mut payload = vec![24]; // STAP-A header
            for nal in parameter_sets {
                payload.extend_from_slice(&(nal.len() as u16).to_be_bytes());
                payload.extend_from_slice(&nal);
            }
            fake_rtp.extend_from_slice(&payload);
            let _ = data_tx.send(fake_rtp).await;
        }

        // Step 5: Start receiving RTP data
        if rtp_port == 0 {
            // TCP interleaved mode
            self.receive_rtp_interleaved(data_tx).await?;
        } else {
            // UDP mode
            self.receive_rtp_stream(rtp_port, data_tx).await?;
        }

        Ok(())
    }

    async fn send_options(&mut self) -> Result<()> {
        let url = format!("rtsp://{}:{}{}", self.host, self.port, self.path);
        let mut headers = Vec::new();

        if let Some((username, password)) = &self.auth {
            let auth_string = format!("{}:{}", username, password);
            let encoded = BASE64_STANDARD.encode(auth_string);
            headers.push(("Authorization".to_string(), format!("Basic {}", encoded)));
        }

        let response = self.send_request("OPTIONS", &url, &headers).await?;
        if response.status_code != 200 {
            return Err(anyhow!("OPTIONS failed: {}", response.status_text));
        }

        Ok(())
    }

    async fn send_describe(&mut self) -> Result<String> {
        let url = format!("rtsp://{}:{}{}", self.host, self.port, self.path);
        let mut headers = vec![
            ("Accept".to_string(), "application/sdp".to_string()),
        ];

        if let Some((username, password)) = &self.auth {
            let auth_string = format!("{}:{}", username, password);
            let encoded = BASE64_STANDARD.encode(auth_string);
            headers.push(("Authorization".to_string(), format!("Basic {}", encoded)));
        }

        let response = self.send_request("DESCRIBE", &url, &headers).await?;
        if response.status_code != 200 {
            return Err(anyhow!("DESCRIBE failed: {}", response.status_text));
        }

        response.body.ok_or_else(|| anyhow!("No SDP body in DESCRIBE response"))
    }

    async fn send_setup(&mut self, media: &SdpMedia) -> Result<(u16, u16)> {
        // Get control attribute or default to trackID=0
        let control = media.attributes
            .get("control")
            .cloned()
            .unwrap_or_else(|| "trackID=0".to_string());

        let url = if control.starts_with("rtsp://") {
            control
        } else {
            // Relative URL - resolve against base URL
            let base = format!("rtsp://{}:{}{}", self.host, self.port, self.path);
            if base.ends_with('/') || control.starts_with('/') {
                format!("{}{}", base, control)
            } else {
                format!("{}/{}", base, control)
            }
        };

        // Try TCP interleaved transport first (more widely supported)
        let mut headers = vec![
            ("Transport".to_string(), "RTP/AVP/TCP;unicast;interleaved=0-1".to_string()),
        ];

        if let Some(session) = &self.session_id {
            headers.push(("Session".to_string(), session.clone()));
        }

        let response = self.send_request("SETUP", &url, &headers).await?;

        // If TCP fails, try UDP
        if response.status_code != 200 {
            println!("TCP interleaved transport failed, trying UDP...");
            let client_port = self.find_available_udp_port().await?;
            let headers = vec![
                ("Transport".to_string(),
                 format!("RTP/AVP;unicast;client_port={}-{}", client_port, client_port + 1)),
            ];

            let response = self.send_request("SETUP", &url, &headers).await?;
            if response.status_code != 200 {
                return Err(anyhow!("SETUP failed with both TCP and UDP transport: {}", response.status_text));
            }

            // Extract session ID from response
            if let Some(session_header) = response.headers.get("Session") {
                self.session_id = Some(session_header.split(';').next().unwrap_or(session_header).to_string());
            }

            return Ok((client_port, client_port + 1));
        }

        // Extract session ID from response
        if let Some(session_header) = response.headers.get("Session") {
            self.session_id = Some(session_header.split(';').next().unwrap_or(session_header).to_string());
        }

        // For TCP interleaved mode, we use dummy ports
        Ok((0, 1))
    }

    async fn send_play(&mut self) -> Result<()> {
        let url = format!("rtsp://{}:{}{}", self.host, self.port, self.path);
        let mut headers = Vec::new();

        if let Some(session) = &self.session_id {
            headers.push(("Session".to_string(), session.clone()));
        }

        let response = self.send_request("PLAY", &url, &headers).await?;
        if response.status_code != 200 {
            return Err(anyhow!("PLAY failed: {}", response.status_text));
        }

        Ok(())
    }

    async fn send_request(
        &mut self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
    ) -> Result<RtspResponse> {
        // Create connection if it doesn't exist or if it's closed
        if self.stream.is_none() {
            self.stream = Some(TcpStream::connect(format!("{}:{}", self.host, self.port)).await?);
        }

        let stream = self.stream.as_mut().unwrap();

        // Build request manually as string
        let mut request = format!("{} {} RTSP/1.0\r\n", method, url);

        // Add required headers
        request.push_str(&format!("CSeq: {}\r\n", self.cseq));
        self.cseq += 1;
        request.push_str("User-Agent: orb-relay/1.0\r\n");

        // Add custom headers
        for (name, value) in headers {
            request.push_str(&format!("{}: {}\r\n", name, value));
        }

        // End headers
        request.push_str("\r\n");

        // Send request
        stream.write_all(request.as_bytes()).await?;

        // Read response
        let mut buffer = vec![0u8; 8192];
        let n = stream.read(&mut buffer).await?;
        let response_str = String::from_utf8_lossy(&buffer[..n]);

        // Debug output for SETUP request/response
        if method == "SETUP" {
            println!("SETUP request:\n{}", request);
            println!("SETUP response:\n{}", response_str);
        }

        self.parse_response(&response_str)
    }

    fn parse_response(&self, response: &str) -> Result<RtspResponse> {
        let lines: Vec<&str> = response.lines().collect();
        if lines.is_empty() {
            return Err(anyhow!("Empty response"));
        }

        // Parse status line
        let status_parts: Vec<&str> = lines[0].split_whitespace().collect();
        if status_parts.len() < 3 {
            return Err(anyhow!("Invalid status line"));
        }

        let status_code = status_parts[1]
            .parse()
            .map_err(|_| anyhow!("Invalid status code"))?;
        let status_text = status_parts[2..].join(" ");

        // Parse headers
        let mut headers = HashMap::new();
        let mut body_start = 1;

        for (i, line) in lines.iter().enumerate().skip(1) {
            if line.is_empty() {
                body_start = i + 1;
                break;
            }

            if let Some(colon_pos) = line.find(':') {
                let key = line[..colon_pos].trim().to_string();
                let value = line[colon_pos + 1..].trim().to_string();
                headers.insert(key, value);
            }
        }

        // Extract body if present
        let body = if body_start < lines.len() {
            Some(lines[body_start..].join("\n"))
        } else {
            None
        };

        Ok(RtspResponse {
            status_code,
            status_text,
            headers,
            body,
        })
    }

    fn parse_sdp(&self, sdp: &str) -> Result<Vec<SdpMedia>> {
        let mut media_list = Vec::new();
        let mut current_media: Option<SdpMedia> = None;

        for line in sdp.lines() {
            let line = line.trim();

            if line.starts_with("m=") {
                // Save previous media if any
                if let Some(media) = current_media.take() {
                    media_list.push(media);
                }

                // Parse new media
                let parts: Vec<&str> = line[2..].split_whitespace().collect();
                if parts.len() >= 1 {
                    current_media = Some(SdpMedia {
                        media_type: parts[0].to_string(),
                        attributes: HashMap::new(),
                    });
                }
            } else if line.starts_with("a=") {
                // Parse attribute
                if let Some(ref mut media) = current_media {
                    let attr_line = &line[2..];
                    if let Some(colon_pos) = attr_line.find(':') {
                        let key = attr_line[..colon_pos].to_string();
                        let value = attr_line[colon_pos + 1..].to_string();
                        media.attributes.insert(key, value);
                    } else {
                        media.attributes.insert(attr_line.to_string(), String::new());
                    }
                }
            }
        }

        // Add last media
        if let Some(media) = current_media {
            media_list.push(media);
        }

        Ok(media_list)
    }

    async fn find_available_udp_port(&self) -> Result<u16> {
        let socket = UdpSocket::bind("0.0.0.0:0").await?;
        let local_addr = socket.local_addr()?;
        Ok(local_addr.port())
    }

    async fn receive_rtp_stream(
        &self,
        rtp_port: u16,
        data_tx: mpsc::Sender<Vec<u8>>,
    ) -> Result<()> {
        let socket = UdpSocket::bind(format!("0.0.0.0:{}", rtp_port)).await?;
        let mut buf = [0u8; 2048];

        println!("Starting to receive RTP data on port {}", rtp_port);

        loop {
            match timeout(Duration::from_secs(5), socket.recv_from(&mut buf)).await {
                Ok(Ok((n, _))) if n > 0 => {
                    if data_tx.send(buf[..n].to_vec()).await.is_err() {
                        break;
                    }
                }
                Ok(Ok(_)) => {} // Empty packet
                Ok(Err(e)) => {
                    eprintln!("Error receiving RTP: {}", e);
                    break;
                }
                Err(_) => {
                    println!("RTP receive timeout, ending stream");
                    break;
                }
            }
        }

        Ok(())
    }

    async fn receive_rtp_interleaved(&mut self, data_tx: mpsc::Sender<Vec<u8>>) -> Result<()> {
        if let Some(stream) = &mut self.stream {
            println!("Starting to receive interleaved RTP data");
            let mut buf = Vec::with_capacity(65536);
            let mut read_buf = [0u8; 4096];

            loop {
                match timeout(Duration::from_secs(5), stream.read(&mut read_buf)).await {
                    Ok(Ok(n)) if n > 0 => {
                        buf.extend_from_slice(&read_buf[..n]);

                        while buf.len() >= 4 {
                            if buf[0] != b'$' {
                                // Lost sync? Try to find next $
                                if let Some(pos) = buf.iter().position(|&b| b == b'$') {
                                    buf.drain(0..pos);
                                } else {
                                    buf.clear();
                                    break;
                                }
                                if buf.len() < 4 { break; }
                            }

                            let len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
                            if buf.len() >= 4 + len {
                                // We have a full packet. Forward the whole thing (including $ header).
                                let packet = buf.drain(0..4 + len).collect::<Vec<u8>>();
                                if data_tx.send(packet).await.is_err() {
                                    return Ok(());
                                }
                            } else {
                                // Need more data
                                break;
                            }
                        }
                    }
                    Ok(Ok(_)) => {} // Empty read
                    Ok(Err(e)) => {
                        eprintln!("Error reading interleaved RTP: {}", e);
                        break;
                    }
                    Err(_) => {
                        println!("Interleaved RTP receive timeout, ending stream");
                        break;
                    }
                }
            }
        } else {
            return Err(anyhow!("No TCP connection available for interleaved mode"));
        }

        Ok(())
    }
}
